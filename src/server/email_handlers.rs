// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTP handlers for the email data source endpoints.

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::warn;

use super::state::AppState;
use crate::output::{ActionItemSummary, EmailSummary};

/// GET /api/v1/email/summaries
///
/// Returns cached email summaries with action items re-hydrated from the live
/// store so that resolved/ignored/claimed state is always current.
pub async fn handle_email_summaries_cached(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let cache = state.email_cache.read().await;
    let mut summaries: Vec<EmailSummary> = cache.clone();
    drop(cache);

    // Re-hydrate action items from live store for each mailbox
    if let Some(store) = &state.store {
        for s in &mut summaries {
            let live_items: Vec<ActionItemSummary> = store
                .get_pending_action_items(&s.mailbox)
                .unwrap_or_default()
                .into_iter()
                .map(|a| ActionItemSummary {
                    id: a.id,
                    text: a.text,
                    claimed: a.claimed,
                })
                .collect();
            s.action_items = live_items;
        }
    }

    Json(serde_json::json!({ "summaries": summaries }))
}

/// GET /api/v1/email/summaries/subscribe
///
/// Server-Sent Events stream — each event is a JSON-serialised [`EmailSummary`].
pub async fn handle_email_summaries_sse(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let rx = state.email_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(summary) => serde_json::to_string(&summary).ok().map(|json| {
            Ok::<_, std::convert::Infallible>(format!("data: {json}\n\n"))
        }),
        Err(_) => None,
    });

    let body = Body::from_stream(stream);
    axum::response::Response::builder()
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

/// POST /api/v1/email/{mailbox}/read
///
/// Advances the email watermark for this mailbox to the max UID from the
/// cached summary (so already-processed emails are not re-fetched), then
/// evicts the summary from cache and SQLite so it won't reappear.
pub async fn handle_email_mailbox_mark_read(
    State(state): State<Arc<AppState>>,
    Path(mailbox): Path<String>,
) -> impl IntoResponse {
    let Some(store) = &state.store else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // Advance the watermark to the highest UID we showed the user, so those
    // messages aren't re-fetched next cycle.  Must be read before eviction.
    let max_uid = {
        let cache = state.email_cache.read().await;
        cache.iter().find(|s| s.mailbox == mailbox).map(|s| s.max_uid).unwrap_or(0)
    };
    if max_uid > 0 {
        if let Err(e) = store.set_email_watermark(&mailbox, max_uid) {
            warn!("email mark-read: failed to advance watermark for {mailbox}: {e:#}");
        }
    }

    // Evict from SQLite cache
    if let Err(e) = store.remove_cached_email_summary(&mailbox) {
        warn!("email mark-read: failed to remove cached summary for {mailbox}: {e:#}");
    }

    // Evict from in-memory cache
    {
        let mut cache = state.email_cache.write().await;
        cache.retain(|s| s.mailbox != mailbox);
    }

    StatusCode::NO_CONTENT.into_response()
}
