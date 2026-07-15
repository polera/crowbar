use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Deserialize;
use tracing::debug;

use crate::editor::EditorMode;

#[derive(Parser, Debug)]
#[command(name = "crowbar", about = "TUI web security testing proxy")]
struct Cli {
    #[arg(short, long, help = "Proxy bind address (default: 127.0.0.1:8080)")]
    pub bind: Option<SocketAddr>,

    #[arg(
        long,
        help = "Allow binding the unauthenticated proxy to a non-loopback address"
    )]
    pub allow_remote: bool,

    #[arg(long, default_value_t = 10 * 1024 * 1024, help = "Maximum buffered HTTP body size in bytes")]
    pub max_body_bytes: usize,

    #[arg(long, default_value_t = 16 * 1024 * 1024, help = "Maximum WebSocket frame size in bytes")]
    pub max_ws_frame_bytes: usize,

    #[arg(
        long,
        default_value_t = 10_000,
        help = "Maximum number of requests retained in history"
    )]
    pub max_history_entries: usize,

    #[arg(
        long,
        default_value_t = 128,
        help = "Maximum concurrent client connections"
    )]
    pub max_connections: usize,

    #[arg(long, help = "Start with intercept mode enabled")]
    pub intercept: bool,

    #[arg(short, long, help = "Path to config file")]
    pub config: Option<PathBuf>,

    #[arg(
        short,
        long,
        help = "Scope pattern (e.g. *.example.com). Repeat for multiple."
    )]
    pub scope: Vec<String>,

    #[arg(short, long, help = "Load a saved session file")]
    pub load: Option<PathBuf>,

    #[arg(long, help = "Editor mode: 'default' or 'vim'")]
    pub editor_mode: Option<String>,

    #[arg(
        long,
        help = "Directory of .proto files for gRPC decoding. Repeat for multiple."
    )]
    pub proto_dir: Vec<PathBuf>,

    #[arg(long, help = "Extra .proto import/include path. Repeat for multiple.")]
    pub proto_include: Vec<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        name = "ca-export",
        about = "Export the CA certificate for browser/OS trust store installation"
    )]
    CaExport {
        #[arg(help = "Output file path (omit to print to stdout)")]
        output: Option<PathBuf>,
    },
    #[command(name = "import", about = "Import a HAR file into a crowbar session")]
    Import {
        #[arg(help = "Path to HAR file")]
        input: PathBuf,
        #[arg(
            short,
            long,
            help = "Output session name (default: derived from input filename)"
        )]
        name: Option<String>,
    },
    #[command(name = "rules-export", about = "Export rules to a JSON file")]
    RulesExport {
        #[arg(help = "Output file path (default: ~/.crowbar/rules/rules-<timestamp>.json)")]
        output: Option<PathBuf>,
    },
    #[command(name = "rules-validate", about = "Validate a rules JSON file")]
    RulesValidate {
        #[arg(help = "Path to rules JSON file")]
        input: PathBuf,
    },
}

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    bind: Option<String>,
    allow_remote: Option<bool>,
    intercept: Option<bool>,
    scope: Option<Vec<String>>,
    editor_mode: Option<EditorMode>,
    proto_dir: Option<Vec<PathBuf>>,
    proto_include: Option<Vec<PathBuf>>,
}

#[derive(Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub allow_remote: bool,
    pub limits: crate::proxy::ProxyLimits,
    pub max_history_entries: usize,
    pub intercept: bool,
    pub scope: Vec<String>,
    pub load: Option<PathBuf>,
    pub command: Option<Command>,
    pub editor_mode: EditorMode,
    pub proto_dir: Vec<PathBuf>,
    pub proto_include: Vec<PathBuf>,
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
                        eprintln!("Warning: failed to parse {}: {}", config_path.display(), e);
                        FileConfig::default()
                    }
                },
                Err(e) => {
                    eprintln!("Warning: failed to read {}: {}", config_path.display(), e);
                    FileConfig::default()
                }
            }
        } else {
            FileConfig::default()
        };

        let default_bind: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let file_bind = file_config.bind.and_then(|s| s.parse::<SocketAddr>().ok());

        let scope = if !cli.scope.is_empty() {
            cli.scope
        } else {
            file_config.scope.unwrap_or_default()
        };

        let cli_editor_mode = cli
            .editor_mode
            .and_then(|s| match s.to_lowercase().as_str() {
                "vim" => Some(EditorMode::Vim),
                "default" => Some(EditorMode::Default),
                _ => None,
            });

        let editor_mode = cli_editor_mode
            .or(file_config.editor_mode)
            .unwrap_or_default();

        let proto_dir = if !cli.proto_dir.is_empty() {
            cli.proto_dir
        } else {
            file_config.proto_dir.unwrap_or_default()
        };

        let proto_include = if !cli.proto_include.is_empty() {
            cli.proto_include
        } else {
            file_config.proto_include.unwrap_or_default()
        };

        Config {
            bind: cli.bind.or(file_bind).unwrap_or(default_bind),
            allow_remote: cli.allow_remote || file_config.allow_remote.unwrap_or(false),
            limits: crate::proxy::ProxyLimits {
                max_body_bytes: cli.max_body_bytes.max(1),
                max_ws_frame_bytes: cli.max_ws_frame_bytes.max(1),
                max_connections: cli.max_connections.max(1),
            },
            max_history_entries: cli.max_history_entries.max(1),
            intercept: cli.intercept || file_config.intercept.unwrap_or(false),
            scope,
            load: cli.load,
            command: cli.command,
            editor_mode,
            proto_dir,
            proto_include,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_bind_does_not_implicitly_enable_remote_access() {
        let cli = Cli::try_parse_from(["crowbar", "--bind", "0.0.0.0:9090"]).unwrap();

        assert_eq!(cli.bind.unwrap(), "0.0.0.0:9090".parse().unwrap());
        assert!(!cli.allow_remote);
    }

    #[test]
    fn remote_access_requires_explicit_cli_opt_in() {
        let cli =
            Cli::try_parse_from(["crowbar", "--bind", "0.0.0.0:9090", "--allow-remote"]).unwrap();

        assert!(cli.allow_remote);
    }

    #[test]
    fn resource_limit_flags_are_parsed_independently() {
        let cli = Cli::try_parse_from([
            "crowbar",
            "--max-body-bytes",
            "1024",
            "--max-ws-frame-bytes",
            "2048",
            "--max-history-entries",
            "50",
            "--max-connections",
            "8",
        ])
        .unwrap();

        assert_eq!(cli.max_body_bytes, 1024);
        assert_eq!(cli.max_ws_frame_bytes, 2048);
        assert_eq!(cli.max_history_entries, 50);
        assert_eq!(cli.max_connections, 8);
    }

    #[test]
    fn file_config_can_explicitly_allow_remote_access() {
        let config: FileConfig = toml::from_str(
            r#"
                bind = "192.0.2.10:8080"
                allow_remote = true
            "#,
        )
        .unwrap();

        assert_eq!(config.bind.as_deref(), Some("192.0.2.10:8080"));
        assert_eq!(config.allow_remote, Some(true));
    }
}
