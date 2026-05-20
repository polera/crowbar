use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::proxy::handler::ProxyHandler;
use crate::proxy::ProxyContext;

pub struct ProxyServer {
    bind_addr: SocketAddr,
    ctx: ProxyContext,
    cancel: CancellationToken,
}

impl ProxyServer {
    pub fn new(
        bind_addr: SocketAddr,
        ctx: ProxyContext,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            bind_addr,
            ctx,
            cancel,
        }
    }

    pub async fn run(self, listener: TcpListener) -> anyhow::Result<()> {
        info!("Proxy listening on {}", self.bind_addr);

        let handler = Arc::new(ProxyHandler::new(self.ctx));

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, client_addr) = result?;
                    let handler = handler.clone();

                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req| {
                            let handler = handler.clone();
                            async move { handler.handle(req, client_addr).await }
                        });

                        if let Err(e) = http1::Builder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .serve_connection(io, svc)
                            .with_upgrades()
                            .await
                            && !e.to_string().contains("connection closed")
                        {
                            error!("Connection error from {}: {}", client_addr, e);
                        }
                    });
                }
                _ = self.cancel.cancelled() => {
                    info!("Proxy server shutting down");
                    break;
                }
            }
        }

        Ok(())
    }
}
