// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared application state and request/response types for the HTTP server.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{RwLock, Semaphore, broadcast};

use crate::config::Config;
use crate::llm::LlmClient;
use crate::mattermost::MattermostClient;
use crate::rag::VectorStore;
use crate::store::Store;

/// Runtime health state, updated asynchronously after the background connectivity check.
#[derive(Debug, Clone)]
pub struct HealthState {
    /// True when the config file was loaded and all required fields are set.
    pub configured: bool,
    /// True when the Mattermost ping endpoint returned 200.
    pub mm_ok: bool,
    /// Human-readable error from the last failed check, if any.
    pub error: Option<String>,
    /// Non-None when the RAG vector store failed to initialise.
    pub rag_error: Option<String>,
}

pub struct AppState {
    /// Available only when configured; None means the daemon started in degraded mode.
    pub mm: Option<MattermostClient>,
    /// Available only when configured.
    pub llm: Option<LlmClient>,
    pub config: Config,
    pub config_path: std::path::PathBuf,
    pub store: Option<Store>,
    pub health: Arc<RwLock<HealthState>>,
    /// Optional RAG vector store; None if initialisation failed or was disabled.
    pub rag: Option<Arc<VectorStore>>,
    /// Global 1-permit semaphore serialising all LLM calls (summarise + seeding).
    pub llm_sem: Arc<Semaphore>,
    /// Count of in-flight live summarise requests.  Seeding yields while this is > 0.
    pub summarise_active: Arc<AtomicUsize>,
    /// In-memory seeding progress, updated by the background seeding task.
    pub seeding_progress: Arc<RwLock<SeedingProgress>>,
    /// In-memory cache of latest channel summaries (populated by background loop + on-demand).
    pub summary_cache: Arc<RwLock<Vec<crate::output::ChannelSummary>>>,
    /// Broadcast channel for real-time SSE updates when a summary changes.
    pub summary_tx: broadcast::Sender<crate::output::ChannelSummary>,
}

/// Live progress of the first-run history seeding task.
#[derive(Debug, Clone, Default)]
pub struct SeedingProgress {
    /// True once seeding has run to completion at least once.
    pub seeded: bool,
    /// True while the seeding background task is running.
    pub in_progress: bool,
    pub total_channels: usize,
    pub completed_channels: usize,
}

#[allow(dead_code)] // fields sent by Mattermost but not all consumed
#[derive(Debug, Deserialize)]
pub struct SlashCommandPayload {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct SummariseQuery {
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InsightsQuery {
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ClearStateQuery {
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ActionItemPatch {
    pub action: String, // "ignore" | "resolve"
}

#[derive(Debug, Serialize)]
pub struct ConfigStatus {
    pub configured: bool,
    pub mm_ok: bool,
    pub server_url: String,
    pub model: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ChannelUnreadInfo {
    pub channel_id: String,
    pub channel_name: String,
    pub team_name: String,
    pub channel_type: String,
    pub unread_count: i64,
    pub mention_count: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FavouriteAddBody {
    pub channel_name: String,
    pub team_name: String,
}
