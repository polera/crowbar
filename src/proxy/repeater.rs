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
    let start = Instant::now();
    let result = if request.is_grpc {
        send_h2(&request).await
    } else if request.is_tls {
        send_https(&request).await
    } else {
        send_http(&request).await
    };
    match result {
        Ok(mut resp) => {
            resp.duration = start.elapsed();
            Ok(resp)
        }
        Err(e) => Err(e.to_string()),
    }
}

async fn send_http(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = strip_port(&request.host);
    let port = extract_port(&request.uri).unwrap_or(80);

    let tcp = TcpStream::connect(format!("{}:{}", host, port)).await?;
    send_h1_via(TokioIo::new(tcp), request).await
}

async fn send_https(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = strip_port(&request.host);
    let port = extract_port(&request.uri).unwrap_or(443);

    let tcp = TcpStream::connect(format!("{}:{}", host, port)).await?;
    let server_name = crate::tls::server_name_or_localhost(host);
    let connector = TlsConnector::from(crate::tls::build_tls_client_config());
    let tls_stream = connector.connect(server_name, tcp).await?;
    send_h1_via(TokioIo::new(tls_stream), request).await
}

async fn send_h1_via<IO>(io: TokioIo<IO>, request: &RequestData) -> anyhow::Result<ResponseData>
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, conn) = ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await?;

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
    parse_response(resp).await
}

async fn send_h2(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = strip_port(&request.host);
    let port = extract_port(&request.uri).unwrap_or(443);
    let addr = format!("{}:{}", host, port);

    let client_config = crate::tls::build_tls_h2_client_config();

    let tcp = TcpStream::connect(&addr).await?;
    let server_name = crate::tls::server_name_or_localhost(host);
    let connector = TlsConnector::from(client_config);
    let tls_stream = connector.connect(server_name, tcp).await?;

    let io = TokioIo::new(tls_stream);
    let (mut sender, conn) =
        hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .handshake(io)
            .await?;

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

    parse_h2_response(resp).await
}

async fn parse_response(
    resp: hyper::Response<hyper::body::Incoming>,
) -> anyhow::Result<ResponseData> {
    let status = resp.status().as_u16();
    let version = resp.version().into();
    let headers = crate::http::models::extract_headers(resp.headers());

    let body = resp.collect().await?.to_bytes();

    Ok(ResponseData {
        status,
        reason: crate::http::models::status_reason(status).to_string(),
        version,
        headers,
        body,
        trailers: Vec::new(),
        duration: std::time::Duration::ZERO,
    })
}

async fn parse_h2_response(
    resp: hyper::Response<hyper::body::Incoming>,
) -> anyhow::Result<ResponseData> {
    let status = resp.status().as_u16();
    let headers = crate::http::models::extract_headers(resp.headers());

    let collected = resp.into_body().collect().await?;
    let trailers_hm = collected.trailers().cloned();
    let body = collected.to_bytes();

    let trailers: Vec<(String, String)> = trailers_hm
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                .collect()
        })
        .unwrap_or_default();

    let resp_body = if body.is_empty() && !trailers.is_empty() {
        Bytes::new()
    } else {
        body
    };

    Ok(ResponseData {
        status,
        reason: crate::http::models::status_reason(status).to_string(),
        version: HttpVersion::Http2,
        headers,
        body: resp_body,
        trailers,
        duration: std::time::Duration::ZERO,
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
