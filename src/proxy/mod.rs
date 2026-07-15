pub mod handler;
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

use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper_util::rt::TokioIo;
use tracing::debug;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
}

pub(crate) fn resolve_upstream_target(
    uri: &str,
    fallback_authority: &str,
    default_port: u16,
) -> anyhow::Result<UpstreamTarget> {
    let parsed: http::Uri = uri
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid request URI {uri:?}: {error}"))?;
    let fallback: http::uri::Authority = fallback_authority
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid authority {fallback_authority:?}: {error}"))?;
    let authority = parsed.authority().unwrap_or(&fallback);
    Ok(UpstreamTarget {
        host: authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port: authority.port_u16().unwrap_or(default_port),
        path_and_query: parsed
            .path_and_query()
            .map_or_else(|| "/".to_string(), ToString::to_string),
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
}

pub(crate) fn build_forwarding_request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Bytes,
) -> Result<Request<Full<Bytes>>, http::Error> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (key, value) in end_to_end_headers(headers, false) {
        builder = builder.header(key.as_str(), value.as_str());
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

pub(crate) fn build_client_response(
    status: u16,
    headers: &[(String, String)],
    body: Bytes,
) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(status);
    for (key, value) in end_to_end_headers(headers, false) {
        builder = builder.header(key.as_str(), value.as_str());
    }
    builder.body(Full::new(body)).unwrap_or_else(|error| {
        Response::builder()
            .status(http::StatusCode::BAD_GATEWAY)
            .body(Full::new(Bytes::from(format!(
                "Invalid rewritten response: {error}"
            ))))
            .expect("static 502 response is valid")
    })
}

pub(crate) async fn process_h1_response(
    upstream_resp: Response<hyper::body::Incoming>,
    request_id: RequestId,
    timing: TimingContext,
    in_scope: bool,
    ctx: &ProxyContext,
) -> Response<Full<Bytes>> {
    let resp_status = upstream_resp.status().as_u16();
    let resp_version = upstream_resp.version().into();
    let mut resp_headers = crate::http::models::extract_headers(upstream_resp.headers());

    let body_start = Instant::now();
    let mut resp_body =
        match http_body_util::Limited::new(upstream_resp.into_body(), ctx.limits.max_body_bytes)
            .collect()
            .await
        {
            Ok(body) => body.to_bytes(),
            Err(error) => {
                let _ = ctx.ui_tx.try_send(ProxyToUi::RequestError(
                    request_id,
                    format!("Response body rejected: {error}"),
                ));
                return bad_gateway(&format!("Response body rejected: {error}"));
            }
        };
    let content_transfer = body_start.elapsed();
    let duration = timing.start.elapsed();

    rules::apply_response_rules(&ctx.rules, &mut resp_headers, &mut resp_body);

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
            .try_send(ProxyToUi::ResponseReceived(request_id, response_data));
    }

    build_client_response(resp_status, &resp_headers, resp_body)
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

    let (mut sender, conn) = match ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            let _ = ctx.ui_tx.try_send(ProxyToUi::RequestError(
                request_id,
                format!("HTTP handshake failed: {}", e),
            ));
            return bad_gateway(&format!("HTTP handshake failed: {}", e));
        }
    };

    timing.http_handshake_done = Some(Instant::now());

    tokio::spawn(async move {
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
            let _ = ctx.ui_tx.try_send(ProxyToUi::RequestError(
                request_id,
                format!("Invalid edited request: {error}"),
            ));
            return bad_gateway(&format!("Invalid edited request: {error}"));
        }
    };
    *upstream_req.version_mut() = version;

    let upstream_resp = match sender.send_request(upstream_req).await {
        Ok(resp) => resp,
        Err(e) => {
            let _ = ctx.ui_tx.try_send(ProxyToUi::RequestError(
                request_id,
                format!("Request failed: {}", e),
            ));
            return bad_gateway(&format!("Upstream request failed: {}", e));
        }
    };

    timing.first_byte = Some(Instant::now());

    process_h1_response(upstream_resp, request_id, timing, in_scope, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let resp = build_client_response(
            404,
            &[("x-error".into(), "not found".into())],
            Bytes::from("Not Found"),
        );
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
            }
        );
        assert_eq!(
            resolve_upstream_target("/v1", "[::1]:9090", 80).unwrap(),
            UpstreamTarget {
                host: "[::1]".trim_matches(['[', ']']).to_string(),
                port: 9090,
                path_and_query: "/v1".into(),
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
