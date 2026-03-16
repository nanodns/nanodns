mod config;
mod dns;
mod cache;
mod server;
mod mgmt;
mod sync;
mod error;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "nanodns", version = "1.0.0", about = "A lightweight DNS server for internal networks")]
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

        /// Override bind host (default: read from config file)
        #[arg(long)]
        host: Option<String>,

        /// Override DNS port (default: read from config file)
        #[arg(short, long)]
        port: Option<u16>,

        /// Override log level: TRACE, DEBUG, INFO, WARN, ERROR
        #[arg(long)]
        log_level: Option<String>,

        /// Disable DNS response cache
        #[arg(long)]
        no_cache: bool,
    },
    /// Write an example config file
    Init {
        output: Option<PathBuf>,
    },
    /// Validate a config file and print a summary
    Check {
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config, host, port, log_level, no_cache } => {
            // Load config first — CLI flags are overrides only
            let cfg = config::load(&config)?;

            // CLI flag > config file value
            let effective_host  = host.unwrap_or_else(|| cfg.server.host.clone());
            let effective_port  = port.unwrap_or(cfg.server.port);
            let effective_log   = log_level.unwrap_or_else(|| cfg.server.log_level.clone());

            // Build tracing filter.
            // If log_queries is enabled in config, also enable debug for the server module
            // so per-query log lines appear.
            let filter = if cfg.server.log_queries {
                format!("nanodns={},nanodns::dns::resolver=debug", effective_log.to_lowercase())
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
                "Config: {} | bind={}:{} | log_level={}",
                config.display(), effective_host, effective_port, effective_log
            );

            server::run(cfg, effective_host, effective_port, no_cache, config).await?;
        }

        Commands::Init { output } => {
            let path = output.unwrap_or_else(|| PathBuf::from("nanodns.json"));
            config::write_example(&path)?;
            println!("Example config written to {}", path.display());
        }

        Commands::Check { config } => {
            tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).init();
            match config::load(&config) {
                Ok(cfg) => {
                    println!("✓ Config valid: {}", config.display());
                    println!("  Records  : {}", cfg.records.len());
                    println!("  Rewrites : {}", cfg.rewrites.len());
                    println!("  Zones    : {}", cfg.zones.len());
                    println!(
                        "  Bind     : {}:{}",
                        cfg.server.host, cfg.server.port
                    );
                    println!("  Upstream : {:?}", cfg.server.upstream);
                    println!("  Cache    : enabled={} ttl={}s size={}", cfg.server.cache_enabled, cfg.server.cache_ttl, cfg.server.cache_size);
                    println!("  Hot-reload: {}", cfg.server.hot_reload);
                    if let Some(mp) = cfg.server.mgmt_port {
                        println!("  Mgmt API : :{}", mp);
                    }
                    if !cfg.server.peers.is_empty() {
                        println!("  Peers    : {:?}", cfg.server.peers);
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
