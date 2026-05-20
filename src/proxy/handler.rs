use std::net::SocketAddr;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tracing::{debug, warn};

use crate::channel::ProxyToUi;
use crate::http::models::{RequestData, RequestId, ResponseData};
use crate::proxy::intercept::InterceptDecision;
use crate::proxy::tunnel;
use crate::proxy::ProxyContext;
use crate::rules;

pub struct ProxyHandler {
    ctx: ProxyContext,
}

impl ProxyHandler {
    pub fn new(ctx: ProxyContext) -> Self {
        Self { ctx }
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

        let ctx = self.ctx.clone();

        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    tunnel::handle_tunnel(upgraded, host, port, ctx).await;
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
        let version = req.version().into();

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

        let headers = crate::http::models::extract_headers(req.headers());

        let (parts, body) = req.into_parts();
        let body_bytes = body.collect().await?.to_bytes();

        let in_scope = self.ctx.scope.is_in_scope(&host);

        let mut request_data = RequestData {
            id: request_id,
            method: method.to_string(),
            uri: uri.to_string(),
            host: host.clone(),
            version,
            headers,
            body: body_bytes,
            is_tls: false,
            is_grpc: false,
            timestamp: std::time::SystemTime::now(),
        };

        if in_scope {
            let _ = self
                .ctx.ui_tx
                .send(ProxyToUi::RequestCaptured(request_data.clone()));
        }

        if in_scope
            && let Some(rx) = self.ctx.intercept.intercept_request(&request_data, &self.ctx.ui_tx) {
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
                    Err(_) => {}
                }
            }

        rules::apply_request_rules(
            &self.ctx.rules,
            &mut request_data.uri,
            &mut request_data.headers,
            &mut request_data.body,
        );

        let upstream_host = uri.host().unwrap_or(&host);
        let upstream_port = uri.port_u16().unwrap_or(80);
        let addr = format!("{}:{}", upstream_host, upstream_port);

        let upstream = match TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to connect to upstream {}: {}", addr, e);
                let _ = self.ctx.ui_tx.send(ProxyToUi::RequestError(
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
                let _ = self.ctx.ui_tx.send(ProxyToUi::RequestError(
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
                let _ = self.ctx.ui_tx.send(ProxyToUi::RequestError(
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
        let resp_version = upstream_resp.version().into();
        let mut resp_headers = crate::http::models::extract_headers(upstream_resp.headers());

        let mut resp_body = upstream_resp.collect().await?.to_bytes();
        let duration = start.elapsed();

        rules::apply_response_rules(&self.ctx.rules, &mut resp_headers, &mut resp_body);

        let response_data = ResponseData {
            status: resp_status,
            reason: crate::http::models::status_reason(resp_status),
            version: resp_version,
            headers: resp_headers.clone(),
            body: resp_body.clone(),
            trailers: Vec::new(),
            duration,
        };

        if in_scope {
            let _ = self
                .ctx.ui_tx
                .send(ProxyToUi::ResponseReceived(request_id, response_data));
        }

        let mut response = Response::builder().status(resp_status);
        for (key, value) in &resp_headers {
            response = response.header(key.as_str(), value.as_str());
        }

        Ok(response.body(Full::new(resp_body)).unwrap())
    }
}
