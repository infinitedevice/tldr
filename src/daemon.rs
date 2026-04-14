// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Daemon startup: binds the listener, wires `AppState`, and runs `axum::serve`.
//!
//! The daemon always starts even when `Config::validate` reports errors; in that
//! case `mm` and `llm` are `None` and summarise endpoints return 503 until the
//! user completes the web UI setup wizard.  A background Tokio task performs a
//! live Mattermost connectivity check and updates `AppState::health` asynchronously.

use anyhow::{Context, Result};
use listenfd::ListenFd;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::Config;
use crate::llm::LlmClient;
use crate::mattermost::MattermostClient;
use crate::rag::VectorStore;
use crate::seeding::{needs_seeding, seed_history};
use crate::server::{AppState, HealthState, SeedingProgress, create_router};
use crate::store::Store;
use crate::summarise::background_summarise_loop;

/// Start the HTTP daemon.
///
/// The daemon always binds and serves even when `config` fails validation; in that
/// case `mm` and `llm` are `None` and the summarise endpoints return 503 until the
/// user completes setup via the web UI. A background task performs a live Mattermost
/// connectivity check and updates `AppState.health` asynchronously.
pub async fn run_daemon(config: Config, config_path: std::path::PathBuf) -> Result<()> {
    let config_errors = config.validate();
    let configured = config_errors.is_empty();

    if !configured {
        warn!(
            "daemon starting in degraded mode: {}",
            config_errors.join("; ")
        );
    }

    let mm: Option<MattermostClient> = if configured {
        match MattermostClient::new(&config.mattermost.server_url, &config.mattermost.token) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!("failed to create Mattermost client: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let llm: Option<LlmClient> = if configured {
        match LlmClient::new(
            &config.llm.base_url,
            &config.llm.model,
            config.llm.bearer_token.as_deref(),
        ) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!("failed to create LLM client: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let store = if let Some(db_path) = config.state_db_path() {
        info!(path = %db_path.display(), "opening state db");
        match Store::open(&db_path) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!("failed to open state db: {e:#}");
                None
            }
        }
    } else {
        info!("state db disabled (paths.state_db is empty)");
        None
    };

    let mut rag_init_error: Option<String> = None;
    let rag = if let Some(vectors_path) = config.vectors_dir_path() {
        info!(path = %vectors_path.display(), "opening vector store");
        match VectorStore::open(&vectors_path).await {
            Ok(vs) => Some(Arc::new(vs)),
            Err(e) => {
                let msg = format!("{e:#}");
                warn!("failed to open vector store: {msg}; RAG disabled");
                rag_init_error = Some(msg);
                None
            }
        }
    } else {
        info!("vector store disabled (paths.vectors_dir is empty)");
        None
    };

    let rag_error: Option<String> = rag_init_error;

    let health = Arc::new(RwLock::new(HealthState {
        configured,
        mm_ok: false,
        error: if configured {
            None
        } else {
            Some(config_errors.join("; "))
        },
        rag_error,
    }));

    let listen_addr = config.server.listen_addr.clone();

    let llm_sem = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let summarise_active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let initial_seeded = store
        .as_ref()
        .map(|s| s.get_pref("seeding_done").is_some())
        .unwrap_or(false);
    let seeding_progress = std::sync::Arc::new(RwLock::new(SeedingProgress {
        seeded: initial_seeded,
        in_progress: false,
        total_channels: 0,
        completed_channels: 0,
    }));

    // Load cached summaries from SQLite so the first GET /api/v1/summaries is instant
    let initial_cache: Vec<crate::output::ChannelSummary> = if let Some(s) = &store {
        s.get_cached_summaries()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_cid, json, _ts)| serde_json::from_str(&json).ok())
            .collect()
    } else {
        Vec::new()
    };
    let summary_cache = std::sync::Arc::new(RwLock::new(initial_cache));
    let (summary_tx, _) = tokio::sync::broadcast::channel::<crate::output::ChannelSummary>(64);

    let state = std::sync::Arc::new(AppState {
        mm,
        llm,
        config,
        config_path,
        store,
        health: Arc::clone(&health),
        rag,
        llm_sem: std::sync::Arc::clone(&llm_sem),
        summarise_active: std::sync::Arc::clone(&summarise_active),
        seeding_progress: std::sync::Arc::clone(&seeding_progress),
        summary_cache: std::sync::Arc::clone(&summary_cache),
        summary_tx: summary_tx.clone(),
    });

    let app = create_router(Arc::clone(&state));

    // Try to pick up a socket from systemfd (for auto-reload), else bind normally
    let mut listenfd = ListenFd::from_env();
    let listener = match listenfd.take_tcp_listener(0)? {
        Some(std_listener) => {
            std_listener.set_nonblocking(true)?;
            let listener = tokio::net::TcpListener::from_std(std_listener)
                .context("failed to convert inherited socket")?;
            info!(addr = %listen_addr, "daemon listening (inherited socket)");
            listener
        }
        None => {
            let listener = tokio::net::TcpListener::bind(&listen_addr)
                .await
                .with_context(|| format!("failed to bind to {listen_addr}"))?;
            info!(addr = %listen_addr, "daemon listening");
            listener
        }
    };

    // Background task: verify Mattermost connectivity and update health state
    if let Some(mm_ref) = &state.mm {
        let mm_clone = mm_ref.clone();
        let health_clone = Arc::clone(&health);
        tokio::spawn(async move {
            match mm_clone.health_check().await {
                Ok(()) => {
                    let mut h = health_clone.write().await;
                    h.mm_ok = true;
                    h.error = None;
                    info!("Mattermost connectivity: ok");
                }
                Err(e) => {
                    let mut h = health_clone.write().await;
                    h.mm_ok = false;
                    h.error = Some(format!("{e}"));
                    warn!("Mattermost connectivity check failed: {e:#}");
                }
            }
        });
    }

    // Background task: first-run history seeding
    if let (Some(mm_ref), Some(llm_ref), Some(store_ref)) = (&state.mm, &state.llm, &state.store)
        && needs_seeding(store_ref)
    {
        let mm_seed = mm_ref.clone();
        let llm_seed = llm_ref.clone();
        let store_seed = store_ref.clone();
        let sem_seed = std::sync::Arc::clone(&state.llm_sem);
        let active_seed = std::sync::Arc::clone(&state.summarise_active);
        let progress_seed = std::sync::Arc::clone(&state.seeding_progress);
        tokio::spawn(async move {
            if let Err(e) = seed_history(
                &mm_seed,
                &llm_seed,
                &store_seed,
                sem_seed,
                active_seed,
                progress_seed,
            )
            .await
            {
                warn!("first-run seeding failed: {e:#}");
            }
        });
    }

    // Background task: periodic summarisation loop
    if let (Some(mm_ref), Some(llm_ref), Some(store_ref)) = (&state.mm, &state.llm, &state.store) {
        let mm_bg = mm_ref.clone();
        let llm_bg = llm_ref.clone();
        let store_bg = store_ref.clone();
        let rag_bg = state.rag.as_ref().map(Arc::clone);
        let sem_bg = std::sync::Arc::clone(&state.llm_sem);
        let active_bg = std::sync::Arc::clone(&state.summarise_active);
        let cache_bg = std::sync::Arc::clone(&state.summary_cache);
        let tx_bg = state.summary_tx.clone();
        let server_url_bg = state.config.mattermost.server_url.clone();
        let priority_users_bg = state.config.priority_users.clone();
        let poll_interval = state.config.server.poll_interval_secs;
        tokio::spawn(async move {
            background_summarise_loop(
                mm_bg,
                llm_bg,
                store_bg,
                rag_bg,
                sem_bg,
                active_bg,
                cache_bg,
                tx_bg,
                server_url_bg,
                priority_users_bg,
                poll_interval,
            )
            .await;
        });
    }

    axum::serve(listener, app).await.context("server error")?;

    Ok(())
}
