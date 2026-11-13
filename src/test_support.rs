use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::app::{App, AppInit};
use crate::channel::ProxyToUi;
use crate::http::models::*;
use crate::proxy::{ProxyContext, ProxyLimits};
use crate::proxy::{intercept::InterceptState, scope::Scope};
use crate::tls::{ca::CertificateAuthority, cert_cache::CertCache};

pub fn request() -> RequestData {
    RequestData {
        id: RequestId::next(),
        method: "POST".into(),
        uri: "http://example.com/".into(),
        host: "example.com".into(),
        version: HttpVersion::Http11,
        headers: vec![],
        body: Bytes::new(),
        is_tls: false,
        is_grpc: false,
        timestamp: std::time::SystemTime::now(),
    }
}

pub fn response() -> ResponseData {
    ResponseData {
        status: 200,
        reason: "OK".into(),
        version: HttpVersion::Http11,
        headers: vec![],
        body: Bytes::new(),
        trailers: vec![],
        duration: std::time::Duration::ZERO,
        timing: None,
    }
}

pub fn entry() -> HistoryEntry {
    HistoryEntry {
        request: request(),
        response: Some(response()),
        state: EntryState::Complete,
        error_message: None,
        ws_messages: vec![],
        grpc_messages: vec![],
        findings: vec![],
        request_body_truncated: false,
    }
}

/// Ephemeral CA and trust roots: tests never touch ~/.crowbar or disable verification.
pub fn context(limits: ProxyLimits) -> (ProxyContext, mpsc::Receiver<ProxyToUi>) {
    let ca = Arc::new(CertificateAuthority::generate().unwrap());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.ca_cert.der().clone()).unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut h2_config = config.clone();
    h2_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let (sender, receiver) = mpsc::channel(1024);
    let mut ctx = ProxyContext::new(
        sender,
        Arc::new(CertCache::new(ca)),
        Arc::new(InterceptState::new(false)),
        Arc::new(Scope::new(vec![])),
        Arc::new(parking_lot::RwLock::new(vec![])),
        limits,
    );
    ctx.tls_client_config = Arc::new(config);
    ctx.tls_h2_client_config = Arc::new(h2_config);
    (ctx, receiver)
}

pub fn app() -> (App, mpsc::Receiver<ProxyToUi>) {
    let (ctx, receiver) = context(ProxyLimits::default());
    (
        App::new(AppInit {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            intercept_state: ctx.intercept,
            scope: ctx.scope,
            rules: ctx.rules,
            ui_tx: ctx.ui_tx,
            editor_mode: crate::editor::EditorMode::Default,
            allow_remote: false,
            proxy_limits: ctx.limits,
            max_history_entries: 10,
        }),
        receiver,
    )
}
