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
use crate::http::models::{RequestData, RequestId};
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
            start,
            in_scope,
            &self.ctx,
        )
        .await)
    }
}
