use std::time::Instant;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tracing::debug;

use crate::channel::ProxyToUi;
use crate::http::models::{HttpVersion, RequestData, ResponseData};
use crate::proxy::TimingContext;

pub async fn send_request(
    request: RequestData,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
) {
    match send_raw_request(request).await {
        Ok(resp) => {
            let _ = ui_tx.send(ProxyToUi::RepeaterResponse(resp));
        }
        Err(e) => {
            let _ = ui_tx.send(ProxyToUi::RepeaterError(e));
        }
    }
}

pub async fn send_raw_request(request: RequestData) -> Result<ResponseData, String> {
    let result = if request.is_grpc {
        send_h2(&request).await
    } else if request.is_tls {
        send_https(&request).await
    } else {
        send_http(&request).await
    };
    result.map_err(|e| e.to_string())
}

async fn send_http(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = strip_port(&request.host);
    let port = extract_port(&request.uri).unwrap_or(80);

    let mut timing = TimingContext::new();
    let tcp = TcpStream::connect(format!("{}:{}", host, port)).await?;
    timing.tcp_connected = Some(Instant::now());
    send_h1_via(TokioIo::new(tcp), request, timing).await
}

async fn send_https(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = strip_port(&request.host);
    let port = extract_port(&request.uri).unwrap_or(443);

    let mut timing = TimingContext::new();
    let tcp = TcpStream::connect(format!("{}:{}", host, port)).await?;
    timing.tcp_connected = Some(Instant::now());
    let server_name = crate::tls::server_name_or_localhost(host);
    let connector = TlsConnector::from(crate::tls::build_tls_client_config());
    let tls_stream = connector.connect(server_name, tcp).await?;
    timing.tls_done = Some(Instant::now());
    send_h1_via(TokioIo::new(tls_stream), request, timing).await
}

async fn send_h1_via<IO>(io: TokioIo<IO>, request: &RequestData, mut timing: TimingContext) -> anyhow::Result<ResponseData>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, conn) = ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await?;

    timing.http_handshake_done = Some(Instant::now());

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("Repeater connection ended: {}", e);
        }
    });

    let path = crate::http::extract_path(&request.uri);
    let req = crate::proxy::build_forwarding_request(
        &request.method, path, &request.headers, request.body.clone(),
    );
    let resp = sender.send_request(req).await?;
    timing.first_byte = Some(Instant::now());
    parse_response(resp, timing).await
}

async fn send_h2(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = strip_port(&request.host);
    let port = extract_port(&request.uri).unwrap_or(443);
    let addr = format!("{}:{}", host, port);

    let mut timing = TimingContext::new();
    let client_config = crate::tls::build_tls_h2_client_config();

    let tcp = TcpStream::connect(&addr).await?;
    timing.tcp_connected = Some(Instant::now());
    let server_name = crate::tls::server_name_or_localhost(host);
    let connector = TlsConnector::from(client_config);
    let tls_stream = connector.connect(server_name, tcp).await?;
    timing.tls_done = Some(Instant::now());

    let io = TokioIo::new(tls_stream);
    let (mut sender, conn) =
        hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .handshake(io)
            .await?;

    timing.http_handshake_done = Some(Instant::now());

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("Repeater H2 connection ended: {}", e);
        }
    });

    let path = crate::http::extract_path(&request.uri);
    let upstream_uri = if port == 443 {
        format!("https://{}{}", host, path)
    } else {
        format!("https://{}:{}{}", host, port, path)
    };

    let req = crate::proxy::build_forwarding_request(
        &request.method, &upstream_uri, &request.headers, request.body.clone(),
    );
    let resp = sender.send_request(req).await?;
    timing.first_byte = Some(Instant::now());

    parse_h2_response(resp, timing).await
}

async fn parse_response(
    resp: hyper::Response<hyper::body::Incoming>,
    timing: TimingContext,
) -> anyhow::Result<ResponseData> {
    let status = resp.status().as_u16();
    let version = resp.version().into();
    let headers = crate::http::models::extract_headers(resp.headers());

    let body_start = Instant::now();
    let body = resp.collect().await?.to_bytes();
    let content_transfer = body_start.elapsed();
    let duration = timing.start.elapsed();
    let timing_data = timing.finish(Some(content_transfer));

    Ok(ResponseData {
        status,
        reason: crate::http::models::status_reason(status).to_string(),
        version,
        headers,
        body,
        trailers: Vec::new(),
        duration,
        timing: Some(timing_data),
    })
}

async fn parse_h2_response(
    resp: hyper::Response<hyper::body::Incoming>,
    timing: TimingContext,
) -> anyhow::Result<ResponseData> {
    let status = resp.status().as_u16();
    let headers = crate::http::models::extract_headers(resp.headers());

    let body_start = Instant::now();
    let collected = resp.into_body().collect().await?;
    let content_transfer = body_start.elapsed();
    let trailers_hm = collected.trailers().cloned();
    let body = collected.to_bytes();
    let duration = timing.start.elapsed();

    let trailers = crate::http::models::extract_trailers(trailers_hm.as_ref());

    let resp_body = if body.is_empty() && !trailers.is_empty() {
        Bytes::new()
    } else {
        body
    };

    let timing_data = timing.finish(Some(content_transfer));

    Ok(ResponseData {
        status,
        reason: crate::http::models::status_reason(status).to_string(),
        version: HttpVersion::Http2,
        headers,
        body: resp_body,
        trailers,
        duration,
        timing: Some(timing_data),
    })
}

fn strip_port(host: &str) -> &str {
    if let Some(bracket) = host.find(']') {
        // IPv6: [::1]:port
        return &host[..bracket + 1];
    }
    host.split(':').next().unwrap_or(host)
}

fn extract_port(uri: &str) -> Option<u16> {
    if let Some(pos) = uri.find("://") {
        let after_scheme = &uri[pos + 3..];
        let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
        if let Some(colon) = authority.rfind(':') {
            return authority[colon + 1..].parse().ok();
        }
    }
    None
}
