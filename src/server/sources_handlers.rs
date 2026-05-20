// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTP handlers for the sources configuration API.
//!
//! `GET /api/v1/config/sources`  — enumerate live MM teams/channels + email mailboxes,
//!                                  merged with per-source DB overrides
//! `PATCH /api/v1/config/sources` — update enabled/instructions in SQLite (not config.toml)
//! `GET /api/v1/email/mailboxes`  — list all IMAP mailboxes from the server

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::{MailboxConfig, MmChannelConfig, MmTeamConfig};
use crate::email::ImapClient;
use super::state::AppState;
use super::get_user_channels;

// ─── Response types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SourcesResponse {
    pub mattermost: MmSourcesResponse,
    pub email: Option<EmailSourcesResponse>,
}

#[derive(Serialize)]
pub struct MmSourcesResponse {
    pub teams: Vec<TeamInfo>,
}

#[derive(Serialize)]
pub struct TeamInfo {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub enabled: bool,
    pub instructions: Option<String>,
    pub channels: Vec<ChannelInfo>,
}

#[derive(Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub team_name: String,
    pub enabled: bool,
    pub instructions: Option<String>,
}

#[derive(Serialize)]
pub struct EmailSourcesResponse {
    pub mailboxes: Vec<MailboxInfo>,
}

#[derive(Serialize)]
pub struct MailboxInfo {
    pub name: String,
    pub enabled: bool,
    pub instructions: Option<String>,
}

// ─── Request types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SourcesPatch {
    #[serde(default)]
    pub mattermost: Option<MmSourcesPatch>,
    #[serde(default)]
    pub email: Option<EmailSourcesPatch>,
}

#[derive(Deserialize)]
pub struct MmSourcesPatch {
    #[serde(default)]
    pub teams: Vec<TeamPatch>,
    #[serde(default)]
    pub channels: Vec<ChannelPatch>,
}

#[derive(Deserialize)]
pub struct TeamPatch {
    pub name: String,
    pub enabled: bool,
    pub instructions: Option<String>,
}

#[derive(Deserialize)]
pub struct ChannelPatch {
    pub name: String,
    pub team: Option<String>,
    pub enabled: bool,
    pub instructions: Option<String>,
}

#[derive(Deserialize)]
pub struct EmailSourcesPatch {
    pub mailboxes: Vec<MailboxPatch>,
}

#[derive(Deserialize)]
pub struct MailboxPatch {
    pub name: String,
    pub enabled: bool,
    pub instructions: Option<String>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

pub async fn handle_sources_get(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(mm) = &state.mm else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "daemon not configured"})),
        )
            .into_response();
    };

    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "store not available"})),
        )
            .into_response();
    };

    // Load DB source configs
    let db_teams = store.get_source_configs("mm_team").unwrap_or_default();
    let db_channels = store.get_source_configs("mm_channel").unwrap_or_default();

    // Enumerate live teams/channels from Mattermost
    let (live_teams, live_channels) = match get_user_channels(mm).await {
        Ok((_user_id, pairs)) => {
            let mut teams_seen: Vec<crate::mattermost_types::Team> = Vec::new();
            let mut seen_ids = std::collections::HashSet::new();
            let mut channels: Vec<(crate::mattermost_types::Team, crate::mattermost_types::Channel)> = Vec::new();
            for (team, ch) in pairs {
                if seen_ids.insert(team.id.clone()) {
                    teams_seen.push(team.clone());
                }
                channels.push((team, ch));
            }
            (teams_seen, channels)
        }
        Err(e) => {
            warn!("sources GET: failed to enumerate MM channels: {e}");
            (Vec::new(), Vec::new())
        }
    };

    // Helper: look up DB config then fall back to config.toml, then defaults
    let teams: Vec<TeamInfo> = live_teams
        .iter()
        .map(|team| {
            // DB row takes precedence; config.toml overrides DB
            let db_row = db_teams.iter().find(|r| r.name == team.name);
            let cfg_row = state.config.mattermost.teams.iter()
                .find(|c| c.name == team.name || c.name == team.display_name);
            let team_enabled = cfg_row.map(|c| c.enabled)
                .or_else(|| db_row.map(|r| r.enabled))
                .unwrap_or(true);
            let team_instructions = cfg_row.and_then(|c| c.instructions.clone())
                .or_else(|| db_row.and_then(|r| r.instructions.clone()));

            // Only real channels: type "O" (open) or "P" (private)
            let channels: Vec<ChannelInfo> = live_channels
                .iter()
                .filter(|(t, ch)| {
                    t.id == team.id
                        && (ch.channel_type == "O" || ch.channel_type == "P")
                })
                .map(|(t, ch)| {
                    let db_ch = db_channels.iter().find(|r| {
                        r.name == ch.name
                            && (r.team.is_empty() || r.team == t.name)
                    });
                    let cfg_ch = state.config.mattermost.channels.iter().find(|c| {
                        (c.name == ch.name || c.name == ch.display_name)
                            && c.team.as_ref()
                                .map(|tn| tn == &t.name || tn == &t.display_name)
                                .unwrap_or(true)
                    });
                    let ch_enabled = cfg_ch.map(|c| c.enabled)
                        .or_else(|| db_ch.map(|r| r.enabled))
                        .unwrap_or(true);
                    let ch_instructions = cfg_ch.and_then(|c| c.instructions.clone())
                        .or_else(|| db_ch.and_then(|r| r.instructions.clone()));
                    ChannelInfo {
                        id: ch.id.clone(),
                        name: ch.name.clone(),
                        display_name: ch.display_name.clone(),
                        team_name: t.name.clone(),
                        enabled: ch_enabled,
                        instructions: ch_instructions,
                    }
                })
                .collect();

            TeamInfo {
                id: team.id.clone(),
                name: team.name.clone(),
                display_name: team.display_name.clone(),
                enabled: team_enabled,
                instructions: team_instructions,
                channels,
            }
        })
        .collect();

    // Email: mailboxes are fetched live from IMAP by a separate endpoint;
    // here we return the current override settings from DB + config.toml.
    let email = if state.config.email.is_some() {
        let db_mailboxes = store.get_source_configs("email_mailbox").unwrap_or_default();
        let mailbox_configs = state.email_mailbox_config.read().await;

        // Return the in-memory list (which was built from DB + config seeds at startup)
        let mailboxes: Vec<MailboxInfo> = mailbox_configs
            .iter()
            .map(|mb| {
                // config.toml override wins
                let cfg = state.config.email.as_ref()
                    .and_then(|e| e.mailboxes.iter().find(|m| m.name == mb.name));
                let db = db_mailboxes.iter().find(|r| r.name == mb.name);
                MailboxInfo {
                    name: mb.name.clone(),
                    enabled: cfg.map(|c| c.enabled)
                        .or_else(|| db.map(|r| r.enabled))
                        .unwrap_or(mb.enabled),
                    instructions: cfg.and_then(|c| c.instructions.clone())
                        .or_else(|| db.and_then(|r| r.instructions.clone()))
                        .or_else(|| mb.instructions.clone()),
                }
            })
            .collect();
        Some(EmailSourcesResponse { mailboxes })
    } else {
        None
    };

    let resp = SourcesResponse {
        mattermost: MmSourcesResponse { teams },
        email,
    };

    (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
}

pub async fn handle_sources_patch(
    State(state): State<Arc<AppState>>,
    Json(patch): Json<SourcesPatch>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "store not available"})),
        )
            .into_response();
    };

    // Apply MM team overrides → DB + in-memory
    if let Some(mm_patch) = &patch.mattermost {
        for tp in &mm_patch.teams {
            if let Err(e) = store.upsert_source_config(
                "mm_team", &tp.name, "", tp.enabled, tp.instructions.as_deref(),
            ) {
                warn!("sources PATCH: failed to save team config: {e:#}");
            }
        }
        if !mm_patch.teams.is_empty() {
            let mut teams = state.mm_team_config.write().await;
            for tp in &mm_patch.teams {
                if let Some(existing) = teams.iter_mut().find(|t| t.name == tp.name) {
                    existing.enabled = tp.enabled;
                    existing.instructions = tp.instructions.clone();
                } else {
                    teams.push(MmTeamConfig {
                        name: tp.name.clone(),
                        enabled: tp.enabled,
                        instructions: tp.instructions.clone(),
                    });
                }
            }
        }

        for cp in &mm_patch.channels {
            let team_key = cp.team.as_deref().unwrap_or("");
            if let Err(e) = store.upsert_source_config(
                "mm_channel", &cp.name, team_key, cp.enabled, cp.instructions.as_deref(),
            ) {
                warn!("sources PATCH: failed to save channel config: {e:#}");
            }
        }
        if !mm_patch.channels.is_empty() {
            let mut channels = state.mm_channel_config.write().await;
            for cp in &mm_patch.channels {
                let found = channels.iter_mut().find(|c| {
                    c.name == cp.name
                        && c.team.as_deref().unwrap_or("") == cp.team.as_deref().unwrap_or("")
                });
                if let Some(existing) = found {
                    existing.enabled = cp.enabled;
                    existing.instructions = cp.instructions.clone();
                } else {
                    channels.push(MmChannelConfig {
                        name: cp.name.clone(),
                        team: cp.team.clone(),
                        enabled: cp.enabled,
                        instructions: cp.instructions.clone(),
                    });
                }
            }
        }
    }

    // Apply email mailbox overrides → DB + in-memory
    if let Some(email_patch) = &patch.email {
        for mp in &email_patch.mailboxes {
            if let Err(e) = store.upsert_source_config(
                "email_mailbox", &mp.name, "", mp.enabled, mp.instructions.as_deref(),
            ) {
                warn!("sources PATCH: failed to save mailbox config: {e:#}");
            }
        }
        let mut mailboxes = state.email_mailbox_config.write().await;
        for mp in &email_patch.mailboxes {
            if let Some(existing) = mailboxes.iter_mut().find(|m| m.name == mp.name) {
                existing.enabled = mp.enabled;
                existing.instructions = mp.instructions.clone();
            } else {
                mailboxes.push(MailboxConfig {
                    name: mp.name.clone(),
                    enabled: mp.enabled,
                    instructions: mp.instructions.clone(),
                });
            }
        }
    }

    info!("sources PATCH: config updated in DB");
    (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
}

/// List all IMAP mailboxes from the configured email server.
/// Returns a flat list of mailbox names.
pub async fn handle_email_mailboxes_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(email_cfg) = &state.config.email else {
        return (StatusCode::OK, Json(serde_json::json!({"mailboxes": []}))).into_response();
    };

    match ImapClient::connect(email_cfg).await {
        Ok(mut client) => match client.list_mailboxes().await {
            Ok(names) => {
                let _ = client.logout().await;
                (StatusCode::OK, Json(serde_json::json!({"mailboxes": names}))).into_response()
            }
            Err(e) => {
                warn!("email mailboxes list: IMAP LIST failed: {e:#}");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": format!("{e:#}")})),
                )
                    .into_response()
            }
        },
        Err(e) => {
            warn!("email mailboxes list: IMAP connect failed: {e:#}");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("{e:#}")})),
            )
                .into_response()
        }
    }
}
