use std::time::Instant;

use anyhow::Context;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio_rustls::TlsConnector;

use crate::http::models::{HttpVersion, RequestData, ResponseData};
use crate::proxy::{ProxyLimits, TimingContext, io_timeout};

/// A cancelled replay must not leave an upstream driver running indefinitely.
struct ConnectionDriver(tokio::task::JoinHandle<()>);

impl Drop for ConnectionDriver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn send_raw_request(
    request: RequestData,
    limits: ProxyLimits,
) -> anyhow::Result<ResponseData> {
    anyhow::ensure!(
        request.body.len() <= limits.max_body_bytes,
        "Request body exceeds configured limit"
    );
    let fallback = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map_or(request.host.as_str(), |(_, value)| value.as_str());
    let target = crate::proxy::resolve_upstream_target(
        &request.uri,
        fallback,
        if request.is_tls { 443 } else { 80 },
    )?;
    let mut request = request;
    request.is_tls = target.uses_tls(request.is_tls);
    let mut timing = TimingContext::new();
    let tcp = crate::proxy::connect_tcp(&target.host, target.port)
        .await
        .context("connecting replay upstream")?;
    timing.tcp_connected = Some(Instant::now());
    let h2 = request.is_grpc || request.version == HttpVersion::Http2;
    if request.is_tls {
        let config = if h2 {
            crate::tls::build_tls_h2_client_config()
        } else {
            crate::tls::build_tls_client_config()
        };
        let connector = TlsConnector::from(config);
        let stream =
            io_timeout(connector.connect(crate::tls::server_name_or_localhost(&target.host), tcp))
                .await
                .context("replay TLS handshake")?;
        timing.tls_done = Some(Instant::now());
        let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
        anyhow::ensure!(!request.is_grpc || h2, "gRPC requires an HTTP/2 upstream");
        send_via(stream, &request, &target, timing, limits, h2).await
    } else {
        send_via(tcp, &request, &target, timing, limits, h2).await
    }
}

async fn send_via<IO>(
    io: IO,
    request: &RequestData,
    target: &crate::proxy::UpstreamTarget,
    mut timing: TimingContext,
    limits: ProxyLimits,
    h2: bool,
) -> anyhow::Result<ResponseData>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let authority = crate::proxy::format_authority(
        &target.host,
        target.port,
        if request.is_tls { 443 } else { 80 },
    );
    let uri = if h2 {
        format!(
            "{}://{authority}{}",
            if request.is_tls { "https" } else { "http" },
            target.path_and_query
        )
    } else {
        target.path_and_query.clone()
    };
    let mut req = crate::proxy::build_forwarding_request(
        &request.method,
        &uri,
        &request.headers,
        request.body.clone(),
    )
    .context("building replay request")?;
    if !req.headers().contains_key(http::header::HOST) {
        req.headers_mut()
            .insert(http::header::HOST, authority.parse()?);
    }
    if request.is_grpc {
        req.headers_mut()
            .insert(http::header::TE, http::HeaderValue::from_static("trailers"));
    }
    let (response, _driver) = if h2 {
        let (mut sender, connection) =
            hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                .handshake::<_, Full<Bytes>>(TokioIo::new(io))
                .await?;
        timing.http_handshake_done = Some(Instant::now());
        let driver = ConnectionDriver(tokio::spawn(async move {
            let _ = connection.await;
        }));
        (
            io_timeout(sender.send_request(req))
                .await
                .context("sending HTTP/2 replay")?,
            driver,
        )
    } else {
        *req.version_mut() = if request.version == HttpVersion::Http10 {
            hyper::Version::HTTP_10
        } else {
            hyper::Version::HTTP_11
        };
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .preserve_header_case(true)
            .title_case_headers(true)
            .handshake(TokioIo::new(io))
            .await?;
        timing.http_handshake_done = Some(Instant::now());
        let driver = ConnectionDriver(tokio::spawn(async move {
            let _ = connection.await;
        }));
        (
            io_timeout(sender.send_request(req))
                .await
                .context("sending HTTP/1 replay")?,
            driver,
        )
    };
    timing.first_byte = Some(Instant::now());
    parse_response(response, timing, limits).await
}

async fn parse_response(
    response: hyper::Response<hyper::body::Incoming>,
    timing: TimingContext,
    limits: ProxyLimits,
) -> anyhow::Result<ResponseData> {
    let status = response.status().as_u16();
    let version = response.version().into();
    let headers = crate::http::models::extract_headers(response.headers());
    let body_start = Instant::now();
    let collected = io_timeout(
        http_body_util::Limited::new(response.into_body(), limits.max_body_bytes).collect(),
    )
    .await
    .context("reading replay response")?;
    let content_transfer = body_start.elapsed();
    let trailers = crate::http::models::extract_trailers(collected.trailers());
    let body = collected.to_bytes();
    Ok(ResponseData {
        status,
        reason: crate::http::models::status_reason(status).into(),
        version,
        headers,
        body,
        trailers,
        duration: timing.start.elapsed(),
        timing: Some(timing.finish(Some(content_transfer))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::service::service_fn;
    use std::convert::Infallible;

    #[tokio::test]
    async fn configured_limits_apply_to_replay_requests_and_responses() {
        let limits = ProxyLimits {
            max_body_bytes: 2,
            ..ProxyLimits::default()
        };
        let mut request = crate::test_support::request();
        request.body = Bytes::from_static(b"big");
        assert!(
            send_raw_request(request.clone(), limits)
                .await
                .unwrap_err()
                .to_string()
                .contains("configured limit")
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        request.uri = format!("http://{}/", listener.local_addr().unwrap());
        request.body = Bytes::new();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(|_| async {
                Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from_static(b"big"))))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        let error = send_raw_request(request, limits).await.unwrap_err();
        assert!(format!("{error:#}").contains("length limit"));
        server.await.unwrap();
    }
}
