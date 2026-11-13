use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use http_body::Frame;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Incoming;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::sync::{Mutex, OnceCell};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::sync::PollSender;
use tracing::{debug, error, warn};

use crate::channel::ProxyToUi;
use crate::http::models::{
    GrpcDirection, GrpcMessage, HttpVersion, RequestData, RequestId, ResponseData,
};
use crate::proxy::intercept::InterceptDecision;
use crate::proxy::{ProxyContext, TimingContext};
use crate::rules;
use crate::tls::cert_gen;

pub async fn handle_tunnel(
    upgraded: hyper::upgrade::Upgraded,
    host: String,
    port: u16,
    ctx: ProxyContext,
) {
    if let Err(e) = run_tunnel(upgraded, &host, port, ctx).await {
        debug!("Tunnel to {}:{} ended: {}", host, port, e);
    }
}

async fn run_tunnel(
    upgraded: hyper::upgrade::Upgraded,
    host: &str,
    port: u16,
    ctx: ProxyContext,
) -> anyhow::Result<()> {
    let certified_key = ctx.cert_cache.get_or_generate(host).await?;
    let server_config = cert_gen::build_server_config(certified_key)?;
    let acceptor = TlsAcceptor::from(server_config);

    let client_stream = TokioIo::new(upgraded);
    let tls_stream = crate::proxy::io_timeout(acceptor.accept(client_stream)).await?;

    let is_h2 = tls_stream
        .get_ref()
        .1
        .alpn_protocol()
        .is_some_and(|p| p == b"h2");

    if is_h2 {
        debug!("Client negotiated h2 for {}:{}", host, port);
        run_h2_tunnel(tls_stream, host, port, ctx).await
    } else {
        run_h1_tunnel(tls_stream, host, port, ctx).await
    }
}

async fn run_h1_tunnel(
    tls_stream: tokio_rustls::server::TlsStream<TokioIo<hyper::upgrade::Upgraded>>,
    host: &str,
    port: u16,
    ctx: ProxyContext,
) -> anyhow::Result<()> {
    let host = host.to_string();

    let svc = service_fn(move |req: Request<Incoming>| {
        let host = host.clone();
        let ctx = ctx.clone();
        async move { handle_inner_request(req, &host, port, &ctx).await }
    });

    ServerBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(crate::proxy::IO_TIMEOUT)
        .serve_connection(TokioIo::new(tls_stream), svc)
        .with_upgrades()
        .await?;

    Ok(())
}

pub(crate) fn is_websocket_upgrade(req: &Request<Incoming>) -> bool {
    req.headers()
        .get(hyper::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

async fn handle_inner_request(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ctx: &ProxyContext,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    if is_websocket_upgrade(&req) {
        return handle_websocket_upgrade(req, host, port, ctx).await;
    }

    handle_http_request(req, host, port, ctx).await
}

async fn handle_websocket_upgrade(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ctx: &ProxyContext,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    let in_scope = ctx.scope.is_in_scope(host);
    let request_id = RequestId::next();

    let headers = crate::http::models::extract_headers(req.headers());

    let full_uri = format!(
        "wss://{}{}",
        crate::proxy::format_authority(host, port, 443),
        req.uri()
            .path_and_query()
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
        is_tls: true,
        is_grpc: false,
        timestamp: std::time::SystemTime::now(),
    };

    if in_scope {
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::RequestCaptured(request_data))
            .await;
    }

    let client_config = ctx.tls_client_config.clone();

    let tcp = match crate::proxy::connect_tcp(host, port).await {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "WebSocket: failed to connect to upstream {}:{}: {}",
                host, port, e
            );
            ctx.report_error(request_id, format!("Connection failed: {e}"))
                .await;
            return Ok(crate::proxy::bad_gateway(&format!(
                "Connection failed: {}",
                e
            )));
        }
    };

    let server_name = crate::tls::server_name_or_localhost(host);
    let connector = TlsConnector::from(client_config);
    let tls_stream = match crate::proxy::io_timeout(connector.connect(server_name, tcp)).await {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "WebSocket: TLS handshake failed with {}:{}: {}",
                host, port, e
            );
            ctx.report_error(request_id, format!("TLS handshake failed: {e}"))
                .await;
            return Ok(crate::proxy::bad_gateway(&format!(
                "TLS handshake failed: {}",
                e
            )));
        }
    };

    let io = TokioIo::new(tls_stream);
    let (mut sender, conn) = match ClientBuilder::new()
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

    ctx.spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            debug!("WebSocket upstream connection ended: {}", e);
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

    let upstream_resp = match crate::proxy::io_timeout(sender.send_request(upstream_req)).await {
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
            ctx,
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
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::ResponseReceived(request_id, response_data))
            .await;
    }

    let ui_tx_clone = ctx.ui_tx.clone();
    let host_owned = host.to_string();
    let max_ws_frame_bytes = ctx.limits.max_ws_frame_bytes;

    ctx.spawn(async move {
        let upstream_upgraded = match hyper::upgrade::on(upstream_resp).await {
            Ok(u) => u,
            Err(e) => {
                debug!(
                    "WebSocket upstream upgrade failed for {}: {}",
                    host_owned, e
                );
                return;
            }
        };
        let client_upgraded = match hyper::upgrade::on(client_req_for_upgrade).await {
            Ok(u) => u,
            Err(e) => {
                debug!("WebSocket client upgrade failed for {}: {}", host_owned, e);
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

async fn handle_http_request(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ctx: &ProxyContext,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    let in_scope = ctx.scope.is_in_scope(host);
    let request_id = RequestId::next();
    let mut timing = TimingContext::new();

    let method = req.method().clone();
    let version = req.version().into();

    let headers = crate::http::models::extract_headers(req.headers());

    let (parts, body) = req.into_parts();
    let body_bytes = match crate::proxy::io_timeout(
        http_body_util::Limited::new(body, ctx.limits.max_body_bytes).collect(),
    )
    .await
    {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            warn!("Failed to read request body: {}", e);
            if e.root_cause().is::<http_body_util::LengthLimitError>() {
                return Ok(crate::proxy::payload_too_large());
            }
            return Ok(crate::proxy::bad_gateway(&format!(
                "Failed to read request body: {}",
                e
            )));
        }
    };

    let full_uri = format!(
        "https://{}{}",
        crate::proxy::format_authority(host, port, 443),
        parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
    );

    let mut request_data = RequestData {
        id: request_id,
        method: method.to_string(),
        uri: full_uri,
        host: host.to_string(),
        version,
        headers: headers.clone(),
        body: body_bytes.clone(),
        is_tls: true,
        is_grpc: false,
        timestamp: std::time::SystemTime::now(),
    };

    if in_scope {
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::RequestCaptured(request_data.clone()))
            .await;

        let decision = match ctx
            .intercept
            .intercept_request(&request_data, &ctx.ui_tx)
            .await
        {
            Ok(decision) => decision,
            Err(error) => return Ok(crate::proxy::service_unavailable(&error.to_string())),
        };
        if let Some(rx) = decision {
            match rx.await {
                Ok(InterceptDecision::Drop) => {
                    return Ok(hyper::Response::builder()
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
    }

    if let Err(error) = rules::apply_request_rules(
        &ctx.rules,
        &mut request_data.uri,
        &mut request_data.headers,
        &mut request_data.body,
        ctx.limits.max_body_bytes,
    ) {
        ctx.report_error(request_id, error.to_string()).await;
        return Ok(crate::proxy::bad_gateway(&error.to_string()));
    }

    let fallback_authority = request_data
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map_or(request_data.host.as_str(), |(_, value)| value.as_str());
    let target =
        match crate::proxy::resolve_upstream_target(&request_data.uri, fallback_authority, port) {
            Ok(target) => target,
            Err(error) => return Ok(crate::proxy::bad_gateway(&error.to_string())),
        };

    let client_config = ctx.tls_client_config.clone();

    let tcp = match crate::proxy::connect_tcp(&target.host, target.port).await {
        Ok(s) => {
            timing.tcp_connected = Some(Instant::now());
            s
        }
        Err(e) => {
            warn!(
                "Failed to connect to upstream {}:{}: {}",
                target.host, target.port, e
            );
            ctx.report_error(request_id, format!("Connection failed: {e}"))
                .await;
            return Ok(crate::proxy::bad_gateway(&format!(
                "Connection failed: {}",
                e
            )));
        }
    };

    if !target.uses_tls(request_data.is_tls) {
        return Ok(crate::proxy::forward_h1(
            TokioIo::new(tcp),
            request_data.version.into(),
            &target.path_and_query,
            &request_data,
            timing,
            in_scope,
            ctx,
        )
        .await);
    }
    let server_name = crate::tls::server_name_or_localhost(&target.host);
    let connector = TlsConnector::from(client_config);
    let tls_stream = match crate::proxy::io_timeout(connector.connect(server_name, tcp)).await {
        Ok(s) => {
            timing.tls_done = Some(Instant::now());
            s
        }
        Err(e) => {
            warn!(
                "TLS handshake failed with {}:{}: {}",
                target.host, target.port, e
            );
            ctx.report_error(request_id, format!("TLS handshake failed: {e}"))
                .await;
            return Ok(crate::proxy::bad_gateway(&format!(
                "TLS handshake failed: {}",
                e
            )));
        }
    };

    let io = TokioIo::new(tls_stream);
    Ok(crate::proxy::forward_h1(
        io,
        request_data.version.into(),
        &target.path_and_query,
        &request_data,
        timing,
        in_scope,
        ctx,
    )
    .await)
}

fn h2_bad_gateway(msg: &str) -> hyper::Response<H2RespBody> {
    hyper::Response::builder()
        .status(502)
        .body(H2RespBody::Buffered(H2Body {
            data: Some(Bytes::from(msg.to_string())),
            trailers: None,
        }))
        .unwrap()
}

fn is_grpc_request(req: &Request<Incoming>) -> bool {
    req.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/grpc"))
}

struct H2Body {
    data: Option<Bytes>,
    trailers: Option<http::HeaderMap>,
}

impl http_body::Body for H2Body {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let this = self.get_mut();
        if let Some(data) = this.data.take() {
            return Poll::Ready(Some(Ok(Frame::data(data))));
        }
        if let Some(trailers) = this.trailers.take() {
            return Poll::Ready(Some(Ok(Frame::trailers(trailers))));
        }
        Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.data.as_ref().is_none_or(Bytes::is_empty) && self.trailers.is_none()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(self.data.as_ref().map_or(0, |data| data.len() as u64))
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;
struct GrpcTeeBody {
    inner: Incoming,
    request_id: RequestId,
    direction: GrpcDirection,
    ui_tx: PollSender<ProxyToUi>,
    events: VecDeque<ProxyToUi>,
    pending_frame: Option<Frame<Bytes>>,
    buffer: BytesMut,
    in_scope: bool,
    max_message_bytes: usize,
    capture_enabled: bool,
    idle: Pin<Box<tokio::time::Sleep>>,
}

impl GrpcTeeBody {
    fn new(
        inner: Incoming,
        request_id: RequestId,
        direction: GrpcDirection,
        in_scope: bool,
        ctx: &ProxyContext,
    ) -> Self {
        Self {
            inner,
            request_id,
            direction,
            ui_tx: PollSender::new(ctx.ui_tx.clone()),
            events: VecDeque::new(),
            pending_frame: None,
            buffer: BytesMut::new(),
            in_scope,
            max_message_bytes: ctx.limits.max_body_bytes,
            capture_enabled: true,
            idle: Box::pin(tokio::time::sleep(crate::proxy::IO_TIMEOUT)),
        }
    }
}

impl http_body::Body for GrpcTeeBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        while !this.events.is_empty() {
            match this.ui_tx.poll_reserve(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => {
                    this.events.clear();
                    this.in_scope = false;
                    break;
                }
                Poll::Ready(Ok(())) => {
                    let event = this.events.pop_front().expect("queue is not empty");
                    let _ = this.ui_tx.send_item(event);
                }
            }
        }
        if let Some(frame) = this.pending_frame.take() {
            return Poll::Ready(Some(Ok(frame)));
        }
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                this.idle
                    .as_mut()
                    .reset(tokio::time::Instant::now() + crate::proxy::IO_TIMEOUT);
                if this.in_scope {
                    if this.capture_enabled
                        && let Some(data) = frame.data_ref()
                    {
                        this.buffer.extend_from_slice(data);
                        this.capture_enabled = extract_grpc_messages(
                            &mut this.buffer,
                            this.request_id,
                            this.direction,
                            &mut this.events,
                            this.max_message_bytes,
                        );
                        if !this.capture_enabled && this.direction == GrpcDirection::ClientToServer
                        {
                            this.events
                                .push_back(ProxyToUi::RequestCaptureTruncated(this.request_id));
                        }
                    }
                    if this.direction == GrpcDirection::ServerToClient
                        && let Some(trailers) = frame.trailers_ref()
                    {
                        let pairs = crate::http::models::extract_trailers(Some(trailers));
                        this.events
                            .push_back(ProxyToUi::GrpcTrailers(this.request_id, pairs));
                    }
                }
                // Flush associated events before exposing even the final data/trailer
                // frame, so backpressure cannot silently produce a partial replay.
                this.pending_frame = Some(frame);
                Pin::new(this).poll_frame(cx)
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(Box::new(error)))),
            Poll::Ready(None) => {
                if this.in_scope
                    && this.direction == GrpcDirection::ClientToServer
                    && !this.buffer.is_empty()
                {
                    this.buffer.clear();
                    this.events
                        .push_back(ProxyToUi::RequestCaptureTruncated(this.request_id));
                    Pin::new(this).poll_frame(cx)
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => {
                if std::future::Future::poll(this.idle.as_mut(), cx).is_ready() {
                    Poll::Ready(Some(Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "gRPC stream idle timeout",
                    )))))
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
            && self.pending_frame.is_none()
            && self.events.is_empty()
            && self.buffer.is_empty()
    }
}

fn extract_grpc_messages(
    buffer: &mut BytesMut,
    request_id: RequestId,
    direction: GrpcDirection,
    events: &mut VecDeque<ProxyToUi>,
    max_message_bytes: usize,
) -> bool {
    while buffer.len() >= 5 {
        let compressed = buffer[0] != 0;
        let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
        if len > max_message_bytes {
            buffer.clear();
            return false;
        }
        let Some(frame_len) = 5usize.checked_add(len) else {
            buffer.clear();
            return false;
        };
        if buffer.len() < frame_len {
            break;
        }
        let frame_data = buffer.split_to(frame_len);
        let payload = Bytes::copy_from_slice(&frame_data[5..]);

        let msg = GrpcMessage {
            direction,
            compressed,
            payload,
            timestamp: std::time::SystemTime::now(),
        };
        events.push_back(ProxyToUi::GrpcFrame(request_id, msg));
    }
    true
}

enum H2RespBody {
    Buffered(H2Body),
    Streaming(GrpcTeeBody),
}

impl http_body::Body for H2RespBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        match self.get_mut() {
            H2RespBody::Buffered(body) => match Pin::new(body).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(e))) => match e {},
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            H2RespBody::Streaming(body) => Pin::new(body).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            H2RespBody::Buffered(b) => b.is_end_stream(),
            H2RespBody::Streaming(b) => b.is_end_stream(),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            H2RespBody::Buffered(body) => body.size_hint(),
            H2RespBody::Streaming(body) => body.size_hint(),
        }
    }
}

async fn run_h2_tunnel(
    tls_stream: tokio_rustls::server::TlsStream<TokioIo<hyper::upgrade::Upgraded>>,
    host: &str,
    port: u16,
    ctx: ProxyContext,
) -> anyhow::Result<()> {
    // Negotiate each side independently. An h2 browser connection does not imply
    // that the origin supports h2. Initialize inside the service so connection
    // failures produce a 502 instead of abruptly closing the browser's tunnel.
    let upstream = Arc::new(OnceCell::new());
    let host = host.to_string();
    let svc = service_fn(move |req: Request<Incoming>| {
        let host = host.clone();
        let ctx = ctx.clone();
        let upstream = upstream.clone();
        async move {
            let sender = match upstream
                .get_or_try_init(|| establish_h2_upstream(&host, port, &ctx))
                .await
            {
                Ok(sender) => sender.clone(),
                Err(error) => {
                    warn!("Failed to establish upstream to {host}:{port}: {error}");
                    return Ok(h2_bad_gateway(&format!(
                        "Upstream connection failed: {error}"
                    )));
                }
            };
            handle_h2_request(req, &host, port, &ctx, sender).await
        }
    });

    hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(tls_stream), svc)
        .await?;

    Ok(())
}

#[derive(Clone)]
enum H2Upstream {
    Http2(hyper::client::conn::http2::SendRequest<H2RespBody>),
    Http1(Arc<Mutex<hyper::client::conn::http1::SendRequest<H2RespBody>>>),
}

async fn establish_h1_upstream(
    stream: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    ctx: &ProxyContext,
) -> anyhow::Result<hyper::client::conn::http1::SendRequest<H2RespBody>> {
    let (sender, conn) = ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(TokioIo::new(stream))
        .await?;
    ctx.spawn(async move {
        if let Err(error) = conn.await {
            debug!("HTTP/1 upstream connection ended: {error}");
        }
    });
    Ok(sender)
}

async fn establish_h2_upstream(
    host: &str,
    port: u16,
    ctx: &ProxyContext,
) -> anyhow::Result<H2Upstream> {
    let client_config = ctx.tls_h2_client_config.clone();

    let tcp = crate::proxy::connect_tcp(host, port).await?;

    let server_name = crate::tls::server_name_or_localhost(host);
    let connector = TlsConnector::from(client_config);
    let tls_stream = crate::proxy::io_timeout(connector.connect(server_name, tcp)).await?;

    if tls_stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        debug!("Upstream {host}:{port} did not negotiate h2; using HTTP/1.1");
        let sender = establish_h1_upstream(tls_stream, ctx).await?;
        return Ok(H2Upstream::Http1(Arc::new(Mutex::new(sender))));
    }

    let io = TokioIo::new(tls_stream);
    let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(io)
        .await?;

    ctx.spawn(async move {
        if let Err(e) = conn.await {
            debug!("H2 upstream connection ended: {}", e);
        }
    });

    Ok(H2Upstream::Http2(sender))
}

async fn handle_h2_request(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ctx: &ProxyContext,
    upstream: H2Upstream,
) -> Result<hyper::Response<H2RespBody>, Infallible> {
    let in_scope = ctx.scope.is_in_scope(host);
    let request_id = RequestId::next();
    let timing = TimingContext::new();

    let is_grpc = is_grpc_request(&req);
    if is_grpc && matches!(&upstream, H2Upstream::Http1(_)) {
        return Ok(h2_bad_gateway("gRPC requires an HTTP/2 upstream"));
    }
    let method = req.method().clone();

    let headers = crate::http::models::extract_headers(req.headers());

    let (parts, body) = req.into_parts();
    let mut streaming_body = None;
    let body_bytes = if is_grpc {
        streaming_body = Some(body);
        Bytes::new()
    } else {
        match crate::proxy::io_timeout(
            http_body_util::Limited::new(body, ctx.limits.max_body_bytes).collect(),
        )
        .await
        {
            Ok(b) => b.to_bytes(),
            Err(e) => {
                warn!("Failed to read h2 request body: {}", e);
                return Ok(h2_bad_gateway(&format!(
                    "Failed to read request body: {}",
                    e
                )));
            }
        }
    };

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let full_uri = format!(
        "https://{}{}",
        crate::proxy::format_authority(host, port, 443),
        path_and_query
    );

    let mut request_data = RequestData {
        id: request_id,
        method: method.to_string(),
        uri: full_uri,
        host: host.to_string(),
        version: HttpVersion::Http2,
        headers: headers.clone(),
        body: body_bytes.clone(),
        is_tls: true,
        is_grpc,
        timestamp: std::time::SystemTime::now(),
    };

    if in_scope {
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::RequestCaptured(request_data.clone()))
            .await;

        let decision = if !is_grpc {
            match ctx
                .intercept
                .intercept_request(&request_data, &ctx.ui_tx)
                .await
            {
                Ok(decision) => decision,
                Err(error) => return Ok(h2_bad_gateway(&error.to_string())),
            }
        } else {
            None
        };
        if let Some(rx) = decision {
            match rx.await {
                Ok(InterceptDecision::Drop) => {
                    return Ok(hyper::Response::builder()
                        .status(503)
                        .body(H2RespBody::Buffered(H2Body {
                            data: Some(Bytes::from("Request dropped by interceptor")),
                            trailers: None,
                        }))
                        .unwrap());
                }
                Ok(InterceptDecision::ForwardEdited(edited)) => {
                    request_data = edited;
                }
                Ok(InterceptDecision::Forward) => {}
                Err(_) => return Ok(h2_bad_gateway("Interceptor disconnected")),
            }
        }
    }

    if !is_grpc
        && let Err(error) = rules::apply_request_rules(
            &ctx.rules,
            &mut request_data.uri,
            &mut request_data.headers,
            &mut request_data.body,
            ctx.limits.max_body_bytes,
        )
    {
        ctx.report_error(request_id, error.to_string()).await;
        return Ok(h2_bad_gateway(&error.to_string()));
    }

    let fallback_authority = request_data
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map_or(host, |(_, value)| value.as_str());
    if request_data.body.len() > ctx.limits.max_body_bytes {
        return Ok(h2_bad_gateway(
            "Rewritten request body exceeds configured limit",
        ));
    }
    let target =
        match crate::proxy::resolve_upstream_target(&request_data.uri, fallback_authority, port) {
            Ok(target) => target,
            Err(error) => return Ok(h2_bad_gateway(&error.to_string())),
        };
    if !target.host.eq_ignore_ascii_case(host) || target.port != port || !target.uses_tls(true) {
        return Ok(h2_bad_gateway(
            "Changing authority is not supported on an existing HTTP/2 tunnel",
        ));
    }
    let authority = crate::proxy::format_authority(host, port, 443);
    let upstream_uri = format!("https://{}{}", authority, target.path_and_query);

    let upstream_req = match crate::proxy::build_forwarding_request(
        &request_data.method,
        &upstream_uri,
        &request_data.headers,
        request_data.body.clone(),
    ) {
        Ok(request) => request,
        Err(error) => return Ok(h2_bad_gateway(&format!("Invalid edited request: {error}"))),
    };
    let (mut upstream_parts, _) = upstream_req.into_parts();
    let upstream_body = if let Some(body) = streaming_body {
        upstream_parts
            .headers
            .insert(http::header::TE, http::HeaderValue::from_static("trailers"));
        H2RespBody::Streaming(GrpcTeeBody::new(
            body,
            request_id,
            GrpcDirection::ClientToServer,
            in_scope,
            ctx,
        ))
    } else {
        H2RespBody::Buffered(H2Body {
            data: Some(request_data.body.clone()),
            trailers: None,
        })
    };
    let mut upstream_req = Request::from_parts(upstream_parts, upstream_body);

    let response = crate::proxy::io_timeout(async {
        match upstream {
            H2Upstream::Http2(mut sender) => sender
                .send_request(upstream_req)
                .await
                .map_err(anyhow::Error::from),
            H2Upstream::Http1(sender) => {
                *upstream_req.version_mut() = hyper::Version::HTTP_11;
                *upstream_req.uri_mut() = target.path_and_query.parse::<http::Uri>()?;
                // HTTP/2 clients normally use :authority rather than Host.
                upstream_req
                    .headers_mut()
                    .insert(http::header::HOST, authority.parse()?);
                let mut sender = sender.lock().await;
                if sender.ready().await.is_err() {
                    // Reconnect only before sending, never retry a possibly
                    // processed request (especially POSTs).
                    let tcp = crate::proxy::connect_tcp(host, port).await?;
                    let mut config = (*ctx.tls_client_config).clone();
                    config.alpn_protocols = vec![b"http/1.1".to_vec()];
                    let connector = TlsConnector::from(Arc::new(config));
                    let stream = connector
                        .connect(crate::tls::server_name_or_localhost(host), tcp)
                        .await?;
                    *sender = establish_h1_upstream(stream, ctx).await?;
                }
                sender
                    .send_request(upstream_req)
                    .await
                    .map_err(anyhow::Error::from)
            }
        }
    })
    .await;
    let upstream_resp = match response {
        Ok(resp) => resp,
        Err(e) => {
            warn!("H2 upstream request to {}:{} failed: {}", host, port, e);
            ctx.report_error(request_id, format!("Request failed: {e}"))
                .await;
            return Ok(h2_bad_gateway(&format!("Request failed: {}", e)));
        }
    };

    let resp_status = upstream_resp.status().as_u16();
    let resp_headers = crate::http::models::extract_headers(upstream_resp.headers());

    if is_grpc {
        return Ok(build_grpc_response(
            upstream_resp,
            resp_status,
            resp_headers,
            timing,
            request_id,
            in_scope,
            ctx,
        )
        .await);
    }

    build_buffered_response(
        upstream_resp,
        resp_status,
        resp_headers,
        timing,
        &request_data,
        in_scope,
        ctx,
    )
    .await
}

async fn build_grpc_response(
    upstream_resp: hyper::Response<Incoming>,
    resp_status: u16,
    resp_headers: Vec<(String, String)>,
    timing: TimingContext,
    request_id: RequestId,
    in_scope: bool,
    ctx: &ProxyContext,
) -> hyper::Response<H2RespBody> {
    let duration = timing.start.elapsed();
    let mut timing_data = timing.finish(None);
    timing_data.time_to_first_byte = Some(duration);

    if in_scope {
        let response_data = ResponseData {
            status: resp_status,
            reason: crate::http::models::status_reason(resp_status).to_string(),
            version: HttpVersion::Http2,
            headers: resp_headers.clone(),
            body: Bytes::new(),
            trailers: Vec::new(),
            duration,
            timing: Some(timing_data),
        };
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::ResponseReceived(request_id, response_data))
            .await;
    }

    let (_, resp_body_incoming) = upstream_resp.into_parts();
    let resp_tee = GrpcTeeBody::new(
        resp_body_incoming,
        request_id,
        GrpcDirection::ServerToClient,
        in_scope,
        ctx,
    );
    crate::proxy::build_response(
        resp_status,
        &resp_headers,
        H2RespBody::Streaming(resp_tee),
        false,
    )
    .unwrap_or_else(|error| h2_bad_gateway(&format!("Invalid upstream response: {error}")))
}

async fn build_buffered_response(
    upstream_resp: hyper::Response<Incoming>,
    resp_status: u16,
    resp_headers: Vec<(String, String)>,
    mut timing: TimingContext,
    request: &RequestData,
    in_scope: bool,
    ctx: &ProxyContext,
) -> Result<hyper::Response<H2RespBody>, Infallible> {
    let request_id = request.id;
    let response_version = upstream_resp.version().into();
    timing.first_byte = Some(Instant::now());

    let (_, resp_body_incoming) = upstream_resp.into_parts();
    let body_start = Instant::now();
    let collected = match crate::proxy::io_timeout(
        http_body_util::Limited::new(resp_body_incoming, ctx.limits.max_body_bytes).collect(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read h2 upstream response body: {}", e);
            ctx.report_error(request_id, format!("Response body error: {e}"))
                .await;
            return Ok(h2_bad_gateway(&format!("Response body error: {}", e)));
        }
    };
    let content_transfer = body_start.elapsed();

    let resp_trailers = collected.trailers().cloned();
    let mut resp_body = collected.to_bytes();
    let duration = timing.start.elapsed();

    let trailer_pairs = crate::http::models::extract_trailers(resp_trailers.as_ref());

    let mut resp_headers_mut = resp_headers;
    if let Err(error) = rules::apply_response_rules(
        &ctx.rules,
        &mut resp_headers_mut,
        &mut resp_body,
        ctx.limits.max_body_bytes,
    ) {
        ctx.report_error(request_id, error.to_string()).await;
        return Ok(h2_bad_gateway(&error.to_string()));
    }
    if resp_body.len() > ctx.limits.max_body_bytes {
        return Ok(h2_bad_gateway(
            "Rewritten response body exceeds configured limit",
        ));
    }
    crate::proxy::normalize_response_headers(
        &mut resp_headers_mut,
        resp_body.len(),
        &request.method,
        resp_status,
    );
    let response = match crate::proxy::build_response(
        resp_status,
        &resp_headers_mut,
        H2RespBody::Buffered(H2Body {
            data: Some(resp_body.clone()),
            trailers: resp_trailers,
        }),
        false,
    ) {
        Ok(response) => response,
        Err(error) => {
            let _ = ctx
                .ui_tx
                .send(ProxyToUi::RequestError(
                    request_id,
                    format!("Invalid rewritten response: {error}"),
                ))
                .await;
            return Ok(h2_bad_gateway(&format!(
                "Invalid rewritten response: {error}"
            )));
        }
    };

    let timing_data = timing.finish(Some(content_transfer));

    let response_data = ResponseData {
        status: resp_status,
        reason: crate::http::models::status_reason(resp_status).to_string(),
        version: response_version,
        headers: resp_headers_mut.clone(),
        body: resp_body.clone(),
        trailers: trailer_pairs,
        duration,
        timing: Some(timing_data),
    };

    if in_scope {
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::ResponseReceived(request_id, response_data))
            .await;
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn h2_rewrite(scope: crate::rules::RuleScope, replacement: &str) {
        let (ctx, mut events) = crate::test_support::context(crate::proxy::ProxyLimits::default());
        let mut rule = crate::rules::Rule::new("rewrite".into());
        rule.scope = scope;
        rule.set_pattern("foo".into()).unwrap();
        rule.replacement = replacement.into();
        ctx.rules.write().push(rule);
        let (upstream_io, server_io) = tokio::io::duplex(4096);
        let expected = replacement.to_string();
        let upstream_server = tokio::spawn(async move {
            let service = service_fn(move |request: Request<Incoming>| {
                let expected = expected.clone();
                async move {
                    let body = request.into_body().collect().await.unwrap().to_bytes();
                    if scope == crate::rules::RuleScope::Body {
                        assert_eq!(body.as_ref(), expected.as_bytes());
                    }
                    Ok::<_, Infallible>(
                        hyper::Response::builder()
                            .header("x-test", "foo")
                            .header("content-length", "3")
                            .body(Full::new(Bytes::from_static(b"foo")))
                            .unwrap(),
                    )
                }
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(server_io), service)
                .await;
        });
        let (upstream, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .handshake(TokioIo::new(upstream_io))
            .await
            .unwrap();
        let upstream_driver = tokio::spawn(connection);
        let (client_io, proxy_io) = tokio::io::duplex(4096);
        let proxy_server = tokio::spawn(async move {
            let service = service_fn(move |request| {
                let ctx = ctx.clone();
                let upstream = upstream.clone();
                async move {
                    handle_h2_request(
                        request,
                        "example.com",
                        443,
                        &ctx,
                        H2Upstream::Http2(upstream),
                    )
                    .await
                }
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(proxy_io), service)
                .await;
        });
        let (mut client, connection) =
            hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                .handshake(TokioIo::new(client_io))
                .await
                .unwrap();
        let client_driver = tokio::spawn(connection);
        let request = Request::builder()
            .method("POST")
            .uri("https://example.com/")
            .header("content-length", "3")
            .body(Full::new(Bytes::from_static(b"foo")))
            .unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.send_request(request),
        )
        .await
        .unwrap()
        .unwrap();
        if scope == crate::rules::RuleScope::Body {
            assert_eq!(response.status(), 200);
            assert_eq!(
                response.headers()["content-length"],
                replacement.len().to_string()
            );
            assert_eq!(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .as_ref(),
                replacement.as_bytes()
            );
        } else {
            assert_eq!(response.status(), 502);
            assert!(matches!(
                events.recv().await,
                Some(ProxyToUi::RequestCaptured(_))
            ));
            assert!(matches!(
                events.recv().await,
                Some(ProxyToUi::RequestError(_, _))
            ));
        }
        client_driver.abort();
        proxy_server.abort();
        upstream_driver.abort();
        upstream_server.abort();
    }

    #[tokio::test]
    async fn h2_rewrites_update_framing_for_growing_and_shrinking_bodies() {
        h2_rewrite(crate::rules::RuleScope::Body, "longer").await;
        h2_rewrite(crate::rules::RuleScope::Body, "x").await;
    }

    #[tokio::test]
    async fn invalid_rewritten_h2_headers_return_502_without_panicking() {
        h2_rewrite(crate::rules::RuleScope::Headers, "bad\r\nvalue").await;
    }
}
