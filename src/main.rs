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
        #[arg(short, long, default_value = "nanodns.json")]
        config: PathBuf,
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(short, long, default_value_t = 53)]
        port: u16,
        #[arg(long, default_value = "INFO")]
        log_level: String,
        #[arg(long)]
        no_cache: bool,
    },
    /// Write an example config file
    Init {
        output: Option<PathBuf>,
    },
    /// Validate a config file
    Check {
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config, host, port, log_level, no_cache } => {
            let filter = format!("nanodns={}", log_level.to_lowercase());
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)),
                )
                .init();

            info!("NanoDNS v{} starting", env!("CARGO_PKG_VERSION"));
            let cfg = config::load(&config)?;
            server::run(cfg, host, port, no_cache, config).await?;
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
                    println!("  Records : {}", cfg.records.len());
                    println!("  Rewrites: {}", cfg.rewrites.len());
                    println!("  Zones   : {}", cfg.zones.len());
                    println!("  Server  : {}:{} upstream={:?}", cfg.server.host, cfg.server.port, cfg.server.upstream);
                    if let Some(mp) = cfg.server.mgmt_port { println!("  Mgmt    : :{}", mp); }
                    if !cfg.server.peers.is_empty() { println!("  Peers   : {:?}", cfg.server.peers); }
                }
                Err(e) => { eprintln!("✗ Config invalid: {}", e); std::process::exit(1); }
            }
        }
    }
    Ok(())
}
