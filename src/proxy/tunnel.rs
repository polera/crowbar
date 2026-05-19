use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use http_body::Frame;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, error, warn};

use crate::channel::ProxyToUi;
use crate::http::models::{GrpcDirection, GrpcMessage, HttpVersion, RequestData, RequestId, ResponseData};
use crate::proxy::intercept::{InterceptDecision, InterceptState};
use crate::proxy::scope::Scope;
use crate::rules::{self, SharedRules};
use crate::tls::cert_cache::CertCache;
use crate::tls::cert_gen;

#[allow(clippy::too_many_arguments)]
pub async fn handle_tunnel(
    upgraded: hyper::upgrade::Upgraded,
    host: String,
    port: u16,
    cert_cache: Arc<CertCache>,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    intercept: Arc<InterceptState>,
    scope: Arc<Scope>,
    rules: SharedRules,
) {
    if let Err(e) = run_tunnel(upgraded, &host, port, cert_cache, ui_tx, intercept, scope, rules).await {
        debug!("Tunnel to {}:{} ended: {}", host, port, e);
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_tunnel(
    upgraded: hyper::upgrade::Upgraded,
    host: &str,
    port: u16,
    cert_cache: Arc<CertCache>,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    intercept: Arc<InterceptState>,
    scope: Arc<Scope>,
    rules: SharedRules,
) -> anyhow::Result<()> {
    let certified_key = cert_cache.get_or_generate(host).await?;
    let server_config = cert_gen::build_server_config(certified_key)?;
    let acceptor = TlsAcceptor::from(server_config);

    let client_stream = TokioIo::new(upgraded);
    let tls_stream = acceptor.accept(client_stream).await?;

    let is_h2 = tls_stream
        .get_ref()
        .1
        .alpn_protocol()
        .is_some_and(|p| p == b"h2");

    if is_h2 {
        debug!("Client negotiated h2 for {}:{}", host, port);
        run_h2_tunnel(tls_stream, host, port, ui_tx, intercept, scope, rules).await
    } else {
        run_h1_tunnel(tls_stream, host, port, ui_tx, intercept, scope, rules).await
    }
}

async fn run_h1_tunnel(
    tls_stream: tokio_rustls::server::TlsStream<TokioIo<hyper::upgrade::Upgraded>>,
    host: &str,
    port: u16,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    intercept: Arc<InterceptState>,
    scope: Arc<Scope>,
    rules: SharedRules,
) -> anyhow::Result<()> {
    let host = host.to_string();

    let svc = service_fn(move |req: Request<Incoming>| {
        let host = host.clone();
        let ui_tx = ui_tx.clone();
        let intercept = intercept.clone();
        let scope = scope.clone();
        let rules = rules.clone();
        async move { handle_inner_request(req, &host, port, &ui_tx, &intercept, &scope, &rules).await }
    });

    ServerBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(TokioIo::new(tls_stream), svc)
        .with_upgrades()
        .await?;

    Ok(())
}

fn is_websocket_upgrade(req: &Request<Incoming>) -> bool {
    req.headers()
        .get(hyper::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

async fn handle_inner_request(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
    intercept: &InterceptState,
    scope: &Scope,
    rules: &SharedRules,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    if is_websocket_upgrade(&req) {
        return handle_websocket_upgrade(req, host, port, ui_tx, scope).await;
    }

    handle_http_request(req, host, port, ui_tx, intercept, scope, rules).await
}

async fn handle_websocket_upgrade(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
    scope: &Scope,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    let in_scope = scope.is_in_scope(host);
    let request_id = RequestId::next();

    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    let full_uri = format!(
        "wss://{}{}",
        host,
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
        let _ = ui_tx.send(ProxyToUi::RequestCaptured(request_data));
    }

    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );

    let addr = format!("{}:{}", host, port);
    let tcp = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!("WebSocket: failed to connect to upstream {}: {}", addr, e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("Connection failed: {}", e),
            ));
            return Ok(bad_gateway(&format!("Connection failed: {}", e)));
        }
    };

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| ())
        .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());
    let connector = TlsConnector::from(client_config);
    let tls_stream = match connector.connect(server_name, tcp).await {
        Ok(s) => s,
        Err(e) => {
            warn!("WebSocket: TLS handshake failed with {}: {}", addr, e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("TLS handshake failed: {}", e),
            ));
            return Ok(bad_gateway(&format!("TLS handshake failed: {}", e)));
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
            return Ok(bad_gateway(&format!("HTTP handshake failed: {}", e)));
        }
    };

    tokio::spawn(async move {
        if let Err(e) = conn.await {
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
            return Ok(bad_gateway(&format!("Request failed: {}", e)));
        }
    };

    let resp_status = upstream_resp.status().as_u16();

    if resp_status != 101 {
        let resp_headers: Vec<(String, String)> = upstream_resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
            .collect();
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

    let resp_headers: Vec<(String, String)> = upstream_resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    let response_data = ResponseData {
        status: 101,
        reason: "Switching Protocols".to_string(),
        version: HttpVersion::Http11,
        headers: resp_headers.clone(),
        body: Bytes::new(),
        trailers: Vec::new(),
        duration: std::time::Duration::ZERO,
    };
    if in_scope {
        let _ = ui_tx.send(ProxyToUi::ResponseReceived(request_id, response_data));
    }

    let ui_tx_clone = ui_tx.clone();
    let host_owned = host.to_string();

    tokio::spawn(async move {
        let upstream_upgraded = match hyper::upgrade::on(upstream_resp).await {
            Ok(u) => u,
            Err(e) => {
                debug!("WebSocket upstream upgrade failed for {}: {}", host_owned, e);
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
        )
        .await;
    });

    let mut response = hyper::Response::builder().status(101);
    for (key, value) in &resp_headers {
        response = response.header(key.as_str(), value.as_str());
    }

    Ok(response.body(Full::new(Bytes::new())).unwrap())
}

async fn handle_http_request(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
    intercept: &InterceptState,
    scope: &Scope,
    rules: &SharedRules,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    let in_scope = scope.is_in_scope(host);
    let request_id = RequestId::next();
    let start = Instant::now();

    let method = req.method().clone();
    let version = match req.version() {
        hyper::Version::HTTP_10 => HttpVersion::Http10,
        hyper::Version::HTTP_11 => HttpVersion::Http11,
        hyper::Version::HTTP_2 => HttpVersion::Http2,
        _ => HttpVersion::Http11,
    };

    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    let (parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            warn!("Failed to read request body: {}", e);
            return Ok(bad_gateway(&format!("Failed to read request body: {}", e)));
        }
    };

    let full_uri = format!(
        "https://{}{}",
        host,
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
        let _ = ui_tx.send(ProxyToUi::RequestCaptured(request_data.clone()));

        if let Some(rx) = intercept.intercept_request(&request_data, ui_tx) {
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
                Err(_) => {}
            }
        }
    }

    rules::apply_request_rules(
        rules,
        &mut request_data.uri,
        &mut request_data.headers,
        &mut request_data.body,
    );

    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );

    let addr = format!("{}:{}", host, port);
    let tcp = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to connect to upstream {}: {}", addr, e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("Connection failed: {}", e),
            ));
            return Ok(bad_gateway(&format!("Connection failed: {}", e)));
        }
    };

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| ())
        .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());
    let connector = TlsConnector::from(client_config);
    let tls_stream = match connector.connect(server_name, tcp).await {
        Ok(s) => s,
        Err(e) => {
            warn!("TLS handshake failed with {}: {}", addr, e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("TLS handshake failed: {}", e),
            ));
            return Ok(bad_gateway(&format!("TLS handshake failed: {}", e)));
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
            warn!("Upstream HTTP handshake failed: {}", e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("HTTP handshake failed: {}", e),
            ));
            return Ok(bad_gateway(&format!("HTTP handshake failed: {}", e)));
        }
    };

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("Upstream connection ended: {}", e);
        }
    });

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let fwd_headers = &request_data.headers;
    let fwd_body = request_data.body.clone();

    let mut upstream_req = hyper::Request::builder()
        .method(parts.method)
        .uri(path_and_query)
        .version(parts.version);

    for (key, value) in fwd_headers {
        upstream_req = upstream_req.header(key.as_str(), value.as_str());
    }

    let upstream_req = upstream_req
        .body(Full::new(fwd_body))
        .expect("building upstream request");

    let upstream_resp = match sender.send_request(upstream_req).await {
        Ok(resp) => resp,
        Err(e) => {
            warn!("Upstream request to {} failed: {}", addr, e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("Request failed: {}", e),
            ));
            return Ok(bad_gateway(&format!("Request failed: {}", e)));
        }
    };

    let resp_status = upstream_resp.status().as_u16();
    let resp_version = match upstream_resp.version() {
        hyper::Version::HTTP_10 => HttpVersion::Http10,
        hyper::Version::HTTP_11 => HttpVersion::Http11,
        hyper::Version::HTTP_2 => HttpVersion::Http2,
        _ => HttpVersion::Http11,
    };
    let mut resp_headers: Vec<(String, String)> = upstream_resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    let mut resp_body = match upstream_resp.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            error!("Failed to read upstream response body: {}", e);
            Bytes::new()
        }
    };
    let duration = start.elapsed();

    rules::apply_response_rules(rules, &mut resp_headers, &mut resp_body);

    let response_data = ResponseData {
        status: resp_status,
        reason: http::StatusCode::from_u16(resp_status)
            .map(|s| s.canonical_reason().unwrap_or(""))
            .unwrap_or("")
            .to_string(),
        version: resp_version,
        headers: resp_headers.clone(),
        body: resp_body.clone(),
        trailers: Vec::new(),
        duration,
    };

    if in_scope {
        let _ = ui_tx.send(ProxyToUi::ResponseReceived(request_id, response_data));
    }

    let mut response = hyper::Response::builder().status(resp_status);
    for (key, value) in &resp_headers {
        response = response.header(key.as_str(), value.as_str());
    }

    Ok(response.body(Full::new(resp_body)).unwrap())
}

fn bad_gateway(msg: &str) -> hyper::Response<Full<Bytes>> {
    hyper::Response::builder()
        .status(502)
        .body(Full::new(Bytes::from(msg.to_string())))
        .unwrap()
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
        self.data.is_none() && self.trailers.is_none()
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;
struct GrpcTeeBody {
    inner: Incoming,
    request_id: RequestId,
    direction: GrpcDirection,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    buffer: BytesMut,
    in_scope: bool,
}

impl http_body::Body for GrpcTeeBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if this.in_scope {
                    if let Some(data) = frame.data_ref() {
                        this.buffer.extend_from_slice(data);
                        extract_grpc_messages(
                            &mut this.buffer,
                            this.request_id,
                            this.direction,
                            &this.ui_tx,
                        );
                    }
                    if let Some(trailers) = frame.trailers_ref() {
                        let pairs: Vec<(String, String)> = trailers
                            .iter()
                            .map(|(k, v)| {
                                (k.to_string(), v.to_str().unwrap_or("<binary>").to_string())
                            })
                            .collect();
                        let _ = this.ui_tx.send(ProxyToUi::GrpcTrailers(this.request_id, pairs));
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
}

fn extract_grpc_messages(
    buffer: &mut BytesMut,
    request_id: RequestId,
    direction: GrpcDirection,
    ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
) {
    while buffer.len() >= 5 {
        let compressed = buffer[0] != 0;
        let len =
            u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
        if buffer.len() < 5 + len {
            break;
        }
        let frame_data = buffer.split_to(5 + len);
        let payload = Bytes::copy_from_slice(&frame_data[5..]);

        let msg = GrpcMessage {
            direction,
            compressed,
            payload,
            timestamp: std::time::SystemTime::now(),
        };
        let _ = ui_tx.send(ProxyToUi::GrpcFrame(request_id, msg));
    }
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
            H2RespBody::Streaming(body) => Pin::new(body)
                .poll_frame(cx)
                .map(|opt| opt.map(|res| res.map_err(|e| Box::new(e) as BoxError))),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            H2RespBody::Buffered(b) => b.is_end_stream(),
            H2RespBody::Streaming(b) => b.is_end_stream(),
        }
    }
}

async fn run_h2_tunnel(
    tls_stream: tokio_rustls::server::TlsStream<TokioIo<hyper::upgrade::Upgraded>>,
    host: &str,
    port: u16,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    intercept: Arc<InterceptState>,
    scope: Arc<Scope>,
    rules: SharedRules,
) -> anyhow::Result<()> {
    let upstream_sender = match establish_h2_upstream(host, port).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to establish h2 upstream to {}:{}: {}", host, port, e);
            return Err(e);
        }
    };

    let host = host.to_string();
    let svc = service_fn(move |req: Request<Incoming>| {
        let host = host.clone();
        let ui_tx = ui_tx.clone();
        let intercept = intercept.clone();
        let scope = scope.clone();
        let rules = rules.clone();
        let sender = upstream_sender.clone();
        async move {
            handle_h2_request(req, &host, port, &ui_tx, &intercept, &scope, &rules, sender).await
        }
    });

    hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(tls_stream), svc)
        .await?;

    Ok(())
}

async fn establish_h2_upstream(
    host: &str,
    port: u16,
) -> anyhow::Result<hyper::client::conn::http2::SendRequest<Full<Bytes>>> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut client_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec()];
    let client_config = Arc::new(client_config);

    let addr = format!("{}:{}", host, port);
    let tcp = TcpStream::connect(&addr).await?;

    let server_name = ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());
    let connector = TlsConnector::from(client_config);
    let tls_stream = connector.connect(server_name, tcp).await?;

    let io = TokioIo::new(tls_stream);
    let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(io)
        .await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("H2 upstream connection ended: {}", e);
        }
    });

    Ok(sender)
}

#[allow(clippy::too_many_arguments)]
async fn handle_h2_request(
    req: Request<Incoming>,
    host: &str,
    port: u16,
    ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
    intercept: &InterceptState,
    scope: &Scope,
    rules: &SharedRules,
    mut upstream: hyper::client::conn::http2::SendRequest<Full<Bytes>>,
) -> Result<hyper::Response<H2RespBody>, Infallible> {
    let in_scope = scope.is_in_scope(host);
    let request_id = RequestId::next();
    let start = Instant::now();

    let is_grpc = is_grpc_request(&req);
    let method = req.method().clone();

    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    let (parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            warn!("Failed to read h2 request body: {}", e);
            return Ok(h2_bad_gateway(&format!("Failed to read request body: {}", e)));
        }
    };

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let full_uri = format!("https://{}{}", host, path_and_query);

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
        if is_grpc && !body_bytes.is_empty() {
            let mut buf = BytesMut::from(body_bytes.as_ref());
            extract_grpc_messages(&mut buf, request_id, GrpcDirection::ClientToServer, ui_tx);
        }

        let _ = ui_tx.send(ProxyToUi::RequestCaptured(request_data.clone()));

        if !is_grpc
            && let Some(rx) = intercept.intercept_request(&request_data, ui_tx) {
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
                    Err(_) => {}
                }
            }
    }

    if !is_grpc {
        rules::apply_request_rules(
            rules,
            &mut request_data.uri,
            &mut request_data.headers,
            &mut request_data.body,
        );
    }

    let fwd_path = {
        let uri: http::Uri = request_data.uri.parse().unwrap_or_else(|_| {
            http::Uri::builder()
                .path_and_query("/")
                .build()
                .unwrap()
        });
        uri.path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string()
    };

    let upstream_uri = if port == 443 {
        format!("https://{}{}", host, fwd_path)
    } else {
        format!("https://{}:{}{}", host, port, fwd_path)
    };

    let fwd_headers = &request_data.headers;
    let fwd_body = request_data.body.clone();

    let mut upstream_req = hyper::Request::builder()
        .method(parts.method)
        .uri(&upstream_uri);

    for (key, value) in fwd_headers {
        upstream_req = upstream_req.header(key.as_str(), value.as_str());
    }

    let upstream_req = upstream_req
        .body(Full::new(fwd_body))
        .expect("building h2 upstream request");

    let upstream_resp = match upstream.send_request(upstream_req).await {
        Ok(resp) => resp,
        Err(e) => {
            warn!("H2 upstream request to {}:{} failed: {}", host, port, e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("Request failed: {}", e),
            ));
            return Ok(h2_bad_gateway(&format!("Request failed: {}", e)));
        }
    };

    let resp_status = upstream_resp.status().as_u16();
    let resp_headers: Vec<(String, String)> = upstream_resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect();

    if is_grpc {
        let duration = start.elapsed();

        if in_scope {
            let response_data = ResponseData {
                status: resp_status,
                reason: http::StatusCode::from_u16(resp_status)
                    .map(|s| s.canonical_reason().unwrap_or(""))
                    .unwrap_or("")
                    .to_string(),
                version: HttpVersion::Http2,
                headers: resp_headers.clone(),
                body: Bytes::new(),
                trailers: Vec::new(),
                duration,
            };
            let _ = ui_tx.send(ProxyToUi::ResponseReceived(request_id, response_data));
        }

        let (_, resp_body_incoming) = upstream_resp.into_parts();
        let resp_tee = GrpcTeeBody {
            inner: resp_body_incoming,
            request_id,
            direction: GrpcDirection::ServerToClient,
            ui_tx: ui_tx.clone(),
            buffer: BytesMut::new(),
            in_scope,
        };

        let mut response = hyper::Response::builder().status(resp_status);
        for (key, value) in &resp_headers {
            response = response.header(key.as_str(), value.as_str());
        }
        return Ok(response.body(H2RespBody::Streaming(resp_tee)).unwrap());
    }

    let (_, resp_body_incoming) = upstream_resp.into_parts();
    let collected = match resp_body_incoming.collect().await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read h2 upstream response body: {}", e);
            let _ = ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("Response body error: {}", e),
            ));
            return Ok(h2_bad_gateway(&format!("Response body error: {}", e)));
        }
    };

    let resp_trailers = collected.trailers().cloned();
    let mut resp_body = collected.to_bytes();
    let duration = start.elapsed();

    let trailer_pairs: Vec<(String, String)> = resp_trailers
        .as_ref()
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut resp_headers_mut = resp_headers;
    rules::apply_response_rules(rules, &mut resp_headers_mut, &mut resp_body);

    let response_data = ResponseData {
        status: resp_status,
        reason: http::StatusCode::from_u16(resp_status)
            .map(|s| s.canonical_reason().unwrap_or(""))
            .unwrap_or("")
            .to_string(),
        version: HttpVersion::Http2,
        headers: resp_headers_mut.clone(),
        body: resp_body.clone(),
        trailers: trailer_pairs,
        duration,
    };

    if in_scope {
        let _ = ui_tx.send(ProxyToUi::ResponseReceived(request_id, response_data));
    }

    let mut response = hyper::Response::builder().status(resp_status);
    for (key, value) in &resp_headers_mut {
        response = response.header(key.as_str(), value.as_str());
    }

    Ok(response
        .body(H2RespBody::Buffered(H2Body {
            data: Some(resp_body),
            trailers: resp_trailers,
        }))
        .unwrap())
}
