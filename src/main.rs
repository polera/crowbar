mod app;
mod channel;
mod config;
mod event;
mod http;
mod proxy;
mod rules;
mod scanning;
mod tls;
mod tui;

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::channel::ProxyToUi;
use crate::config::Config;
use crate::event::{AppEvent, EventLoop};
use crate::proxy::intercept::InterceptState;
use crate::proxy::scope::Scope;
use crate::proxy::server::ProxyServer;
use crate::rules::SharedRules;
use crate::tls::ca::CertificateAuthority;
use crate::tls::cert_cache::CertCache;
use crate::tui::terminal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    if let Some(cmd) = config.command {
        return handle_command(cmd);
    }

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
    let scope = Arc::new(Scope::new(config.scope));

    let rules: SharedRules = Arc::new(std::sync::RwLock::new(Vec::new()));

    let mut cancel = CancellationToken::new();

    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<ProxyToUi>();
    let app_tx = ui_tx.clone();

    let listener = TcpListener::bind(config.bind).await?;
    let server = ProxyServer::new(
        config.bind,
        ui_tx,
        cert_cache.clone(),
        intercept.clone(),
        scope.clone(),
        rules.clone(),
        cancel.clone(),
    );
    tokio::spawn(async move {
        if let Err(e) = server.run(listener).await {
            tracing::error!("Proxy server error: {}", e);
        }
    });

    let mut tui = terminal::init()?;
    let mut app = App::new(config.bind, intercept.clone(), scope.clone(), rules.clone(), app_tx);

    if let Some(path) = config.load {
        app.load_session(&path);
    }

    let mut events = EventLoop::new(ui_rx);

    let result = run_app(&mut tui, &mut app, &mut events, cert_cache, &mut cancel).await;

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
    cert_cache: Arc<CertCache>,
    cancel: &mut CancellationToken,
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

        if let Some(new_addr) = app.pending_rebind.take() {
            match TcpListener::bind(new_addr).await {
                Ok(listener) => {
                    cancel.cancel();
                    *cancel = CancellationToken::new();

                    let server = ProxyServer::new(
                        new_addr,
                        app.ui_tx.clone(),
                        cert_cache.clone(),
                        app.intercept_state.clone(),
                        app.scope.clone(),
                        app.rules.clone(),
                        cancel.clone(),
                    );
                    tokio::spawn(async move {
                        if let Err(e) = server.run(listener).await {
                            tracing::error!("Proxy server error: {}", e);
                        }
                    });

                    app.bind_addr = new_addr;
                    app.status_message = Some((
                        format!("Proxy restarted on {}", new_addr),
                        std::time::Instant::now(),
                    ));
                }
                Err(e) => {
                    app.status_message = Some((
                        format!("Failed to bind {}: {}", new_addr, e),
                        std::time::Instant::now(),
                    ));
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_command(cmd: crate::config::Command) -> anyhow::Result<()> {
    match cmd {
        crate::config::Command::CaExport { output } => {
            let ca_dir = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
                .join(".crowbar");
            let cert_path = ca_dir.join("ca.pem");

            if !cert_path.exists() {
                eprintln!("No CA certificate found. Run crowbar once to generate it.");
                std::process::exit(1);
            }

            let pem = std::fs::read_to_string(&cert_path)?;

            match output {
                Some(path) => {
                    std::fs::write(&path, &pem)?;
                    eprintln!("CA certificate written to {}", path.display());
                    eprintln!();
                    eprintln!("To trust this certificate:");
                    eprintln!("  macOS:   security add-trusted-cert -d -r trustRoot -k ~/Library/Keychains/login.keychain-db {}", path.display());
                    eprintln!("  Linux:   sudo cp {} /usr/local/share/ca-certificates/crowbar.crt && sudo update-ca-certificates", path.display());
                    eprintln!("  Firefox: Settings > Privacy & Security > Certificates > Import");
                }
                None => {
                    print!("{}", pem);
                }
            }
            Ok(())
        }
        crate::config::Command::Import { input, name } => {
            let entries = crate::http::import::load_file(&input)?;
            let session_name = name.unwrap_or_else(|| {
                input
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
            let path = crate::http::session::save(&entries, &session_name)?;
            eprintln!(
                "Imported {} entries from {} -> {}",
                entries.len(),
                input.display(),
                path.display()
            );
            eprintln!("Load with: crowbar --load {}", path.display());
            Ok(())
        }
    }
}
