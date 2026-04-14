// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `tldr-daemon` binary entry point.
//!
//! Parses CLI arguments, initialises `tracing`, loads (or defaults) the config,
//! and delegates to [`tldr::daemon::run_daemon`].
//!
//! If the config file is absent or unreadable the daemon starts in degraded mode
//! with an empty default config.  A warning is printed to stderr with the URL
//! to open the web UI setup wizard.

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "tldr-daemon", about = "Mattermost chat summarisation daemon")]
struct Args {
    /// Path to the config file
    #[arg(long, default_value = "~/.config/tldr/config.toml")]
    config: String,

    /// Enable debug-level logging (overrides RUST_LOG)
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let default_filter = if args.debug { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .with_target(false)
        .init();

    let config_path = tldr::config::expand_tilde(&args.config);
    let config = match tldr::config::Config::load(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "Warning: could not load config from {}: {e}\n\
                 Starting in degraded mode. Open http://127.0.0.1:8765 in your browser to configure.",
                config_path.display(),
            );
            tldr::config::Config::default()
        }
    };

    tldr::daemon::run_daemon(config, config_path).await
}
