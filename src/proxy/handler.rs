use std::net::SocketAddr;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tracing::warn;

use crate::channel::ProxyToUi;
use crate::http::models::{HttpVersion, RequestData, RequestId, ResponseData};
use crate::proxy::intercept::InterceptDecision;
use crate::proxy::tunnel;
use crate::proxy::{ProxyContext, TimingContext};
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
        let mut timing = TimingContext::new();

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

        if tunnel::is_websocket_upgrade(&req) {
            return self.handle_plain_ws_upgrade(req, &host).await;
        }

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
            Ok(s) => {
                timing.tcp_connected = Some(Instant::now());
                s
            }
            Err(e) => {
                warn!("Failed to connect to upstream {}: {}", addr, e);
                let _ = self.ctx.ui_tx.send(ProxyToUi::RequestError(
                    request_id,
                    format!("Connection failed: {}", e),
                ));
                return Ok(crate::proxy::bad_gateway(&format!(
                    "Failed to connect to upstream: {}",
                    e
                )));
            }
        };

        let io = TokioIo::new(upstream);
        let path_and_query = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        Ok(crate::proxy::forward_h1(
            io,
            parts.version,
            path_and_query,
            &request_data,
            timing,
            in_scope,
            &self.ctx,
        )
        .await)
    }

    async fn handle_plain_ws_upgrade(
        &self,
        req: Request<Incoming>,
        host: &str,
    ) -> Result<Response<Full<Bytes>>, hyper::Error> {
        let in_scope = self.ctx.scope.is_in_scope(host);
        let request_id = RequestId::next();

        let headers = crate::http::models::extract_headers(req.headers());

        let uri = req.uri().clone();
        let port = uri.port_u16().unwrap_or(80);

        let full_uri = format!(
            "ws://{}{}",
            host,
            uri.path_and_query()
                .map(|pq| pq.as_str())
                .unwrap_or("/")
        );

        let request_data = RequestData {
            id: request_id,
            method: req.method().to_string(),
            uri: full_uri,
            host: host.to_string(),
            version: HttpVersion::Http11,
            headers: headers.clone(),
            body: Bytes::new(),
            is_tls: false,
            is_grpc: false,
            timestamp: std::time::SystemTime::now(),
        };

        if in_scope {
            let _ = self
                .ctx
                .ui_tx
                .send(ProxyToUi::RequestCaptured(request_data));
        }

        let upstream_host = uri.host().unwrap_or(host);
        let addr = format!("{}:{}", upstream_host, port);
        let tcp = match TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!("WebSocket: failed to connect to upstream {}: {}", addr, e);
                let _ = self.ctx.ui_tx.send(ProxyToUi::RequestError(
                    request_id,
                    format!("Connection failed: {}", e),
                ));
                return Ok(crate::proxy::bad_gateway(&format!(
                    "Connection failed: {}",
                    e
                )));
            }
        };

        let io = TokioIo::new(tcp);
        let (mut sender, conn) = match hyper::client::conn::http1::Builder::new()
            .preserve_header_case(true)
            .title_case_headers(true)
            .handshake(io)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                warn!("WebSocket: upstream handshake failed: {}", e);
                return Ok(crate::proxy::bad_gateway(&format!(
                    "HTTP handshake failed: {}",
                    e
                )));
            }
        };

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!("WebSocket upstream connection ended: {}", e);
            }
        });

        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|pq| pq.to_string())
            .unwrap_or_else(|| "/".to_string());

        let mut upstream_req = hyper::Request::builder()
            .method(req.method())
            .uri(&path_and_query)
            .version(req.version());

        for (key, value) in req.headers() {
            upstream_req = upstream_req.header(key, value);
        }

        let client_req_for_upgrade = req;

        let upstream_req = upstream_req
            .body(Full::new(Bytes::new()))
            .expect("building websocket upgrade request");

        let upstream_resp = match sender.send_request(upstream_req).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("WebSocket: upstream request failed: {}", e);
                return Ok(crate::proxy::bad_gateway(&format!(
                    "Request failed: {}",
                    e
                )));
            }
        };

        let resp_status = upstream_resp.status().as_u16();

        if resp_status != 101 {
            let resp_headers = crate::http::models::extract_headers(upstream_resp.headers());
            let resp_body = upstream_resp
                .collect()
                .await
                .map(|b| b.to_bytes())
                .unwrap_or_default();

            let mut response = hyper::Response::builder().status(resp_status);
            for (key, value) in &resp_headers {
                response = response.header(key.as_str(), value.as_str());
            }
            return Ok(response.body(Full::new(resp_body)).unwrap());
        }

        let resp_headers = crate::http::models::extract_headers(upstream_resp.headers());

        let response_data = ResponseData {
            status: 101,
            reason: "Switching Protocols".to_string(),
            version: HttpVersion::Http11,
            headers: resp_headers.clone(),
            body: Bytes::new(),
            trailers: Vec::new(),
            duration: std::time::Duration::ZERO,
            timing: None,
        };
        if in_scope {
            let _ = self
                .ctx
                .ui_tx
                .send(ProxyToUi::ResponseReceived(request_id, response_data));
        }

        let ui_tx_clone = self.ctx.ui_tx.clone();
        let host_owned = host.to_string();

        tokio::spawn(async move {
            let upstream_upgraded = match hyper::upgrade::on(upstream_resp).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(
                        "WebSocket upstream upgrade failed for {}: {}",
                        host_owned,
                        e
                    );
                    return;
                }
            };
            let client_upgraded = match hyper::upgrade::on(client_req_for_upgrade).await {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(
                        "WebSocket client upgrade failed for {}: {}",
                        host_owned,
                        e
                    );
                    return;
                }
            };

            let client_io = TokioIo::new(client_upgraded);
            let upstream_io = TokioIo::new(upstream_upgraded);

            crate::proxy::ws_relay::relay(
                client_io,
                upstream_io,
                request_id,
                ui_tx_clone,
                in_scope,
            )
            .await;
        });

        let mut response = hyper::Response::builder().status(101);
        for (key, value) in &resp_headers {
            response = response.header(key.as_str(), value.as_str());
        }

        Ok(response.body(Full::new(Bytes::new())).unwrap())
    }
}
