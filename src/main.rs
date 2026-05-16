mod app;
mod channel;
mod config;
mod event;
mod http;
mod proxy;
mod tls;
mod tui;

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::channel::ProxyToUi;
use crate::config::Config;
use crate::event::{AppEvent, EventLoop};
use crate::proxy::intercept::InterceptState;
use crate::proxy::server::ProxyServer;
use crate::tls::ca::CertificateAuthority;
use crate::tls::cert_cache::CertCache;
use crate::tui::terminal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();


    let log_dir = dirs::home_dir().unwrap_or_default().join(".crowbar");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = tracing_appender::rolling::never(&log_dir, "crowbar.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    info!("Starting crowbar proxy on {}", config.bind);

    let ca = CertificateAuthority::load_or_generate()?;
    let cert_cache = Arc::new(CertCache::new(Arc::new(ca)));
    let intercept = Arc::new(InterceptState::new(config.intercept));

    let cancel = CancellationToken::new();

    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<ProxyToUi>();
    let app_tx = ui_tx.clone();

    let server = ProxyServer::new(
        config.bind,
        ui_tx,
        cert_cache,
        intercept.clone(),
        cancel.clone(),
    );
    tokio::spawn(async move {
        if let Err(e) = server.run().await {
            tracing::error!("Proxy server error: {}", e);
        }
    });

    let mut tui = terminal::init()?;
    let mut app = App::new(config.bind, intercept.clone(), app_tx);
    let mut events = EventLoop::new(ui_rx);

    let result = run_app(&mut tui, &mut app, &mut events).await;

    info!("Shutting down");
    cancel.cancel();
    intercept.forward_all();

    terminal::restore()?;

    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }

    Ok(())
}

async fn run_app(
    tui: &mut terminal::Tui,
    app: &mut App,
    events: &mut EventLoop,
) -> anyhow::Result<()> {
    loop {
        tui.draw(|frame| app.render(frame))?;

        match events.next().await {
            Some(AppEvent::Input(event)) => {
                app.handle_event(event);
            }
            Some(AppEvent::Proxy(msg)) => {
                app.handle_proxy_message(msg);
            }
            Some(AppEvent::Tick) => {}
            None => break,
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
