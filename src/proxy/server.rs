use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::proxy::ProxyContext;
use crate::proxy::handler::ProxyHandler;

pub struct ProxyServer {
    bind_addr: SocketAddr,
    ctx: ProxyContext,
    cancel: CancellationToken,
}

impl ProxyServer {
    pub fn new(bind_addr: SocketAddr, ctx: ProxyContext, cancel: CancellationToken) -> Self {
        Self {
            bind_addr,
            ctx,
            cancel,
        }
    }

    pub async fn run(self, listener: TcpListener) -> anyhow::Result<()> {
        info!("Proxy listening on {}", self.bind_addr);

        let handler = Arc::new(ProxyHandler::new(self.ctx));
        let connections = Arc::new(Semaphore::new(handler.limits().max_connections));

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, client_addr) = result?;
                    let handler = handler.clone();
                    let permit = connections
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| anyhow::anyhow!("connection limiter closed"))?;

                    tokio::spawn(async move {
                        let _permit = permit;
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req| {
                            let handler = handler.clone();
                            async move { handler.handle(req, client_addr).await }
                        });

                        if let Err(e) = http1::Builder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .timer(TokioTimer::new())
                            .header_read_timeout(Duration::from_secs(15))
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
