// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Axum HTTP server: routes, handlers, and shared application state.
//!
//! All API routes live under `/api/v1/`.  Static frontend files are served by a
//! [`ServeDir`] fallback that reads from the `TLDR_FRONTEND_DIR` environment
//! variable (defaults to `frontend/dist`).
//!
//! When the daemon starts in degraded mode (`mm` / `llm` are `None`), the
//! summarise endpoints return 503.  The `/api/v1/health` and `/api/v1/config`
//! routes always respond so the setup wizard can function without a valid config.

mod handlers;
mod state;
mod summarise_handlers;

pub use state::{AppState, HealthState, SeedingProgress};

use axum::{
    Router,
    routing::{delete, get, patch, post, put},
};
use std::sync::Arc;
use tower_http::services::ServeDir;
use tracing::warn;

use crate::mattermost::MattermostClient;
use crate::mattermost_types::{Channel, Team};

use handlers::*;
use summarise_handlers::*;

/// Enumerate all channels the bot user belongs to across all teams.
///
/// Returns the authenticated user ID and a list of (team, channel) pairs.
async fn get_user_channels(
    mm: &MattermostClient,
) -> Result<(String, Vec<(Team, Channel)>), String> {
    let me = mm
        .get_me()
        .await
        .map_err(|e| format!("get_me failed: {e:#}"))?;
    let teams = mm
        .get_teams_for_user(&me.id)
        .await
        .map_err(|e| format!("get_teams failed: {e:#}"))?;
    let mut result = Vec::new();
    for team in &teams {
        match mm.get_channels_for_team_for_user(&me.id, &team.id).await {
            Ok(channels) => {
                for ch in channels {
                    result.push((team.clone(), ch));
                }
            }
            Err(e) => warn!("get_user_channels: failed for {}: {e:#}", team.display_name),
        }
    }
    Ok((me.id, result))
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/api/v1/health", get(handle_health))
        .route("/api/v1/me", get(handle_me))
        .route("/api/v1/me/avatar", get(handle_me_avatar))
        .route("/api/v1/summarise", get(handle_api_summarise))
        .route("/api/v1/summarise/stream", get(handle_api_summarise_ndjson))
        .route("/api/v1/channels/unread", get(handle_channels_unread))
        .route(
            "/api/v1/channels/categories",
            get(handle_channels_categories),
        )
        .route("/api/v1/state", delete(handle_clear_state))
        .route("/api/v1/config/status", get(handle_config_status))
        .route("/api/v1/config", get(handle_config_get))
        .route("/api/v1/config", put(handle_config_put))
        .route("/api/v1/action-items", get(handle_action_items_list))
        .route("/api/v1/action-items/{id}", patch(handle_action_item_patch))
        .route("/api/v1/channels/{id}/read", post(handle_channel_mark_read))
        .route("/api/v1/user-prefs", get(handle_user_prefs_get))
        .route("/api/v1/user-prefs", put(handle_user_prefs_put))
        .route("/api/v1/insights", get(handle_insights))
        .route("/api/v1/favourites", get(handle_favourites_list))
        .route(
            "/api/v1/favourites/{channel_id}",
            post(handle_favourite_add),
        )
        .route(
            "/api/v1/favourites/{channel_id}",
            delete(handle_favourite_remove),
        )
        .route("/api/v1/channels", get(handle_channels_all))
        .route("/api/v1/seeding/status", get(handle_seeding_status))
        .route("/api/v1/summaries", get(handle_summaries_cached))
        .route("/api/v1/summaries/subscribe", get(handle_summaries_sse))
        .route("/llms.txt", get(handle_llms_txt))
        .route("/slash/summarise", post(handle_summarise))
        .with_state(state);

    // Serve frontend static files as fallback
    let frontend_dir =
        std::env::var("TLDR_FRONTEND_DIR").unwrap_or_else(|_| "frontend/dist".to_string());

    api.fallback_service(ServeDir::new(frontend_dir))
}
