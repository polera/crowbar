pub mod handler;
pub mod intercept;
pub mod repeater;
pub mod scope;
pub mod server;
pub mod tunnel;
pub mod ws_relay;

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response};
use tokio::sync::mpsc;

use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper_util::rt::TokioIo;
use tracing::debug;

use crate::channel::ProxyToUi;
use crate::http::models::{RequestId, ResponseData};
use crate::proxy::intercept::InterceptState;
use crate::proxy::scope::Scope;
use crate::rules::{self, SharedRules};
use crate::tls::cert_cache::CertCache;

#[derive(Clone)]
pub struct ProxyContext {
    pub ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    pub cert_cache: Arc<CertCache>,
    pub intercept: Arc<InterceptState>,
    pub scope: Arc<Scope>,
    pub rules: SharedRules,
}

pub(crate) fn build_forwarding_request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Bytes,
) -> Request<Full<Bytes>> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (key, value) in headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    builder
        .body(Full::new(body))
        .expect("building forwarding request")
}

pub(crate) fn build_client_response(
    status: u16,
    headers: &[(String, String)],
    body: Bytes,
) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(status);
    for (key, value) in headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    builder.body(Full::new(body)).unwrap()
}

pub(crate) async fn process_h1_response(
    upstream_resp: Response<hyper::body::Incoming>,
    request_id: RequestId,
    start: Instant,
    in_scope: bool,
    shared_rules: &SharedRules,
    ui_tx: &mpsc::UnboundedSender<ProxyToUi>,
) -> Response<Full<Bytes>> {
    let resp_status = upstream_resp.status().as_u16();
    let resp_version = upstream_resp.version().into();
    let mut resp_headers = crate::http::models::extract_headers(upstream_resp.headers());

    let mut resp_body = upstream_resp
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    let duration = start.elapsed();

    rules::apply_response_rules(shared_rules, &mut resp_headers, &mut resp_body);

    let response_data = ResponseData {
        status: resp_status,
        reason: crate::http::models::status_reason(resp_status).to_string(),
        version: resp_version,
        headers: resp_headers.clone(),
        body: resp_body.clone(),
        trailers: Vec::new(),
        duration,
    };

    if in_scope {
        let _ = ui_tx.send(ProxyToUi::ResponseReceived(request_id, response_data));
    }

    build_client_response(resp_status, &resp_headers, resp_body)
}

pub(crate) fn bad_gateway(msg: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(502)
        .body(Full::new(Bytes::from(msg.to_string())))
        .unwrap()
}

pub(crate) async fn forward_h1<IO>(
    io: TokioIo<IO>,
    version: hyper::Version,
    path_and_query: &str,
    request_data: &crate::http::models::RequestData,
    start: Instant,
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
            let _ = ctx.ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("HTTP handshake failed: {}", e),
            ));
            return bad_gateway(&format!("HTTP handshake failed: {}", e));
        }
    };

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("Upstream connection ended: {}", e);
        }
    });

    let mut upstream_req = build_forwarding_request(
        &request_data.method,
        path_and_query,
        &request_data.headers,
        request_data.body.clone(),
    );
    *upstream_req.version_mut() = version;

    let upstream_resp = match sender.send_request(upstream_req).await {
        Ok(resp) => resp,
        Err(e) => {
            let _ = ctx.ui_tx.send(ProxyToUi::RequestError(
                request_id,
                format!("Request failed: {}", e),
            ));
            return bad_gateway(&format!("Upstream request failed: {}", e));
        }
    };

    process_h1_response(
        upstream_resp,
        request_id,
        start,
        in_scope,
        &ctx.rules,
        &ctx.ui_tx,
    )
    .await
}
