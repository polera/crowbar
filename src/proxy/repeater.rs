use std::sync::Arc;
use std::time::Instant;

use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::Request;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
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
    let start = Instant::now();

    let result = if request.is_tls {
        send_https(&request).await
    } else {
        send_http(&request).await
    };

    match result {
        Ok(mut resp) => {
            resp.duration = start.elapsed();
            let _ = ui_tx.send(ProxyToUi::RepeaterResponse(resp));
        }
        Err(e) => {
            let _ = ui_tx.send(ProxyToUi::RepeaterError(e.to_string()));
        }
    }
}

async fn send_http(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = &request.host;
    let port = extract_port(&request.uri).unwrap_or(80);
    let addr = format!("{}:{}", host, port);

    let tcp = TcpStream::connect(&addr).await?;
    let io = TokioIo::new(tcp);

    let (mut sender, conn) = ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("Repeater HTTP connection ended: {}", e);
        }
    });

    let path = extract_path(&request.uri);

    let mut req = Request::builder()
        .method(request.method.as_str())
        .uri(&path);

    for (key, value) in &request.headers {
        req = req.header(key.as_str(), value.as_str());
    }

    let req = req.body(Full::new(request.body.clone()))?;
    let resp = sender.send_request(req).await?;

    parse_response(resp).await
}

async fn send_https(request: &RequestData) -> anyhow::Result<ResponseData> {
    let host = &request.host;
    let port = extract_port(&request.uri).unwrap_or(443);
    let addr = format!("{}:{}", host, port);

    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );

    let tcp = TcpStream::connect(&addr).await?;
    let server_name = ServerName::try_from(host.clone())
        .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());
    let connector = TlsConnector::from(client_config);
    let tls_stream = connector.connect(server_name, tcp).await?;

    let io = TokioIo::new(tls_stream);
    let (mut sender, conn) = ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("Repeater HTTPS connection ended: {}", e);
        }
    });

    let path = extract_path(&request.uri);

    let mut req = Request::builder()
        .method(request.method.as_str())
        .uri(&path);

    for (key, value) in &request.headers {
        req = req.header(key.as_str(), value.as_str());
    }

    let req = req.body(Full::new(request.body.clone()))?;
    let resp = sender.send_request(req).await?;

    parse_response(resp).await
}

async fn parse_response(
    resp: hyper::Response<hyper::body::Incoming>,
) -> anyhow::Result<ResponseData> {
    let status = resp.status().as_u16();
    let version = match resp.version() {
        hyper::Version::HTTP_10 => HttpVersion::Http10,
        hyper::Version::HTTP_11 => HttpVersion::Http11,
        hyper::Version::HTTP_2 => HttpVersion::Http2,
        _ => HttpVersion::Http11,
    };
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    let body = resp.collect().await?.to_bytes();

    Ok(ResponseData {
        status,
        reason: http::StatusCode::from_u16(status)
            .map(|s| s.canonical_reason().unwrap_or(""))
            .unwrap_or("")
            .to_string(),
        version,
        headers,
        body,
        duration: std::time::Duration::ZERO,
    })
}

fn extract_path(uri: &str) -> String {
    if let Some(pos) = uri.find("://") {
        let after_scheme = &uri[pos + 3..];
        if let Some(slash) = after_scheme.find('/') {
            return after_scheme[slash..].to_string();
        }
    }
    if uri.starts_with('/') {
        return uri.to_string();
    }
    "/".to_string()
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
