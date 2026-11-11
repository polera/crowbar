pub mod handler;
#[cfg(test)]
mod integration_tests;
pub mod intercept;
pub mod repeater;
pub mod scope;
pub mod server;
pub mod tunnel;
pub mod ws_relay;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response};
use tokio::sync::mpsc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper_util::rt::TokioIo;
use tracing::debug;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn io_timeout<T, E>(
    future: impl std::future::Future<Output = Result<T, E>>,
) -> anyhow::Result<T>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    use anyhow::Context;
    tokio::time::timeout(IO_TIMEOUT, future)
        .await
        .context("network operation timed out")?
        .map_err(|error| anyhow::Error::from_boxed(error.into()))
}

use crate::channel::ProxyToUi;
use crate::http::models::{RequestId, ResponseData, TimingData};
use crate::proxy::intercept::InterceptState;
use crate::proxy::scope::Scope;
use crate::rules::{self, SharedRules};
use crate::tls::cert_cache::CertCache;

pub(crate) struct TimingContext {
    pub start: Instant,
    pub tcp_connected: Option<Instant>,
    pub tls_done: Option<Instant>,
    pub http_handshake_done: Option<Instant>,
    pub first_byte: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
pub struct ProxyLimits {
    pub max_body_bytes: usize,
    pub max_ws_frame_bytes: usize,
    pub max_connections: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UpstreamTarget {
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
    pub scheme: Option<http::uri::Scheme>,
}

impl UpstreamTarget {
    pub(crate) fn uses_tls(&self, fallback: bool) -> bool {
        self.scheme.as_ref().map_or(fallback, |scheme| {
            matches!(scheme.as_str(), "https" | "wss")
        })
    }
}

pub(crate) fn resolve_upstream_target(
    uri: &str,
    fallback_authority: &str,
    default_port: u16,
) -> anyhow::Result<UpstreamTarget> {
    let parsed: http::Uri = uri
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid request URI {uri:?}: {error}"))?;
    let fallback;
    let authority = if let Some(authority) = parsed.authority() {
        authority
    } else {
        fallback = fallback_authority
            .parse::<http::uri::Authority>()
            .map_err(|error| {
                anyhow::anyhow!("invalid authority {fallback_authority:?}: {error}")
            })?;
        &fallback
    };
    let port = match parsed.scheme_str() {
        Some("http" | "ws") => 80,
        Some("https" | "wss") => 443,
        None => default_port,
        Some(scheme) => anyhow::bail!("Unsupported upstream scheme {scheme:?}"),
    };
    Ok(UpstreamTarget {
        host: authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port: authority.port_u16().unwrap_or(port),
        path_and_query: parsed
            .path_and_query()
            .map_or_else(|| "/".to_string(), ToString::to_string),
        scheme: parsed.scheme().cloned(),
    })
}

pub(crate) async fn connect_tcp(host: &str, port: u16) -> anyhow::Result<tokio::net::TcpStream> {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connection to {host}:{port} timed out"))?
    .map_err(Into::into)
}

pub(crate) fn format_authority(host: &str, port: u16, default_port: u16) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port {
        host
    } else {
        format!("{host}:{port}")
    }
}

impl Default for ProxyLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 10 * 1024 * 1024,
            max_ws_frame_bytes: 16 * 1024 * 1024,
            max_connections: 128,
        }
    }
}

impl TimingContext {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            tcp_connected: None,
            tls_done: None,
            http_handshake_done: None,
            first_byte: None,
        }
    }

    pub fn finish(&self, content_transfer: Option<Duration>) -> TimingData {
        let tcp_connect = self.tcp_connected.map(|t| t - self.start);
        let tls_handshake = match (self.tcp_connected, self.tls_done) {
            (Some(tcp), Some(tls)) => Some(tls - tcp),
            _ => None,
        };
        let tls_or_tcp = self.tls_done.or(self.tcp_connected);
        let http_handshake = match (tls_or_tcp, self.http_handshake_done) {
            (Some(prev), Some(hs)) => Some(hs - prev),
            _ => None,
        };
        let send_start = self.http_handshake_done.or(tls_or_tcp).or(Some(self.start));
        let time_to_first_byte = match (send_start, self.first_byte) {
            (Some(s), Some(fb)) => Some(fb - s),
            _ => None,
        };
        TimingData {
            tcp_connect,
            tls_handshake,
            http_handshake,
            time_to_first_byte,
            content_transfer,
        }
    }
}

#[derive(Clone)]
pub struct ProxyContext {
    pub ui_tx: mpsc::Sender<ProxyToUi>,
    pub cert_cache: Arc<CertCache>,
    pub intercept: Arc<InterceptState>,
    pub scope: Arc<Scope>,
    pub rules: SharedRules,
    pub limits: ProxyLimits,
    pub(crate) tls_client_config: Arc<rustls::ClientConfig>,
    pub(crate) tls_h2_client_config: Arc<rustls::ClientConfig>,
    pub(crate) tasks: TaskTracker,
    pub(crate) cancel: CancellationToken,
    pub(crate) connection_permit: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
}

impl ProxyContext {
    pub(crate) async fn report_error(&self, id: RequestId, message: impl Into<String>) {
        let _ = self
            .ui_tx
            .send(ProxyToUi::RequestError(id, message.into()))
            .await;
    }
    pub fn new(
        ui_tx: mpsc::Sender<ProxyToUi>,
        cert_cache: Arc<CertCache>,
        intercept: Arc<InterceptState>,
        scope: Arc<Scope>,
        rules: SharedRules,
        limits: ProxyLimits,
    ) -> Self {
        Self {
            ui_tx,
            cert_cache,
            intercept,
            scope,
            rules,
            limits,
            tls_client_config: crate::tls::build_tls_client_config(),
            tls_h2_client_config: crate::tls::build_tls_h2_client_config(),
            tasks: TaskTracker::new(),
            cancel: CancellationToken::new(),
            connection_permit: None,
        }
    }

    /// Upgrades and upstream drivers remain owned by the server, not detached.
    pub(crate) fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        let cancel = self.cancel.clone();
        let permit = self.connection_permit.clone();
        self.tasks.spawn(async move {
            let _permit = permit;
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {},
                _ = future => {},
            }
        });
    }
}

pub(crate) fn build_forwarding_request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Bytes,
) -> Result<Request<Full<Bytes>>, http::Error> {
    let method: http::Method = method.parse()?;
    let uri: http::Uri = uri.parse()?;
    let mut builder = Request::builder().method(method).uri(uri);
    for (key, value) in end_to_end_headers(headers, false) {
        if key.eq_ignore_ascii_case("content-length") {
            continue;
        }
        builder = builder.header(key.as_str(), crate::http::models::header_value(value)?);
    }
    builder.body(Full::new(body))
}

pub(crate) fn end_to_end_headers(
    headers: &[(String, String)],
    preserve_upgrade: bool,
) -> impl Iterator<Item = &(String, String)> {
    let connection_tokens: HashSet<String> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();

    headers.iter().filter(move |(name, _)| {
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "proxy-authorization" | "proxy-authenticate" | "proxy-connection"
        ) {
            return false;
        }
        if preserve_upgrade && matches!(lower.as_str(), "connection" | "upgrade") {
            return true;
        }
        !matches!(
            lower.as_str(),
            "connection" | "keep-alive" | "te" | "trailer" | "transfer-encoding" | "upgrade"
        ) && !connection_tokens.contains(&lower)
    })
}

pub(crate) fn build_response<B>(
    status: u16,
    headers: &[(String, String)],
    body: B,
    preserve_upgrade: bool,
) -> Result<Response<B>, http::Error> {
    let mut builder = Response::builder().status(status);
    for (key, value) in end_to_end_headers(headers, preserve_upgrade) {
        let value = crate::http::models::header_value(value)?;
        builder = builder.header(key.as_str(), value);
    }
    builder.body(body)
}

pub(crate) fn normalize_response_headers(
    headers: &mut Vec<(String, String)>,
    body_len: usize,
    method: &str,
    status: u16,
) {
    // HEAD and 304 may advertise the representation's length without a body.
    if method.eq_ignore_ascii_case("HEAD") || status == 304 {
        return;
    }
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-length"));
    if status >= 200 && status != 204 {
        headers.push(("content-length".into(), body_len.to_string()));
    }
}

pub(crate) async fn process_h1_response(
    upstream_resp: Response<hyper::body::Incoming>,
    request_id: RequestId,
    request_method: &str,
    timing: TimingContext,
    in_scope: bool,
    ctx: &ProxyContext,
) -> Response<Full<Bytes>> {
    let resp_status = upstream_resp.status().as_u16();
    let resp_version = upstream_resp.version().into();
    let mut resp_headers = crate::http::models::extract_headers(upstream_resp.headers());

    let body_start = Instant::now();
    let mut resp_body = match io_timeout(
        http_body_util::Limited::new(upstream_resp.into_body(), ctx.limits.max_body_bytes)
            .collect(),
    )
    .await
    {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            ctx.report_error(request_id, format!("Response body rejected: {error}"))
                .await;
            return bad_gateway(&format!("Response body rejected: {error}"));
        }
    };
    let content_transfer = body_start.elapsed();
    let duration = timing.start.elapsed();

    rules::apply_response_rules(&ctx.rules, &mut resp_headers, &mut resp_body);
    if resp_body.len() > ctx.limits.max_body_bytes {
        return bad_gateway("Rewritten response body exceeds configured limit");
    }
    normalize_response_headers(
        &mut resp_headers,
        resp_body.len(),
        request_method,
        resp_status,
    );
    let response = match build_response(
        resp_status,
        &resp_headers,
        Full::new(resp_body.clone()),
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
            return bad_gateway(&format!("Invalid rewritten response: {error}"));
        }
    };

    let timing_data = timing.finish(Some(content_transfer));

    let response_data = ResponseData {
        status: resp_status,
        reason: crate::http::models::status_reason(resp_status).to_string(),
        version: resp_version,
        headers: resp_headers.clone(),
        body: resp_body.clone(),
        trailers: Vec::new(),
        duration,
        timing: Some(timing_data),
    };

    if in_scope {
        let _ = ctx
            .ui_tx
            .send(ProxyToUi::ResponseReceived(request_id, response_data))
            .await;
    }

    response
}

pub(crate) fn bad_gateway(msg: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(502)
        .body(Full::new(Bytes::from(msg.to_string())))
        .unwrap()
}

pub(crate) fn payload_too_large() -> Response<Full<Bytes>> {
    Response::builder()
        .status(http::StatusCode::PAYLOAD_TOO_LARGE)
        .body(Full::new(Bytes::from_static(
            b"Body exceeds configured limit",
        )))
        .expect("static 413 response is valid")
}

pub(crate) fn service_unavailable(message: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(503)
        .body(Full::new(Bytes::copy_from_slice(message.as_bytes())))
        .expect("static status is valid")
}

pub(crate) async fn forward_h1<IO>(
    io: TokioIo<IO>,
    version: hyper::Version,
    path_and_query: &str,
    request_data: &crate::http::models::RequestData,
    mut timing: TimingContext,
    in_scope: bool,
    ctx: &ProxyContext,
) -> Response<Full<Bytes>>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let request_id = request_data.id;
    if request_data.body.len() > ctx.limits.max_body_bytes {
        return payload_too_large();
    }

    let (mut sender, conn) = match ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            ctx.report_error(request_id, format!("HTTP handshake failed: {e}"))
                .await;
            return bad_gateway(&format!("HTTP handshake failed: {}", e));
        }
    };

    timing.http_handshake_done = Some(Instant::now());

    ctx.spawn(async move {
        if let Err(e) = conn.await {
            debug!("Upstream connection ended: {}", e);
        }
    });

    let mut upstream_req = match build_forwarding_request(
        &request_data.method,
        path_and_query,
        &request_data.headers,
        request_data.body.clone(),
    ) {
        Ok(request) => request,
        Err(error) => {
            ctx.report_error(request_id, format!("Invalid edited request: {error}"))
                .await;
            return bad_gateway(&format!("Invalid edited request: {error}"));
        }
    };
    *upstream_req.version_mut() = version;

    let upstream_resp = match io_timeout(sender.send_request(upstream_req)).await {
        Ok(resp) => resp,
        Err(e) => {
            ctx.report_error(request_id, format!("Request failed: {e}"))
                .await;
            return bad_gateway(&format!("Upstream request failed: {}", e));
        }
    };

    timing.first_byte = Some(Instant::now());

    process_h1_response(
        upstream_resp,
        request_id,
        &request_data.method,
        timing,
        in_scope,
        ctx,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn network_operations_have_a_deadline() {
        let operation = std::future::pending::<Result<(), std::io::Error>>();
        let start = tokio::time::Instant::now();
        assert!(io_timeout(operation).await.is_err());
        assert_eq!(start.elapsed(), IO_TIMEOUT);
    }

    #[test]
    fn non_ascii_header_octets_round_trip_without_placeholder_replacement() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-binary",
            http::HeaderValue::from_bytes(b"\xff\x80").unwrap(),
        );
        let stored = crate::http::models::extract_headers(&headers);
        let request = build_forwarding_request("GET", "/", &stored, Bytes::new()).unwrap();
        assert_eq!(request.headers()["x-binary"].as_bytes(), b"\xff\x80");
        assert!(
            build_response(200, &[("x-test".into(), "bad\r\nheader".into())], (), false).is_err()
        );
    }

    #[test]
    fn framing_is_recomputed_except_for_representation_metadata() {
        let headers = vec![("content-length".into(), "3".into())];
        let request =
            build_forwarding_request("POST", "/", &headers, Bytes::from_static(b"longer")).unwrap();
        assert!(!request.headers().contains_key("content-length"));
        for (method, status, expected) in [
            ("GET", 200, Some("6")),
            ("HEAD", 200, Some("3")),
            ("GET", 304, Some("3")),
            ("GET", 204, None),
        ] {
            let mut headers = headers.clone();
            normalize_response_headers(&mut headers, 6, method, status);
            assert_eq!(
                headers
                    .iter()
                    .find(|(name, _)| name == "content-length")
                    .map(|(_, value)| value.as_str()),
                expected
            );
        }
    }

    #[test]
    fn authority_resolution_uses_scheme_defaults_and_validates_only_needed_fallback() {
        let target = resolve_upstream_target("https://example.com/path", "", 80).unwrap();
        assert_eq!(target.port, 443);
        assert!(target.uses_tls(false));
        assert_eq!(format_authority("::1", 8443, 443), "[::1]:8443");
        assert!(resolve_upstream_target("ftp://example.com/", "", 80).is_err());
    }

    #[test]
    fn timing_context_finish_all_phases() {
        let start = Instant::now();
        let tcp = start + Duration::from_millis(10);
        let tls = tcp + Duration::from_millis(20);
        let hs = tls + Duration::from_millis(5);
        let fb = hs + Duration::from_millis(50);
        let content = Duration::from_millis(30);

        let ctx = TimingContext {
            start,
            tcp_connected: Some(tcp),
            tls_done: Some(tls),
            http_handshake_done: Some(hs),
            first_byte: Some(fb),
        };

        let timing = ctx.finish(Some(content));
        assert_eq!(timing.tcp_connect, Some(Duration::from_millis(10)));
        assert_eq!(timing.tls_handshake, Some(Duration::from_millis(20)));
        assert_eq!(timing.http_handshake, Some(Duration::from_millis(5)));
        assert_eq!(timing.time_to_first_byte, Some(Duration::from_millis(50)));
        assert_eq!(timing.content_transfer, Some(Duration::from_millis(30)));
    }

    #[test]
    fn timing_context_finish_no_tls() {
        let start = Instant::now();
        let tcp = start + Duration::from_millis(15);
        let hs = tcp + Duration::from_millis(3);
        let fb = hs + Duration::from_millis(40);

        let ctx = TimingContext {
            start,
            tcp_connected: Some(tcp),
            tls_done: None,
            http_handshake_done: Some(hs),
            first_byte: Some(fb),
        };

        let timing = ctx.finish(None);
        assert_eq!(timing.tcp_connect, Some(Duration::from_millis(15)));
        assert_eq!(timing.tls_handshake, None);
        // http_handshake should be relative to tcp (since no tls)
        assert_eq!(timing.http_handshake, Some(Duration::from_millis(3)));
        assert_eq!(timing.time_to_first_byte, Some(Duration::from_millis(40)));
        assert_eq!(timing.content_transfer, None);
    }

    #[test]
    fn timing_context_finish_minimal() {
        let start = Instant::now();

        let ctx = TimingContext {
            start,
            tcp_connected: None,
            tls_done: None,
            http_handshake_done: None,
            first_byte: None,
        };

        let timing = ctx.finish(None);
        assert_eq!(timing.tcp_connect, None);
        assert_eq!(timing.tls_handshake, None);
        assert_eq!(timing.http_handshake, None);
        assert_eq!(timing.time_to_first_byte, None);
        assert_eq!(timing.content_transfer, None);
    }

    #[test]
    fn build_forwarding_request_basic() {
        let req = build_forwarding_request(
            "POST",
            "/api/v1",
            &[
                ("content-type".into(), "application/json".into()),
                ("x-custom".into(), "value".into()),
            ],
            Bytes::from("{\"a\":1}"),
        )
        .unwrap();
        assert_eq!(req.method(), "POST");
        assert_eq!(req.uri(), "/api/v1");
        assert_eq!(req.headers().len(), 2);
        assert_eq!(
            req.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    #[test]
    fn build_client_response_basic() {
        let resp = build_response(
            404,
            &[("x-error".into(), "not found".into())],
            Full::new(Bytes::from("Not Found")),
            false,
        )
        .unwrap();
        assert_eq!(resp.status(), 404);
        assert_eq!(resp.headers().get("x-error").unwrap(), "not found");
    }

    #[test]
    fn bad_gateway_returns_502() {
        let resp = bad_gateway("upstream failed");
        assert_eq!(resp.status(), 502);
    }

    #[test]
    fn forwarding_strips_proxy_and_connection_headers() {
        let req = build_forwarding_request(
            "GET",
            "/",
            &[
                ("proxy-authorization".into(), "secret".into()),
                ("connection".into(), "x-private, keep-alive".into()),
                ("x-private".into(), "remove-me".into()),
                ("x-public".into(), "keep-me".into()),
            ],
            Bytes::new(),
        )
        .unwrap();
        assert!(req.headers().get("proxy-authorization").is_none());
        assert!(req.headers().get("connection").is_none());
        assert!(req.headers().get("x-private").is_none());
        assert_eq!(req.headers().get("x-public").unwrap(), "keep-me");
    }

    #[test]
    fn resolves_absolute_and_ipv6_targets() {
        assert_eq!(
            resolve_upstream_target("http://example.com:8080/a?q=1", "fallback", 80).unwrap(),
            UpstreamTarget {
                host: "example.com".into(),
                port: 8080,
                path_and_query: "/a?q=1".into(),
                scheme: Some(http::uri::Scheme::HTTP),
            }
        );
        assert_eq!(
            resolve_upstream_target("/v1", "[::1]:9090", 80).unwrap(),
            UpstreamTarget {
                host: "[::1]".trim_matches(['[', ']']).to_string(),
                port: 9090,
                path_and_query: "/v1".into(),
                scheme: None,
            }
        );
    }

    #[test]
    fn invalid_edited_request_is_an_error() {
        assert!(
            build_forwarding_request(
                "NOT A METHOD",
                "/",
                &[("bad header".into(), "value".into())],
                Bytes::new(),
            )
            .is_err()
        );
    }
}
