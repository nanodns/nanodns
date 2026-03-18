mod cache;
mod config;
mod dns;
mod error;
mod mgmt;
mod server;
mod sync;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(
    name = "nanodns",
    version = "1.0.5",
    about = "A lightweight DNS server for internal networks"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the DNS server
    Start {
        /// Path to config file
        #[arg(short, long, default_value = "nanodns.json")]
        config: PathBuf,

        /// Override DNS bind host (default: read from config)
        #[arg(long)]
        host: Option<String>,

        /// Override DNS port (default: read from config)
        #[arg(short, long)]
        port: Option<u16>,

        /// Override management API host (default: read from config)
        #[arg(long)]
        mgmt_host: Option<String>,

        /// Override management API port — 0 = disabled (default: read from config)
        /// Useful for running multiple nodes on one machine:
        ///   nanodns start --port 5353 --mgmt-port 9053 --config node1.json
        ///   nanodns start --port 5354 --mgmt-port 9054 --config node2.json
        #[arg(long)]
        mgmt_port: Option<u16>,

        /// Override log level: TRACE, DEBUG, INFO, WARN, ERROR
        #[arg(long)]
        log_level: Option<String>,

        /// Disable DNS response cache
        #[arg(long)]
        no_cache: bool,
    },
    /// Write an example config file
    Init { output: Option<PathBuf> },
    /// Validate a config file and print a summary
    Check { config: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            config,
            host,
            port,
            mgmt_host,
            mgmt_port,
            log_level,
            no_cache,
        } => {
            // Load config first — CLI flags override config file values
            let mut cfg = config::load(&config)?;

            // CLI flag > config file: apply overrides
            if let Some(h) = host {
                cfg.server.host = h;
            }
            if let Some(p) = port {
                cfg.server.port = p;
            }
            if let Some(mh) = mgmt_host {
                cfg.server.mgmt_host = mh;
            }
            if let Some(mp) = mgmt_port {
                cfg.server.mgmt_port = mp;
            }

            let effective_log = log_level.unwrap_or_else(|| cfg.server.log_level.clone());

            // Build tracing filter
            let filter = if cfg.server.log_queries {
                format!(
                    "nanodns={},nanodns::dns::resolver=info",
                    effective_log.to_lowercase()
                )
            } else {
                format!("nanodns={}", effective_log.to_lowercase())
            };

            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)),
                )
                .init();

            info!("NanoDNS v{} starting", env!("CARGO_PKG_VERSION"));
            info!(
                "bind={}:{} | mgmt={}:{} | log={}",
                cfg.server.host,
                cfg.server.port,
                cfg.server.mgmt_host,
                cfg.server.mgmt_port,
                effective_log,
            );

            server::run(cfg, no_cache, config).await?;
        }

        Commands::Init { output } => {
            let path = output.unwrap_or_else(|| PathBuf::from("nanodns.json"));
            config::write_example(&path)?;
            println!("Example config written to {}", path.display());
        }

        Commands::Check { config } => {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .init();
            match config::load(&config) {
                Ok(cfg) => {
                    println!("✓ Config valid: {}", config.display());
                    println!("  Records        : {}", cfg.records.len());
                    println!("  Rewrites       : {}", cfg.rewrites.len());
                    println!("  Zones          : {}", cfg.zones.len());
                    println!("  Bind           : {}:{}", cfg.server.host, cfg.server.port);
                    println!(
                        "  Upstream       : {:?} timeout={}s port={}",
                        cfg.server.upstream, cfg.server.upstream_timeout, cfg.server.upstream_port
                    );
                    println!(
                        "  Cache          : enabled={} ttl={}s size={}",
                        cfg.server.cache_enabled, cfg.server.cache_ttl, cfg.server.cache_size
                    );
                    println!("  Hot-reload     : {}", cfg.server.hot_reload);
                    println!("  Config version : {}", cfg.server.config_version);
                    if cfg.server.mgmt_port > 0 {
                        println!(
                            "  Mgmt API       : {}:{}",
                            cfg.server.mgmt_host, cfg.server.mgmt_port
                        );
                    } else {
                        println!("  Mgmt API       : disabled");
                    }
                    if !cfg.server.peers.is_empty() {
                        println!("  Peers          : {:?}", cfg.server.peers);
                    }
                }
                Err(e) => {
                    eprintln!("✗ Config invalid: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
