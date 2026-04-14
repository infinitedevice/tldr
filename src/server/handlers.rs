// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTP handlers for general endpoints: health, user info, preferences, config,
//! insights, favourites, action items, channels, seeding, and SSE subscriptions.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::warn;

use super::get_user_channels;
use super::state::*;

// ── Health ──────────────────────────────────────────────────────────────

pub async fn handle_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let h = state.health.read().await;
    let store_ok = state.store.is_some();
    let llm_ok = state.llm.is_some();
    let rag_ok = state.rag.is_some();
    let rag_configured = rag_ok || h.rag_error.is_some();
    let status = if h.configured && h.mm_ok {
        "ok"
    } else {
        "degraded"
    };
    Json(serde_json::json!({
        "status": status,
        "configured": h.configured,
        "mm_status": if h.mm_ok { "ok" } else { "error" },
        "error": h.error,
        "store_ok": store_ok,
        "llm_ok": llm_ok,
        "rag_ok": rag_ok,
        "rag_configured": rag_configured,
        "rag_error": h.rag_error,
        "poll_interval_secs": state.config.server.poll_interval_secs,
    }))
}

// ── User info ───────────────────────────────────────────────────────────

pub async fn handle_me(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(mm) = &state.mm else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "daemon is not configured" })),
        )
            .into_response();
    };
    match mm.get_me().await {
        Ok(user) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "id": user.id,
                "username": user.username,
                "display_name": user.display_name(),
                "avatar_url": "/api/v1/me/avatar",
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

pub async fn handle_me_avatar(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let Some(mm) = &state.mm else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let user_id = match mm.get_me().await {
        Ok(u) => u.id,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match mm.get_user_avatar_bytes(&user_id).await {
        Ok((bytes, content_type)) => axum::response::Response::builder()
            .status(200)
            .header("Content-Type", content_type)
            .header("Cache-Control", "public, max-age=3600")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── User preferences ────────────────────────────────────────────────────

pub async fn handle_user_prefs_get(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "state db not configured" })),
        )
            .into_response();
    };
    match store.get_all_prefs() {
        Ok(prefs) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "prefs": prefs })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

pub async fn handle_user_prefs_put(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Map<String, serde_json::Value>>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "state db not configured" })),
        )
            .into_response();
    };
    for (key, value) in &body {
        let val_str = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if let Err(e) = store.set_pref(key, &val_str) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
            )
                .into_response();
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ── Config ──────────────────────────────────────────────────────────────

pub async fn handle_config_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let h = state.health.read().await;
    Json(ConfigStatus {
        configured: h.configured,
        mm_ok: h.mm_ok,
        server_url: state.config.mattermost.server_url.clone(),
        model: state.config.llm.model.clone(),
        error: h.error.clone(),
    })
}

pub async fn handle_config_get(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.config.clone())
}

pub async fn handle_config_put(
    State(state): State<Arc<AppState>>,
    Json(new_config): Json<crate::config::Config>,
) -> impl IntoResponse {
    let toml = match toml::to_string_pretty(&new_config) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("{e}") })),
            );
        }
    };
    if let Some(parent) = state.config_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e}") })),
        );
    }
    match std::fs::write(&state.config_path, toml) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e}") })),
        ),
    }
}

// ── Channels ────────────────────────────────────────────────────────────

pub async fn handle_channels_unread(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(mm) = &state.mm else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "daemon is not configured" })),
        )
            .into_response();
    };
    let (user_id, work_items) = match get_user_channels(mm).await {
        Ok(items) => items,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": e })),
            )
                .into_response();
        }
    };
    let mut join_set: JoinSet<Option<ChannelUnreadInfo>> = JoinSet::new();
    for (team, channel) in work_items {
        let mm = mm.clone();
        let user_id = user_id.clone();
        join_set.spawn(async move {
            let unread = mm.get_channel_unread(&user_id, &channel.id).await.ok()?;
            if unread.msg_count == 0 {
                return None;
            }
            let channel_id = channel.id.clone();
            let channel_name = channel.label().to_string();
            Some(ChannelUnreadInfo {
                channel_id,
                channel_name,
                team_name: team.display_name,
                channel_type: channel.channel_type,
                unread_count: unread.msg_count,
                mention_count: unread.mention_count,
            })
        });
    }
    let mut channels: Vec<ChannelUnreadInfo> = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(Some(info)) = result {
            channels.push(info);
        }
    }
    channels.sort_by(|a, b| b.mention_count.cmp(&a.mention_count));
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "channels": channels })),
    )
        .into_response()
}

pub async fn handle_channels_categories(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(mm) = &state.mm else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "daemon is not configured" })),
        )
            .into_response();
    };
    let me = match mm.get_me().await {
        Ok(u) => u,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
            )
                .into_response();
        }
    };
    let teams = match mm.get_teams_for_user(&me.id).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
            )
                .into_response();
        }
    };
    let mut all_categories: Vec<serde_json::Value> = Vec::new();
    for team in &teams {
        match mm.get_channel_categories(&me.id, &team.id).await {
            Ok(cats) => {
                for cat in cats {
                    all_categories.push(serde_json::json!({
                        "id": cat.id,
                        "team_id": team.id,
                        "team_name": team.display_name,
                        "type": cat.category_type,
                        "display_name": cat.display_name,
                        "channel_ids": cat.channel_ids,
                    }));
                }
            }
            Err(e) => warn!(
                "channels/categories: failed to get categories for {}: {e:#}",
                team.display_name
            ),
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "categories": all_categories })),
    )
        .into_response()
}

pub async fn handle_channels_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(mm) = &state.mm else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "daemon is not configured" })),
        )
            .into_response();
    };
    let (_user_id, items) = match get_user_channels(mm).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": e })),
            )
                .into_response();
        }
    };
    let channels: Vec<serde_json::Value> = items
        .into_iter()
        .map(|(team, ch)| {
            serde_json::json!({
                "channel_id": ch.id,
                "channel_name": ch.name,
                "display_name": ch.display_name,
                "team_id": team.id,
                "team_name": team.display_name,
                "team_name_normalized": team.name,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "channels": channels })),
    )
        .into_response()
}

pub async fn handle_channel_mark_read(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let now = jiff::Timestamp::now().as_millisecond();
    match store.set_watermark(&channel_id, now) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!("mark-read error for {channel_id}: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Insights ────────────────────────────────────────────────────────────

pub async fn handle_insights(
    State(state): State<Arc<AppState>>,
    Query(params): Query<InsightsQuery>,
) -> impl IntoResponse {
    let (Some(store), Some(llm)) = (&state.store, &state.llm) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "daemon is not configured" })),
        )
            .into_response();
    };

    let now_ms = jiff::Timestamp::now().as_millisecond();
    let to_ms = params.to_ms.unwrap_or(now_ms);
    let from_ms = params.from_ms.unwrap_or(to_ms - 7 * 24 * 60 * 60 * 1000);

    let mut insights = match store.get_channel_insights_in_range(from_ms, to_ms) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
            )
                .into_response();
        }
    };

    // Resolve stale DM channel names that are still raw userid1__userid2 slugs.
    if let Some(mm) = &state.mm {
        let me_id = mm.get_me().await.map(|u| u.id).unwrap_or_default();
        for insight in &mut insights {
            if insight.channel_name.contains("__") {
                let parts: Vec<&str> = insight.channel_name.split("__").collect();
                if parts.len() == 2 && parts.iter().all(|p| p.len() >= 20) {
                    let mut names = Vec::new();
                    for uid in &parts {
                        if *uid == me_id {
                            continue;
                        }
                        match mm.get_user(uid).await {
                            Ok(user) => names.push(user.display_name().to_string()),
                            Err(_) => names.push((*uid).to_string()),
                        }
                    }
                    if !names.is_empty() {
                        insight.channel_name = names.join(", ");
                    }
                }
            }
        }
    }

    if insights.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "insights": insights,
                "synthesis": serde_json::Value::Null,
            })),
        )
            .into_response();
    }

    let user_role = &state.config.user_role;

    match llm.synthesize_insights(&insights, user_role).await {
        Ok(synthesis) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "insights": insights,
                "synthesis": synthesis,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

// ── Favourites ──────────────────────────────────────────────────────────

pub async fn handle_favourites_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "store not configured" })),
        )
            .into_response();
    };
    match store.get_favourites() {
        Ok(favs) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "favourites": favs })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

pub async fn handle_favourite_add(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<String>,
    Json(body): Json<FavouriteAddBody>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "store not configured" })),
        )
            .into_response();
    };
    match store.add_favourite(&channel_id, &body.channel_name, &body.team_name) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

pub async fn handle_favourite_remove(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "store not configured" })),
        )
            .into_response();
    };
    match store.remove_favourite(&channel_id) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

// ── Action items ────────────────────────────────────────────────────────

pub async fn handle_action_items_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "state db not configured" })),
        );
    };
    match store.get_all_action_items_global() {
        Ok(items) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "items": items })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        ),
    }
}

pub async fn handle_action_item_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ActionItemPatch>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "state db not configured" })),
        );
    };
    let result = match body.action.as_str() {
        "ignore" => store.set_action_item_ignored(&id, true),
        "resolve" => store.set_action_item_resolved(&id, true),
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "ok": false, "error": format!("unknown action: {other}") }),
                ),
            );
        }
    };
    match result {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        ),
    }
}

// ── Seeding ─────────────────────────────────────────────────────────────

pub async fn handle_seeding_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let p = state.seeding_progress.read().await;
    let seed_from_ms = if p.seeded {
        let now_ms = jiff::Timestamp::now().as_millisecond();
        let four_weeks_ms = 4 * 7 * 24 * 60 * 60 * 1000_i64;
        Some(now_ms - four_weeks_ms)
    } else {
        None
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "seeding": {
                "seeded": p.seeded,
                "in_progress": p.in_progress,
                "total_channels": p.total_channels,
                "completed_channels": p.completed_channels,
            },
            "seed_from_ms": seed_from_ms,
        })),
    )
        .into_response()
}

// ── Summaries (cached + SSE) ────────────────────────────────────────────

/// Return the in-memory summary cache instantly (no LLM calls).
pub async fn handle_summaries_cached(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cache = state.summary_cache.read().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "summaries": *cache })),
    )
}

/// SSE endpoint: streams a `ChannelSummary` JSON event each time a channel is
/// re-summarised in the background.
pub async fn handle_summaries_sse(State(state): State<Arc<AppState>>) -> axum::response::Response {
    use tokio_stream::{StreamExt as _, wrappers::BroadcastStream};

    let rx = state.summary_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(summary) => {
            let json = serde_json::to_string(&summary).unwrap_or_default();
            Some(Ok::<_, std::convert::Infallible>(format!(
                "data: {json}\n\n"
            )))
        }
        Err(_) => None,
    });

    axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(stream))
        .expect("response builder failed")
}

// ── llms.txt ────────────────────────────────────────────────────────────

pub async fn handle_llms_txt() -> impl IntoResponse {
    const CONTENT: &str = "\
# tldr — Mattermost chat summarisation daemon

> A self-hosted daemon that summarises unread Mattermost channels using an
> OpenAI-compatible LLM. Background loop generates summaries automatically;
> results are served via a JSON API and SSE to a Svelte frontend and a CLI.

## API

GET /api/v1/health — health check; returns {status, configured, mm_status, error}
GET /api/v1/me — current authenticated Mattermost user {id, username, display_name, avatar_url}
GET /api/v1/me/avatar — current user's Mattermost avatar image (proxied)
GET /api/v1/summaries — cached summaries from the background loop; returns {ok, summaries[]}
GET /api/v1/summaries/subscribe — SSE stream; each event is a ChannelSummary JSON object
GET /api/v1/summarise — on-demand summarise all unread channels; returns {ok, summaries[]}
GET /api/v1/summarise/stream — streaming NDJSON; one ChannelSummary JSON per line then {\"done\":true}
GET /api/v1/channels/unread — list channels with unread messages (fast, no LLM); returns {ok, channels[]}
GET /api/v1/channels/categories — Mattermost sidebar categories per team (Favorites, custom groups); returns {ok, categories[]}
POST /api/v1/channels/{id}/read — mark channel as read (advances watermark to now)
DELETE /api/v1/state — clear stored watermarks (forces full re-summarise); optional ?channel=id
GET /api/v1/user-prefs — user preferences; returns {ok, prefs:{favourites, user_role}}
PUT /api/v1/user-prefs — update user preferences; body: JSON object of key/value pairs
GET /api/v1/config — full daemon configuration (TOML fields as JSON)
PUT /api/v1/config — write updated daemon configuration
GET /api/v1/config/status — config validation; returns {configured, mm_ok, server_url, model, error}
GET /api/v1/action-items — all tracked action items across channels; returns {ok, items[]}
PATCH /api/v1/action-items/{id} — update action item status; body: {action: \"ignore\"|\"resolve\"}
POST /slash/summarise — Mattermost slash command handler (form-encoded)

## Data shapes

ChannelSummary: {team_name, channel_name, channel_id, channel_url, unread_count, mention_count, summary, summary_html, topics[], action_items[], topic?, participants?}
ActionItem: {id, channel_id, text, created_at, resolved, ignored}
ChannelUnreadInfo: {channel_id, channel_name, team_name, channel_type, unread_count, mention_count}
ChannelCategory: {id, team_id, team_name, type, display_name, channel_ids[]}
UserPrefs: {favourites: string[] (channel_ids), user_role: string}

## Notes

- Background loop runs every poll_interval_secs (default 120s, set to 0 to disable)
- Summaries are cached in SQLite (cached_summary table) and in-memory (AppState.summary_cache)
- SSE subscribers receive updates via tokio::sync::broadcast as each channel completes
- All LLM calls are serialised through a single-permit semaphore to avoid overwhelming the model server
- Posts are fetched newest-first (page-based pagination) to avoid silent truncation of unread messages
- Watermarks are stored in SQLite so subsequent runs are incremental
- User prefs (favourites, user_role) survive across browser sessions and devices via the daemon's SQLite
";
    (
        StatusCode::OK,
        [("Content-Type", "text/plain; charset=utf-8")],
        CONTENT,
    )
}
