use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use serde::Deserialize;
use tracing::debug;

#[derive(Parser, Debug)]
#[command(name = "crowbar", about = "TUI web security testing proxy")]
struct Cli {
    #[arg(short, long, help = "Proxy bind address (default: 127.0.0.1:8080)")]
    pub bind: Option<SocketAddr>,

    #[arg(long, help = "Start with intercept mode enabled")]
    pub intercept: bool,

    #[arg(short, long, help = "Path to config file")]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    bind: Option<String>,
    intercept: Option<bool>,
}

#[derive(Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub intercept: bool,
}

impl Config {
    pub fn parse() -> Self {
        let cli = Cli::parse();

        let config_path = cli.config.unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".crowbar")
                .join("config.toml")
        });

        let file_config = if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(contents) => match toml::from_str::<FileConfig>(&contents) {
                    Ok(fc) => {
                        debug!("Loaded config from {}", config_path.display());
                        fc
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to parse {}: {}",
                            config_path.display(),
                            e
                        );
                        FileConfig::default()
                    }
                },
                Err(e) => {
                    eprintln!(
                        "Warning: failed to read {}: {}",
                        config_path.display(),
                        e
                    );
                    FileConfig::default()
                }
            }
        } else {
            FileConfig::default()
        };

        let default_bind: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let file_bind = file_config
            .bind
            .and_then(|s| s.parse::<SocketAddr>().ok());

        Config {
            bind: cli.bind.or(file_bind).unwrap_or(default_bind),
            intercept: cli.intercept || file_config.intercept.unwrap_or(false),
        }
    }
}
