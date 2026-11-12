use std::net::SocketAddr;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
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
        let Some(host) = req.uri().host() else {
            return Ok(crate::proxy::bad_gateway("CONNECT requires an authority"));
        };
        let host = host.trim_matches(['[', ']']).to_string();
        let port = req.uri().port_u16().unwrap_or(443);

        let ctx = self.ctx.clone();

        self.ctx.spawn(async move {
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
                    .and_then(|value| value.parse::<http::uri::Authority>().ok())
                    .map(|authority| authority.host().to_string())
            })
            .unwrap_or_default();

        let headers = crate::http::models::extract_headers(req.headers());

        if tunnel::is_websocket_upgrade(&req) {
            return self.handle_plain_ws_upgrade(req, &host).await;
        }

        let (_, body) = req.into_parts();
        let body_bytes = match crate::proxy::io_timeout(
            http_body_util::Limited::new(body, self.ctx.limits.max_body_bytes).collect(),
        )
        .await
        {
            Ok(body) => body.to_bytes(),
            Err(error) if error.root_cause().is::<http_body_util::LengthLimitError>() => {
                return Ok(crate::proxy::payload_too_large());
            }
            Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
        };

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
                .ctx
                .ui_tx
                .send(ProxyToUi::RequestCaptured(request_data.clone()))
                .await;
        }

        let decision = if in_scope {
            match self
                .ctx
                .intercept
                .intercept_request(&request_data, &self.ctx.ui_tx)
                .await
            {
                Ok(decision) => decision,
                Err(error) => return Ok(crate::proxy::service_unavailable(&error.to_string())),
            }
        } else {
            None
        };
        if let Some(rx) = decision {
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
                    return Ok(crate::proxy::service_unavailable(
                        "Interceptor disconnected",
                    ));
                }
            }
        }

        if let Err(error) = rules::apply_request_rules(
            &self.ctx.rules,
            &mut request_data.uri,
            &mut request_data.headers,
            &mut request_data.body,
            self.ctx.limits.max_body_bytes,
        ) {
            self.ctx.report_error(request_id, error.to_string()).await;
            return Ok(crate::proxy::bad_gateway(&error.to_string()));
        }

        let fallback_authority = request_data
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map_or(request_data.host.as_str(), |(_, value)| value.as_str());
        let target = match crate::proxy::resolve_upstream_target(
            &request_data.uri,
            fallback_authority,
            80,
        ) {
            Ok(target) => target,
            Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
        };

        let upstream = match crate::proxy::connect_tcp(&target.host, target.port).await {
            Ok(s) => {
                timing.tcp_connected = Some(Instant::now());
                s
            }
            Err(e) => {
                warn!(
                    "Failed to connect to upstream {}:{}: {}",
                    target.host, target.port, e
                );
                self.ctx
                    .report_error(request_id, format!("Connection failed: {e}"))
                    .await;
                return Ok(crate::proxy::bad_gateway(&format!(
                    "Failed to connect to upstream: {}",
                    e
                )));
            }
        };

        if target.uses_tls(request_data.is_tls) {
            let connector = tokio_rustls::TlsConnector::from(self.ctx.tls_client_config.clone());
            let stream = match crate::proxy::io_timeout(
                connector.connect(crate::tls::server_name_or_localhost(&target.host), upstream),
            )
            .await
            {
                Ok(stream) => stream,
                Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
            };
            timing.tls_done = Some(Instant::now());
            return Ok(crate::proxy::forward_h1(
                TokioIo::new(stream),
                request_data.version.into(),
                &target.path_and_query,
                &request_data,
                timing,
                in_scope,
                &self.ctx,
            )
            .await);
        }
        let io = TokioIo::new(upstream);
        Ok(crate::proxy::forward_h1(
            io,
            request_data.version.into(),
            &target.path_and_query,
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
        let fallback_authority = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map_or(host, |(_, value)| value.as_str());
        let target =
            match crate::proxy::resolve_upstream_target(&uri.to_string(), fallback_authority, 80) {
                Ok(target) => target,
                Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
            };
        let port = target.port;

        let full_uri = format!(
            "ws://{}{}",
            crate::proxy::format_authority(&target.host, port, 80),
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
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
                .send(ProxyToUi::RequestCaptured(request_data))
                .await;
        }

        let upstream_host = &target.host;
        let tcp = match crate::proxy::connect_tcp(upstream_host, port).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "WebSocket: failed to connect to upstream {}:{}: {}",
                    upstream_host, port, e
                );
                self.ctx
                    .report_error(request_id, format!("Connection failed: {e}"))
                    .await;
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

        self.ctx.spawn(async move {
            if let Err(e) = conn.with_upgrades().await {
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

        for (key, value) in crate::proxy::end_to_end_headers(&headers, true) {
            let value = match crate::http::models::header_value(value) {
                Ok(value) => value,
                Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
            };
            upstream_req = upstream_req.header(key.as_str(), value);
        }

        let client_req_for_upgrade = req;

        let upstream_req = match upstream_req.body(Full::new(Bytes::new())) {
            Ok(request) => request,
            Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
        };

        let upstream_resp = match crate::proxy::io_timeout(sender.send_request(upstream_req)).await
        {
            Ok(resp) => resp,
            Err(e) => {
                warn!("WebSocket: upstream request failed: {}", e);
                return Ok(crate::proxy::bad_gateway(&format!("Request failed: {}", e)));
            }
        };

        let resp_status = upstream_resp.status().as_u16();

        if resp_status != 101 {
            return Ok(crate::proxy::process_h1_response(
                upstream_resp,
                request_id,
                "GET",
                TimingContext::new(),
                in_scope,
                &self.ctx,
            )
            .await);
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
                .send(ProxyToUi::ResponseReceived(request_id, response_data))
                .await;
        }

        let ui_tx_clone = self.ctx.ui_tx.clone();
        let host_owned = host.to_string();
        let max_ws_frame_bytes = self.ctx.limits.max_ws_frame_bytes;

        self.ctx.spawn(async move {
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
                    tracing::debug!("WebSocket client upgrade failed for {}: {}", host_owned, e);
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
                max_ws_frame_bytes,
            )
            .await;
        });

        Ok(
            crate::proxy::build_response(101, &resp_headers, Full::new(Bytes::new()), true)
                .unwrap_or_else(|error| crate::proxy::bad_gateway(&error.to_string())),
        )
    }
}
