// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTP handlers for summarisation: on-demand, streaming NDJSON, clear-state,
//! and the Mattermost slash command.

use axum::{
    Form, Json,
    body::Body,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::warn;

use super::state::*;
use crate::summarise::{summarise_all_unread, summarise_all_unread_stream};

pub async fn handle_api_summarise(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SummariseQuery>,
) -> impl IntoResponse {
    let (Some(mm), Some(llm)) = (&state.mm, &state.llm) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "daemon is not configured" })),
        );
    };
    let channel_filter = query.channel.as_deref();
    let server_url = &state.config.mattermost.server_url;

    state.summarise_active.fetch_add(1, Ordering::AcqRel);
    let result = summarise_all_unread(
        mm,
        llm,
        channel_filter,
        server_url,
        state.store.as_ref(),
        &state.config.priority_users,
        state.rag.as_ref().map(Arc::clone),
        Arc::clone(&state.llm_sem),
    )
    .await;
    state.summarise_active.fetch_sub(1, Ordering::AcqRel);

    match result {
        Ok(summaries) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "summaries": summaries })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
        ),
    }
}

/// Streaming NDJSON variant of `/api/v1/summarise`.
///
/// Sends one `ChannelSummary` JSON object per line as each channel is processed,
/// followed by a `{"done":true}` sentinel line when all channels are complete.
/// The client should re-sort by `mention_count` on receipt of the sentinel.
pub async fn handle_api_summarise_ndjson(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SummariseQuery>,
) -> axum::response::Response {
    use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

    let (Some(mm), Some(llm)) = (&state.mm, &state.llm) else {
        return axum::response::Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Content-Type", "application/x-ndjson")
            .body(Body::from("{\"error\":\"daemon is not configured\"}\n"))
            .unwrap();
    };

    let (sum_tx, sum_rx) = tokio::sync::mpsc::channel::<crate::output::ChannelSummary>(64);
    let (line_tx, line_rx) = tokio::sync::mpsc::channel::<String>(64);

    let mm = mm.clone();
    let llm = llm.clone();
    let channel_filter = query.channel.clone();
    let server_url = state.config.mattermost.server_url.clone();
    let store = state.store.clone();
    let priority_users = state.config.priority_users.clone();
    let rag = state.rag.as_ref().map(Arc::clone);
    let llm_sem = Arc::clone(&state.llm_sem);
    let summarise_active = Arc::clone(&state.summarise_active);

    // Inner task: drives the summarise work and sends ChannelSummary items.
    // Increments summarise_active so seeding yields while this is running.
    summarise_active.fetch_add(1, Ordering::AcqRel);
    tokio::spawn(async move {
        summarise_all_unread_stream(
            sum_tx,
            &mm,
            &llm,
            channel_filter.as_deref(),
            &server_url,
            store.as_ref(),
            &priority_users,
            rag,
            llm_sem,
        )
        .await
        .ok();
        summarise_active.fetch_sub(1, Ordering::AcqRel);
        // sum_tx drops here → sum_rx.recv() returns None
    });

    // Outer task: format each summary as an NDJSON line, then emit sentinel.
    // Also updates the summary cache and broadcasts to SSE subscribers.
    let cache_ref = Arc::clone(&state.summary_cache);
    let broadcast_tx = state.summary_tx.clone();
    let store_for_cache = state.store.clone();
    tokio::spawn(async move {
        let mut rx = sum_rx;
        let mut collected: Vec<crate::output::ChannelSummary> = Vec::new();
        while let Some(s) = rx.recv().await {
            // Persist to SQLite cache
            if let Some(ref st) = store_for_cache
                && let Ok(json) = serde_json::to_string(&s)
            {
                let _ = st.set_cached_summary(&s.channel_id, &json);
            }
            // Broadcast to SSE subscribers
            let _ = broadcast_tx.send(s.clone());
            collected.push(s.clone());
            let line = serde_json::to_string(&s).unwrap_or_default() + "\n";
            if line_tx.send(line).await.is_err() {
                break;
            }
        }
        // Replace in-memory cache with this cycle's results
        if !collected.is_empty() {
            let mut cache = cache_ref.write().await;
            *cache = collected;
        }
        line_tx.send("{\"done\":true}\n".to_string()).await.ok();
    });

    let stream = ReceiverStream::new(line_rx).map(Ok::<_, std::convert::Infallible>);

    axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/x-ndjson")
        .header("Cache-Control", "no-cache")
        .body(Body::from_stream(stream))
        .expect("response builder failed")
}

pub async fn handle_clear_state(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ClearStateQuery>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": "state db not configured" })),
        );
    };

    let result = if let Some(channel_id) = &query.channel {
        store.clear_channel(channel_id)
    } else {
        store.clear_all()
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        Err(e) => {
            warn!("clear_state error: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": format!("{e:#}") })),
            )
        }
    }
}

pub async fn handle_summarise(
    State(state): State<Arc<AppState>>,
    Form(payload): Form<SlashCommandPayload>,
) -> impl IntoResponse {
    let (Some(mm), Some(llm)) = (&state.mm, &state.llm) else {
        return Json(serde_json::json!({
            "response_type": "ephemeral",
            "text": "Daemon is not configured. Visit the web UI to set up."
        }));
    };

    // Validate slash command token if configured
    if let Some(expected_token) = &state.config.server.slash_token
        && payload.token != *expected_token
    {
        return Json(serde_json::json!({
            "response_type": "ephemeral",
            "text": "Invalid slash command token."
        }));
    }

    let channel_filter = if payload.text.is_empty() {
        None
    } else {
        Some(payload.text.as_str())
    };

    let server_url = state.config.mattermost.server_url.clone();

    state.summarise_active.fetch_add(1, Ordering::AcqRel);
    let slash_result = summarise_all_unread(
        mm,
        llm,
        channel_filter,
        &server_url,
        state.store.as_ref(),
        &state.config.priority_users,
        state.rag.as_ref().map(Arc::clone),
        Arc::clone(&state.llm_sem),
    )
    .await;
    state.summarise_active.fetch_sub(1, Ordering::AcqRel);

    match slash_result {
        Ok(summaries) => {
            if summaries.is_empty() {
                return Json(serde_json::json!({
                    "response_type": "ephemeral",
                    "text": "No unread channels to summarise."
                }));
            }

            let mut text = String::new();
            let mut current_team = String::new();
            for s in &summaries {
                if s.team_name != current_team {
                    if !current_team.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("### {}\n", s.team_name));
                    current_team = s.team_name.clone();
                }
                text.push_str(&format!(
                    "\n**#{}** ({} unread, {} mentions)\n{}\n",
                    s.channel_name, s.unread_count, s.mention_count, s.summary
                ));
            }

            Json(serde_json::json!({
                "response_type": "ephemeral",
                "text": text
            }))
        }
        Err(e) => Json(serde_json::json!({
            "response_type": "ephemeral",
            "text": format!("Error: {e:#}")
        })),
    }
}
