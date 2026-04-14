// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `tldr` command-line client.
//!
//! Talks to a running `tldr-daemon` over HTTP.  Subcommands:
//! - `summarise`  — fetch and print channel summaries
//! - `status`     — check daemon liveness
//! - `clear-state` — wipe watermarks (forces full re-summarise)
//! - `config validate` — validate the local config file
//! - `config set-priority-users` — update the priority-users list
//! - `completions` — emit shell completion script
//! - `install-service` — install + enable the systemd user unit

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use colored::Colorize;

#[derive(Parser)]
#[command(name = "tldr", about = "Mattermost chat summarisation CLI")]
struct Cli {
    /// Daemon URL
    #[arg(long, default_value = "http://127.0.0.1:8765")]
    daemon_url: String,

    /// Path to the config file (used by config subcommands)
    #[arg(long, default_value = "~/.config/tldr/config.toml")]
    config: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise unread channels
    Summarise {
        /// Summarise only this channel (by name)
        #[arg(long)]
        channel: Option<String>,
    },
    /// Check if the daemon is running
    Status,
    /// Clear stored read-state watermarks (forces full re-summarise on next run)
    ClearState {
        /// Clear only the watermark for this channel ID (clears all if omitted)
        #[arg(long)]
        channel: Option<String>,
    },
    /// Manage daemon configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Generate shell completion script
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
    /// Manage favourite channels (local to this tool, not Mattermost)
    Favourite {
        #[command(subcommand)]
        action: FavouriteAction,
    },
    /// Install the systemd user service for tldr-daemon
    InstallService,
}

#[derive(Subcommand)]
enum FavouriteAction {
    /// List all favourite channels
    List,
    /// Add a channel to favourites  (format: team/channel, case-insensitive)
    Add {
        /// Channel in team/channel format, e.g. "myteam/general"
        #[arg(value_hint = clap::builder::ValueHint::Other)]
        channel: String,
    },
    /// Remove a channel from favourites  (format: team/channel or channel name)
    Remove {
        /// Channel in team/channel format or bare channel name
        #[arg(value_hint = clap::builder::ValueHint::Other)]
        channel: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Validate the configuration file and report missing required fields
    Validate,
    /// Set the list of priority users (replaces the existing list)
    ///
    /// Priority users have their messages highlighted in LLM summaries.
    /// Use Mattermost @usernames (the @ prefix is optional).
    SetPriorityUsers {
        /// Mattermost @usernames to treat as high priority
        #[arg(required = true, value_hint = clap::builder::ValueHint::Other)]
        users: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();

    match cli.command {
        Command::Summarise { channel } => {
            let url = format!("{}/api/v1/summaries", cli.daemon_url);

            let resp = client
                .get(&url)
                .send()
                .await
                .context("failed to reach daemon — is tldr-daemon running?")?;

            if !resp.status().is_success() {
                anyhow::bail!("daemon returned HTTP {}", resp.status());
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .context("failed to parse daemon response")?;
            let mut summaries: Vec<tldr::output::ChannelSummary> =
                serde_json::from_value(body["summaries"].clone()).unwrap_or_default();

            // Optional client-side channel filter
            if let Some(ch) = &channel {
                let ch_lower = ch.to_lowercase();
                summaries
                    .retain(|s| s.channel_name.to_lowercase() == ch_lower || s.channel_id == *ch);
            }

            // Sort by mention_count descending
            summaries.sort_by(|a, b| b.mention_count.cmp(&a.mention_count));
            tldr::output::print_summaries(&summaries);
        }
        Command::Status => {
            let url = format!("{}/api/v1/health", cli.daemon_url);
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    println!("{}", "daemon is running".green().bold());
                }
                Ok(resp) => {
                    println!(
                        "{} (HTTP {})",
                        "daemon returned error".red().bold(),
                        resp.status()
                    );
                }
                Err(e) => {
                    println!("{}: {e}", "daemon is not reachable".red().bold());
                    std::process::exit(1);
                }
            }
        }
        Command::ClearState { channel } => {
            let mut url = format!("{}/api/v1/state", cli.daemon_url);
            if let Some(ch) = &channel {
                url.push_str(&format!("?channel={}", urlencoding::encode(ch)));
            }

            let resp: serde_json::Value = client
                .delete(&url)
                .send()
                .await
                .context("failed to reach daemon — is tldr-daemon running?")?
                .json()
                .await
                .context("failed to parse daemon response")?;

            if resp["ok"].as_bool() != Some(true) {
                let err = resp["error"].as_str().unwrap_or("unknown error");
                anyhow::bail!("daemon error: {err}");
            }

            if channel.is_some() {
                println!("{}", "channel watermark cleared".green());
            } else {
                println!("{}", "all watermarks cleared".green());
            }
        }
        Command::Config { action } => {
            let config_path = tldr::config::expand_tilde(&cli.config);
            match action {
                ConfigAction::Validate => match tldr::config::Config::load(&config_path) {
                    Ok(config) => {
                        let errors = config.validate();
                        if errors.is_empty() {
                            println!("{}", "Config is valid.".green().bold());
                        } else {
                            eprintln!("{}", "Config validation failed:".red().bold());
                            for e in &errors {
                                eprintln!("  • {}", e.red());
                            }
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", "Failed to load config".red().bold());
                        std::process::exit(1);
                    }
                },
                ConfigAction::SetPriorityUsers { users } => {
                    let mut config = match tldr::config::Config::load(&config_path) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("{}: {e}", "Failed to load config".red().bold());
                            std::process::exit(1);
                        }
                    };
                    // Normalise: strip leading @ for canonical storage
                    config.priority_users = users
                        .into_iter()
                        .map(|u| u.trim_start_matches('@').to_string())
                        .collect();
                    config.save(&config_path).context("failed to save config")?;
                    println!("{}", "Priority users updated.".green().bold());
                    println!("Restart the daemon for the change to take effect.");
                }
            }
        }
        Command::Favourite { action } => {
            match action {
                FavouriteAction::List => {
                    let url = format!("{}/api/v1/favourites", cli.daemon_url);
                    let resp: serde_json::Value = client
                        .get(&url)
                        .send()
                        .await
                        .context("failed to reach daemon")?
                        .json()
                        .await
                        .context("failed to parse response")?;
                    if resp["ok"].as_bool() != Some(true) {
                        anyhow::bail!(
                            "daemon error: {}",
                            resp["error"].as_str().unwrap_or("unknown")
                        );
                    }
                    let favs = resp["favourites"]
                        .as_array()
                        .map(|a| a.as_slice())
                        .unwrap_or_default();
                    if favs.is_empty() {
                        println!("{}", "No favourite channels.".dimmed());
                    } else {
                        for f in favs {
                            println!(
                                "{}/{}",
                                f["team_name"].as_str().unwrap_or("?"),
                                f["channel_name"].as_str().unwrap_or("?")
                            );
                        }
                    }
                }
                FavouriteAction::Add { channel } => {
                    // Resolve team/channel → channel_id via /api/v1/channels
                    let url = format!("{}/api/v1/channels", cli.daemon_url);
                    let resp: serde_json::Value = client
                        .get(&url)
                        .send()
                        .await
                        .context("failed to reach daemon")?
                        .json()
                        .await
                        .context("failed to parse response")?;
                    if resp["ok"].as_bool() != Some(true) {
                        anyhow::bail!(
                            "daemon error: {}",
                            resp["error"].as_str().unwrap_or("unknown")
                        );
                    }
                    let channels = resp["channels"]
                        .as_array()
                        .map(|a| a.as_slice())
                        .unwrap_or_default();
                    let input_lower = channel.to_lowercase();
                    let found = channels.iter().find(|c| {
                        let team = c["team_name_normalized"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase();
                        let name = c["channel_name"].as_str().unwrap_or("").to_lowercase();
                        let combined = format!("{team}/{name}");
                        combined == input_lower || name == input_lower
                    });
                    let Some(ch) = found else {
                        anyhow::bail!("channel '{}' not found — use team/channel format", channel);
                    };
                    let channel_id = ch["channel_id"].as_str().unwrap_or("");
                    let channel_name = ch["channel_name"].as_str().unwrap_or("");
                    let team_name = ch["team_name"].as_str().unwrap_or("");
                    let add_url = format!(
                        "{}/api/v1/favourites/{}",
                        cli.daemon_url,
                        urlencoding::encode(channel_id)
                    );
                    let add_resp: serde_json::Value = client.post(&add_url)
                        .json(&serde_json::json!({ "channel_name": channel_name, "team_name": team_name }))
                        .send().await.context("failed to reach daemon")?
                        .json().await.context("failed to parse response")?;
                    if add_resp["ok"].as_bool() != Some(true) {
                        anyhow::bail!(
                            "daemon error: {}",
                            add_resp["error"].as_str().unwrap_or("unknown")
                        );
                    }
                    println!(
                        "{}",
                        format!("Added #{channel_name} to favourites.").green()
                    );
                }
                FavouriteAction::Remove { channel } => {
                    // Resolve using stored favourites list
                    let list_url = format!("{}/api/v1/favourites", cli.daemon_url);
                    let resp: serde_json::Value = client
                        .get(&list_url)
                        .send()
                        .await
                        .context("failed to reach daemon")?
                        .json()
                        .await
                        .context("failed to parse response")?;
                    if resp["ok"].as_bool() != Some(true) {
                        anyhow::bail!(
                            "daemon error: {}",
                            resp["error"].as_str().unwrap_or("unknown")
                        );
                    }
                    let favs = resp["favourites"]
                        .as_array()
                        .map(|a| a.as_slice())
                        .unwrap_or_default();
                    let input_lower = channel.to_lowercase();
                    let found = favs.iter().find(|f| {
                        let team = f["team_name"].as_str().unwrap_or("").to_lowercase();
                        let name = f["channel_name"].as_str().unwrap_or("").to_lowercase();
                        format!("{team}/{name}") == input_lower || name == input_lower
                    });
                    let Some(fav) = found else {
                        anyhow::bail!("'{}' is not in your favourites", channel);
                    };
                    let channel_id = fav["channel_id"].as_str().unwrap_or("");
                    let channel_name = fav["channel_name"].as_str().unwrap_or("");
                    let del_url = format!(
                        "{}/api/v1/favourites/{}",
                        cli.daemon_url,
                        urlencoding::encode(channel_id)
                    );
                    let del_resp: serde_json::Value = client
                        .delete(&del_url)
                        .send()
                        .await
                        .context("failed to reach daemon")?
                        .json()
                        .await
                        .context("failed to parse response")?;
                    if del_resp["ok"].as_bool() != Some(true) {
                        anyhow::bail!(
                            "daemon error: {}",
                            del_resp["error"].as_str().unwrap_or("unknown")
                        );
                    }
                    println!(
                        "{}",
                        format!("Removed #{channel_name} from favourites.").green()
                    );
                }
            }
        }
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "tldr", &mut std::io::stdout());
        }
        Command::InstallService => {
            install_service()?;
        }
    }

    Ok(())
}

fn install_service() -> Result<()> {
    // Locate the service file bundled in the source tree or installed location
    let service_src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contrib/tldr-daemon.service");
    if !service_src.exists() {
        anyhow::bail!(
            "service file not found at {}; ensure you are running from the source tree",
            service_src.display()
        );
    }

    let home = std::env::var("HOME").context("$HOME not set")?;
    let unit_dir = std::path::PathBuf::from(&home).join(".config/systemd/user");
    std::fs::create_dir_all(&unit_dir).context("failed to create systemd user unit directory")?;

    let dest = unit_dir.join("tldr-daemon.service");
    std::fs::copy(&service_src, &dest)
        .with_context(|| format!("failed to copy service file to {}", dest.display()))?;
    println!("{}", format!("installed: {}", dest.display()).green());

    // Reload and enable the unit
    for args in &[
        vec!["--user", "daemon-reload"],
        vec!["--user", "enable", "--now", "tldr-daemon.service"],
    ] {
        let status = std::process::Command::new("systemctl")
            .args(args)
            .status()
            .context("failed to run systemctl")?;
        if !status.success() {
            anyhow::bail!("systemctl {} failed", args.join(" "));
        }
    }
    println!("{}", "service enabled and started".green().bold());
    println!("Run 'journalctl --user -u tldr-daemon -f' to view logs.");
    Ok(())
}
