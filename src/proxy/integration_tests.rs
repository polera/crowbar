//! Loopback-only protocol tests, including upgrades and cancellation under saturation.
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ProxyContext, ProxyLimits, server::ProxyServer};
use crate::channel::ProxyToUi;

async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("protocol test timed out")
}

async fn start_proxy(
    ctx: ProxyContext,
) -> (
    SocketAddr,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let server = ProxyServer::new(address, ctx, cancel.clone());
    (address, cancel, tokio::spawn(server.run(listener)))
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 16384);
        bytes.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(bytes).unwrap()
}

async fn connect_tunnel(address: SocketAddr, upstream_port: u16) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(format!("CONNECT localhost:{upstream_port} HTTP/1.1\r\nHost: localhost:{upstream_port}\r\n\r\n").as_bytes()).await.unwrap();
    assert!(read_head(&mut stream).await.starts_with("HTTP/1.1 200"));
    stream
}

trait TestStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> TestStream for T {}

async fn echo_websocket(mut stream: impl TestStream) {
    let headers = read_head(&mut stream).await;
    assert!(headers.to_ascii_lowercase().contains("upgrade: websocket"));
    assert!(!headers.to_ascii_lowercase().contains("proxy-authorization"));
    stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
    let mut frame = [0; 8];
    stream.read_exact(&mut frame).await.unwrap();
    assert_eq!(frame, [0x81, 0x82, 0, 0, 0, 0, b'h', b'i']);
    stream.write_all(b"\x81\x02ok").await.unwrap();
    let _ = stream.read_u8().await;
}

async fn websocket_round_trip(secure: bool) {
    let (ctx, mut events) = crate::test_support::context(ProxyLimits {
        max_connections: 1,
        ..ProxyLimits::default()
    });
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    let server_config = crate::tls::cert_gen::build_server_config(
        ctx.cert_cache.get_or_generate("localhost").await.unwrap(),
    )
    .unwrap();
    let client_config = ctx.tls_client_config.clone();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        if secure {
            let stream = tokio_rustls::TlsAcceptor::from(server_config)
                .accept(stream)
                .await
                .unwrap();
            echo_websocket(stream).await;
        } else {
            echo_websocket(stream).await;
        }
    });
    let (address, cancel, server) = start_proxy(ctx).await;
    let mut client: Box<dyn TestStream> = if secure {
        let stream = connect_tunnel(address, upstream_port).await;
        Box::new(
            tokio_rustls::TlsConnector::from(client_config)
                .connect(crate::tls::server_name_or_localhost("localhost"), stream)
                .await
                .unwrap(),
        )
    } else {
        Box::new(TcpStream::connect(address).await.unwrap())
    };
    let path = if secure {
        "/chat".into()
    } else {
        format!("http://localhost:{upstream_port}/chat")
    };
    client.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost:{upstream_port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nProxy-Authorization: secret\r\n\r\n").as_bytes()).await.unwrap();
    assert!(read_head(&mut client).await.starts_with("HTTP/1.1 101"));
    client
        .write_all(b"\x81\x82\x00\x00\x00\x00hi")
        .await
        .unwrap();
    let mut reply = [0; 4];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"\x81\x02ok");
    let captured = loop {
        if let Some(ProxyToUi::RequestCaptured(request)) = events.recv().await {
            break request;
        }
    };
    assert_eq!(
        captured.uri,
        format!(
            "{}://localhost:{upstream_port}/chat",
            if secure { "wss" } else { "ws" }
        )
    );

    // A second client can enter the listen backlog, but cannot bypass the upgraded
    // connection's permit and complete a request while the first remains open.
    let mut second = TcpStream::connect(address).await.unwrap();
    second
        .write_all(b"GET http://localhost:1/ HTTP/1.1\r\nHost: localhost:1\r\n\r\n")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), second.read_u8())
            .await
            .is_err()
    );
    drop(client);
    upstream_task.await.unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert!(TcpListener::bind(address).await.is_ok());
}

#[tokio::test]
async fn websocket_upgrades_and_connection_limits_work_over_http() {
    within(websocket_round_trip(false)).await;
}

#[tokio::test]
async fn websocket_upgrades_and_connection_limits_work_over_https() {
    within(websocket_round_trip(true)).await;
}

#[tokio::test]
async fn saturated_connect_tunnels_do_not_prevent_shutdown_or_hold_the_listener() {
    within(async {
        let (ctx, _events) = crate::test_support::context(ProxyLimits {
            max_connections: 1,
            ..ProxyLimits::default()
        });
        let (address, cancel, server) = start_proxy(ctx).await;
        let first = connect_tunnel(address, 443).await; // Deliberately never start TLS.
        let mut second = TcpStream::connect(address).await.unwrap();
        second
            .write_all(b"GET http://localhost:1/ HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), second.read_u8())
                .await
                .is_err()
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
        assert!(TcpListener::bind(address).await.is_ok());
        drop(first);
    })
    .await;
}

#[tokio::test]
async fn edited_http_bodies_have_correct_framing_and_preserve_header_octets() {
    within(async {
        let (ctx, _events) = crate::test_support::context(ProxyLimits::default());
        let mut rule = crate::rules::Rule::new("grow bodies".into());
        rule.scope = crate::rules::RuleScope::Body;
        rule.set_pattern("foo".into()).unwrap();
        rule.replacement = "longer".into();
        ctx.rules.write().push(rule);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(
                |request: hyper::Request<hyper::body::Incoming>| async move {
                    assert_eq!(request.headers()["x-octets"].as_bytes(), b"\xff\x80");
                    assert_eq!(
                        request.into_body().collect().await.unwrap().to_bytes(),
                        b"longer".as_slice()
                    );
                    Ok::<_, Infallible>(
                        hyper::Response::builder()
                            .header("content-length", "3")
                            .header(
                                "x-octets",
                                http::HeaderValue::from_bytes(b"\xff\x80").unwrap(),
                            )
                            .body(Full::new(Bytes::from_static(b"foo")))
                            .unwrap(),
                    )
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        let (address, cancel, server) = start_proxy(ctx).await;
        let stream = TcpStream::connect(address).await.unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        let driver = tokio::spawn(connection);
        let request = hyper::Request::builder()
            .method("POST")
            .uri(format!("http://{upstream_address}/"))
            .header("host", upstream_address.to_string())
            .header("content-length", "3")
            .header(
                "x-octets",
                http::HeaderValue::from_bytes(b"\xff\x80").unwrap(),
            )
            .body(Full::new(Bytes::from_static(b"foo")))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.headers()["content-length"], "6");
        assert_eq!(response.headers()["x-octets"].as_bytes(), b"\xff\x80");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            b"longer".as_slice()
        );
        drop(sender);
        driver.abort();
        cancel.cancel();
        server.await.unwrap().unwrap();
        upstream.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn oversized_request_rewrite_returns_502_and_reports_the_error() {
    within(async {
        let (ctx, mut events) = crate::test_support::context(ProxyLimits {
            max_body_bytes: 8,
            ..ProxyLimits::default()
        });
        let mut rule = crate::rules::Rule::new("expand body".into());
        rule.scope = crate::rules::RuleScope::Body;
        rule.set_pattern("a".into()).unwrap();
        rule.replacement = "a".repeat(16);
        ctx.rules.write().push(rule);
        let (address, cancel, server) = start_proxy(ctx).await;
        let stream = TcpStream::connect(address).await.unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        let driver = tokio::spawn(connection);
        let response = sender
            .send_request(
                hyper::Request::builder()
                    .method("POST")
                    .uri("http://localhost:1/")
                    .header("host", "localhost:1")
                    .body(Full::new(Bytes::from_static(b"a")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 502);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("Rewritten value exceeds configured limit")
        );
        assert!(matches!(
            events.recv().await,
            Some(ProxyToUi::RequestCaptured(_))
        ));
        assert!(matches!(
            events.recv().await,
            Some(ProxyToUi::RequestError(_, _))
        ));
        drop(sender);
        driver.abort();
        cancel.cancel();
        server.await.unwrap().unwrap();
    })
    .await;
}

#[tokio::test]
async fn grpc_response_arrives_before_client_closes_request_stream() {
    within(async {
        let (ctx, mut events) = crate::test_support::context(ProxyLimits::default());
        let config = crate::tls::cert_gen::build_server_config(
            ctx.cert_cache.get_or_generate("localhost").await.unwrap(),
        )
        .unwrap();
        let client_config = ctx.tls_h2_client_config.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let stream = tokio_rustls::TlsAcceptor::from(config)
                .accept(stream)
                .await
                .unwrap();
            let service = service_fn(
                |mut request: hyper::Request<hyper::body::Incoming>| async move {
                    assert_eq!(request.headers()["te"], "trailers");
                    let first = request
                        .body_mut()
                        .frame()
                        .await
                        .unwrap()
                        .unwrap()
                        .into_data()
                        .unwrap();
                    let mut trailers = http::HeaderMap::new();
                    trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
                    let frames = futures::stream::iter([
                        Ok::<_, Infallible>(http_body::Frame::data(first)),
                        Ok(http_body::Frame::trailers(trailers)),
                    ]);
                    Ok::<_, Infallible>(
                        hyper::Response::builder()
                            .header("content-type", "application/grpc")
                            .body(StreamBody::new(frames))
                            .unwrap(),
                    )
                },
            );
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        let (address, cancel, server) = start_proxy(ctx).await;
        let stream = connect_tunnel(address, port).await;
        let stream = tokio_rustls::TlsConnector::from(client_config)
            .connect(crate::tls::server_name_or_localhost("localhost"), stream)
            .await
            .unwrap();
        let (body_sender, body_receiver) =
            mpsc::channel::<Result<http_body::Frame<Bytes>, Infallible>>(1);
        let stream_body = StreamBody::new(Box::pin(futures::stream::unfold(
            body_receiver,
            |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) },
        )));
        let (mut sender, connection) =
            hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                .handshake(TokioIo::new(stream))
                .await
                .unwrap();
        let driver = tokio::spawn(connection);
        let frame = Bytes::from_static(b"\x00\x00\x00\x00\x02hi");
        body_sender
            .send(Ok(http_body::Frame::data(frame.clone())))
            .await
            .unwrap();
        let request = hyper::Request::builder()
            .method("POST")
            .uri(format!("https://localhost:{port}/test.Service/Call"))
            .header("content-type", "application/grpc")
            .body(stream_body)
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        let collected = response.into_body().collect().await.unwrap();
        assert_eq!(collected.trailers().unwrap()["grpc-status"], "0");
        assert_eq!(collected.to_bytes(), frame);
        // body_sender is deliberately alive until after receiving the response.
        drop(body_sender);
        let mut captured = false;
        let mut saw_client_frame = false;
        loop {
            match events.recv().await.unwrap() {
                ProxyToUi::RequestCaptured(request) => {
                    assert_eq!(
                        request.uri,
                        format!("https://localhost:{port}/test.Service/Call")
                    );
                    captured = true;
                }
                ProxyToUi::GrpcFrame(_, message) => {
                    assert!(captured, "frame arrived before its history entry");
                    if message.direction == crate::http::models::GrpcDirection::ClientToServer {
                        saw_client_frame = true;
                    }
                }
                ProxyToUi::GrpcTrailers(_, _) => break,
                _ => {}
            }
        }
        assert!(saw_client_frame);
        drop(sender);
        driver.abort();
        cancel.cancel();
        server.await.unwrap().unwrap();
        upstream.await.unwrap();
    })
    .await;
}
