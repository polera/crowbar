use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::channel::ProxyToUi;
use crate::http::models::{HttpVersion, RequestData, RequestId, ResponseData};
use crate::proxy::intercept::{InterceptDecision, InterceptState};
use crate::proxy::tunnel;
use crate::tls::cert_cache::CertCache;

pub struct ProxyHandler {
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    cert_cache: Arc<CertCache>,
    intercept: Arc<InterceptState>,
}

impl ProxyHandler {
    pub fn new(
        ui_tx: mpsc::UnboundedSender<ProxyToUi>,
        cert_cache: Arc<CertCache>,
        intercept: Arc<InterceptState>,
    ) -> Self {
        Self {
            ui_tx,
            cert_cache,
            intercept,
        }
    }

    pub async fn handle(
        &self,
        req: Request<Incoming>,
        _client_addr: SocketAddr,
    ) -> Result<Response<Full<Bytes>>, hyper::Error> {
        if req.method() == Method::CONNECT {
            return self.handle_connect(req).await;
        }

        self.handle_plain_http(req).await
    }

    async fn handle_connect(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<Full<Bytes>>, hyper::Error> {
        let host = req.uri().host().unwrap_or("unknown").to_string();
        let port = req.uri().port_u16().unwrap_or(443);

        let cert_cache = self.cert_cache.clone();
        let ui_tx = self.ui_tx.clone();
        let intercept = self.intercept.clone();

        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    tunnel::handle_tunnel(upgraded, host, port, cert_cache, ui_tx, intercept)
                        .await;
                }
                Err(e) => {
                    warn!("CONNECT upgrade failed: {}", e);
                }
            }
        });

        Ok(Response::new(Full::new(Bytes::new())))
    }

    async fn handle_plain_http(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<Full<Bytes>>, hyper::Error> {
        let request_id = RequestId::next();
        let start = Instant::now();

        let uri = req.uri().clone();
        let method = req.method().clone();
        let version = match req.version() {
            hyper::Version::HTTP_10 => HttpVersion::Http10,
            hyper::Version::HTTP_11 => HttpVersion::Http11,
            hyper::Version::HTTP_2 => HttpVersion::Http2,
            _ => HttpVersion::Http11,
        };

        let host = uri
            .host()
            .map(|h| h.to_string())
            .or_else(|| {
                req.headers()
                    .get(hyper::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.split(':').next().unwrap_or(s).to_string())
            })
            .unwrap_or_default();

        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
            .collect();

        let (parts, body) = req.into_parts();
        let body_bytes = body.collect().await?.to_bytes();

        let mut request_data = RequestData {
            id: request_id,
            method: method.to_string(),
            uri: uri.to_string(),
            host: host.clone(),
            version,
            headers: headers.clone(),
            body: body_bytes.clone(),
            is_tls: false,
            timestamp: std::time::SystemTime::now(),
        };

        let _ = self
            .ui_tx
            .send(ProxyToUi::RequestCaptured(request_data.clone()));

        // Check intercept
        if let Some(rx) = self.intercept.intercept_request(&request_data, &self.ui_tx) {
            match rx.await {
                Ok(InterceptDecision::Drop) => {
                    return Ok(Response::builder()
                        .status(503)
                        .body(Full::new(Bytes::from("Request dropped by interceptor")))
                        .unwrap());
                }
                Ok(InterceptDecision::ForwardEdited(edited)) => {
                    request_data = edited;
                }
                Ok(InterceptDecision::Forward) => {}
                Err(_) => {
                    // Oneshot sender dropped (TUI closed), forward anyway
                }
            }
        }

        let upstream_host = uri.host().unwrap_or(&host);
        let upstream_port = uri.port_u16().unwrap_or(80);
        let addr = format!("{}:{}", upstream_host, upstream_port);

        let upstream = match TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to connect to upstream {}: {}", addr, e);
                let _ = self.ui_tx.send(ProxyToUi::RequestError(
                    request_id,
                    format!("Connection failed: {}", e),
                ));
                return Ok(Response::builder()
                    .status(502)
                    .body(Full::new(Bytes::from(format!(
                        "Failed to connect to upstream: {}",
                        e
                    ))))
                    .unwrap());
            }
        };

        let io = TokioIo::new(upstream);
        let (mut sender, conn) = match ClientBuilder::new()
            .preserve_header_case(true)
            .title_case_headers(true)
            .handshake(io)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                warn!("Upstream handshake failed for {}: {}", addr, e);
                let _ = self.ui_tx.send(ProxyToUi::RequestError(
                    request_id,
                    format!("Handshake failed: {}", e),
                ));
                return Ok(Response::builder()
                    .status(502)
                    .body(Full::new(Bytes::from(format!(
                        "Upstream handshake failed: {}",
                        e
                    ))))
                    .unwrap());
            }
        };

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                debug!("Upstream connection task ended: {}", e);
            }
        });

        let path_and_query = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        let fwd_headers = &request_data.headers;
        let fwd_body = request_data.body.clone();

        let mut upstream_req = Request::builder()
            .method(parts.method)
            .uri(path_and_query)
            .version(parts.version);

        for (key, value) in fwd_headers {
            upstream_req = upstream_req.header(key.as_str(), value.as_str());
        }

        let upstream_req = upstream_req
            .body(Full::new(fwd_body))
            .expect("building upstream request");

        let upstream_resp = match sender.send_request(upstream_req).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("Upstream request failed for {}: {}", addr, e);
                let _ = self.ui_tx.send(ProxyToUi::RequestError(
                    request_id,
                    format!("Request failed: {}", e),
                ));
                return Ok(Response::builder()
                    .status(502)
                    .body(Full::new(Bytes::from(format!(
                        "Upstream request failed: {}",
                        e
                    ))))
                    .unwrap());
            }
        };

        let resp_status = upstream_resp.status().as_u16();
        let resp_version = match upstream_resp.version() {
            hyper::Version::HTTP_10 => HttpVersion::Http10,
            hyper::Version::HTTP_11 => HttpVersion::Http11,
            hyper::Version::HTTP_2 => HttpVersion::Http2,
            _ => HttpVersion::Http11,
        };
        let resp_headers: Vec<(String, String)> = upstream_resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
            .collect();

        let resp_body = upstream_resp.collect().await?.to_bytes();
        let duration = start.elapsed();

        let response_data = ResponseData {
            status: resp_status,
            reason: http::StatusCode::from_u16(resp_status)
                .map(|s| s.canonical_reason().unwrap_or(""))
                .unwrap_or("")
                .to_string(),
            version: resp_version,
            headers: resp_headers.clone(),
            body: resp_body.clone(),
            duration,
        };

        let _ = self
            .ui_tx
            .send(ProxyToUi::ResponseReceived(request_id, response_data));

        let mut response = Response::builder().status(resp_status);
        for (key, value) in &resp_headers {
            response = response.header(key.as_str(), value.as_str());
        }

        Ok(response.body(Full::new(resp_body)).unwrap())
    }
}
