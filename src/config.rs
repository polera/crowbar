use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
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

    #[arg(short, long, help = "Scope pattern (e.g. *.example.com). Repeat for multiple.")]
    pub scope: Vec<String>,

    #[arg(short, long, help = "Load a saved session file")]
    pub load: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(name = "ca-export", about = "Export the CA certificate for browser/OS trust store installation")]
    CaExport {
        #[arg(help = "Output file path (omit to print to stdout)")]
        output: Option<PathBuf>,
    },
    #[command(name = "import", about = "Import a HAR file into a crowbar session")]
    Import {
        #[arg(help = "Path to HAR file")]
        input: PathBuf,
        #[arg(short, long, help = "Output session name (default: derived from input filename)")]
        name: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    bind: Option<String>,
    intercept: Option<bool>,
    scope: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub intercept: bool,
    pub scope: Vec<String>,
    pub load: Option<PathBuf>,
    pub command: Option<Command>,
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

        let scope = if !cli.scope.is_empty() {
            cli.scope
        } else {
            file_config.scope.unwrap_or_default()
        };

        Config {
            bind: cli.bind.or(file_bind).unwrap_or(default_bind),
            intercept: cli.intercept || file_config.intercept.unwrap_or(false),
            scope,
            load: cli.load,
            command: cli.command,
        }
    }
}
