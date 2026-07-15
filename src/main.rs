mod app;
mod channel;
mod config;
mod editor;
mod event;
mod fs_security;
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

use crate::app::{App, AppInit};
use crate::channel::ProxyToUi;
use crate::config::Config;
use crate::event::{AppEvent, EventLoop};
use crate::proxy::ProxyContext;
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

    let log_dir = dirs::home_dir().unwrap_or_default().join(".crowbar");
    crate::fs_security::harden_private_tree(&log_dir)?;

    if let Some(cmd) = config.command {
        return handle_command(cmd);
    }

    if !config.bind.ip().is_loopback() && !config.allow_remote {
        anyhow::bail!(
            "refusing non-loopback bind {}; pass --allow-remote after securing network access",
            config.bind
        );
    }

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
    crate::fs_security::harden_private_tree(&log_dir)?;

    if !config.proto_dir.is_empty() {
        match crate::http::proto_schema::init(&config.proto_dir, &config.proto_include) {
            Ok(count) => info!(
                "Loaded {} protobuf message type(s) from {:?}",
                count, config.proto_dir
            ),
            Err(e) => eprintln!("Warning: gRPC schema disabled: {e}"),
        }
    }

    let ca = CertificateAuthority::load_or_generate()?;
    let cert_cache = Arc::new(CertCache::new(Arc::new(ca)));
    let intercept = Arc::new(InterceptState::new(config.intercept));
    let scope = Arc::new(Scope::new(config.scope));

    let rules: SharedRules = Arc::new(parking_lot::RwLock::new(Vec::new()));

    let mut cancel = CancellationToken::new();

    let (ui_tx, ui_rx) = mpsc::channel::<ProxyToUi>(1_024);
    let app_tx = ui_tx.clone();

    let bound = {
        let mut addr = config.bind;
        let base_port = addr.port();
        let mut result = None;
        for i in 0..25 {
            addr.set_port(base_port + i);
            match TcpListener::bind(addr).await {
                Ok(l) => {
                    if i > 0 {
                        info!("Port {} in use, bound to {} instead", base_port, addr);
                    }
                    result = Some((l, addr));
                    break;
                }
                Err(e) => {
                    info!("Failed to bind {}: {}", addr, e);
                }
            }
        }
        result
    };

    let proxy_running = bound.is_some();
    let bind_addr = bound.as_ref().map(|(_, a)| *a).unwrap_or(config.bind);

    if let Some((listener, _)) = bound {
        let ctx = ProxyContext {
            ui_tx,
            cert_cache: cert_cache.clone(),
            intercept: intercept.clone(),
            scope: scope.clone(),
            rules: rules.clone(),
            limits: config.limits,
        };
        let server = ProxyServer::new(bind_addr, ctx, cancel.clone());
        tokio::spawn(async move {
            if let Err(e) = server.run(listener).await {
                tracing::error!("Proxy server error: {}", e);
            }
        });
    } else {
        info!(
            "Could not bind to ports {}-{}; starting without proxy",
            config.bind.port(),
            config.bind.port() + 24,
        );
    }

    let mut tui = terminal::init()?;
    let mut app = App::new(AppInit {
        bind_addr,
        intercept_state: intercept.clone(),
        scope: scope.clone(),
        rules: rules.clone(),
        ui_tx: app_tx,
        editor_mode: config.editor_mode,
        allow_remote: config.allow_remote,
        proxy_limits: config.limits,
        max_history_entries: config.max_history_entries,
    });
    app.proxy_running = proxy_running;
    if !proxy_running {
        app.status_message = Some((
            format!(
                "Could not bind to ports {}-{}. Press 'b' to specify a bind address.",
                config.bind.port(),
                config.bind.port() + 24,
            ),
            std::time::Instant::now(),
        ));
    }

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
        app.prepare_render();
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

                    let ctx = ProxyContext {
                        ui_tx: app.ui_tx.clone(),
                        cert_cache: cert_cache.clone(),
                        intercept: app.intercept_state.clone(),
                        scope: app.scope.clone(),
                        rules: app.rules.clone(),
                        limits: app.proxy_limits,
                    };
                    let server = ProxyServer::new(new_addr, ctx, cancel.clone());
                    tokio::spawn(async move {
                        if let Err(e) = server.run(listener).await {
                            tracing::error!("Proxy server error: {}", e);
                        }
                    });

                    app.bind_addr = new_addr;
                    app.proxy_running = true;
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
                    eprintln!(
                        "  macOS:   security add-trusted-cert -d -r trustRoot -k ~/Library/Keychains/login.keychain-db {}",
                        path.display()
                    );
                    eprintln!(
                        "  Linux:   sudo cp {} /usr/local/share/ca-certificates/crowbar.crt && sudo update-ca-certificates",
                        path.display()
                    );
                    eprintln!("  Firefox: Settings > Privacy & Security > Certificates > Import");
                }
                None => {
                    print!("{}", pem);
                }
            }
            Ok(())
        }
        crate::config::Command::Import { input, name } => {
            let session = crate::http::import::load_file(&input)?;
            let session_name = name.unwrap_or_else(|| {
                input
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            let macro_requests = session.macros.map(|m| m.steps).unwrap_or_default();
            let entry_count = session.entries.len();
            let path = crate::http::session::save(session.entries, macro_requests, &session_name)?;
            eprintln!(
                "Imported {} entries from {} -> {}",
                entry_count,
                input.display(),
                path.display()
            );
            eprintln!("Load with: crowbar --load {}", path.display());
            Ok(())
        }
        crate::config::Command::RulesExport { output } => {
            let template = vec![crate::rules::Rule::new("Example rule".into())];
            match output {
                Some(path) => {
                    crate::rules::persist::save_to(&template, &path)?;
                    eprintln!("Exported rules template to {}", path.display());
                }
                None => {
                    let name = crate::rules::persist::auto_save_name();
                    let path = crate::rules::persist::save(&template, &name)?;
                    eprintln!("Exported rules template to {}", path.display());
                }
            }
            Ok(())
        }
        crate::config::Command::RulesValidate { input } => {
            let rules = crate::rules::persist::load(&input)?;
            eprintln!("Validated {} rules from {}", rules.len(), input.display());
            Ok(())
        }
    }
}
