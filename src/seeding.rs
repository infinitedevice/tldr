// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! First-run history seeding.
//!
//! When the store has no watermarks and no insights, [`seed_history`] backfills
//! approximately one month of channel history by splitting it into weekly batches
//! and calling the LLM for each batch.  Results are stored as [`ChannelInsight`]
//! records so the Insights page has data from day one.
//!
//! Seeding runs in a background task and does **not** block startup.  Progress is
//! tracked via a `user_pref` key `seeding_done` so it only runs once.

use anyhow::{Context, Result};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::llm::{FormattedMessage, LlmClient};
use crate::mattermost::MattermostClient;
use crate::server::SeedingProgress;
use crate::store::{ChannelInsight, Store};

/// Number of weeks to seed back from now.
const SEED_WEEKS: u32 = 4;

/// Milliseconds in one week.
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Returns true when there is no seeding data yet.
///
/// Uses the `seeding_done` preference key in the store.
pub fn needs_seeding(store: &Store) -> bool {
    store.get_pref("seeding_done").is_none()
}

/// Seed the store with ~1 month of historical channel summaries.
///
/// For each subscribed channel, splits the last `SEED_WEEKS` weeks into weekly
/// windows and calls the LLM to summarise each window.  Results are stored as
/// `ChannelInsight` records.  Marks `seeding_done` in user prefs when finished.
///
/// This is safe to run concurrently while the daemon serves requests; it only
/// writes to `channel_insights` and `user_prefs`.
///
/// `llm_sem` is the **daemon-wide** LLM semaphore (1 permit) so seeding and live
/// summarise calls are globally serialised and cannot overload the model server.
///
/// `summarise_active` — incremented by live summarise handlers while they hold
/// the semaphore priority; seeding yields (busy-waits with a short sleep) until
/// it drops back to 0 before each LLM call.
///
/// `seeding_progress` — updated as channels complete so the frontend can display
/// a progress bar.
pub async fn seed_history(
    mm: &MattermostClient,
    llm: &LlmClient,
    store: &Store,
    llm_sem: Arc<tokio::sync::Semaphore>,
    summarise_active: Arc<AtomicUsize>,
    seeding_progress: Arc<RwLock<SeedingProgress>>,
) -> Result<()> {
    info!("starting first-run history seeding ({SEED_WEEKS} weeks back)");

    let me = mm
        .get_me()
        .await
        .context("seed: failed to get current user")?;
    let teams = mm
        .get_teams_for_user(&me.id)
        .await
        .context("seed: failed to get teams")?;

    let now_ms = jiff::Timestamp::now().as_millisecond();
    // Seed from 4 weeks ago up to now
    let seed_from_ms = now_ms - SEED_WEEKS as i64 * WEEK_MS;

    // Collect all channels first
    let mut channels: Vec<(String, String, String)> = Vec::new(); // (team_display, ch_id, ch_name)
    for team in &teams {
        match mm.get_channels_for_team_for_user(&me.id, &team.id).await {
            Ok(chs) => {
                for ch in chs {
                    if ch.channel_type == "D" || ch.channel_type == "G" {
                        continue; // Skip DMs / group messages for seeding
                    }
                    channels.push((
                        team.display_name.clone(),
                        ch.id.clone(),
                        ch.display_name.clone(),
                    ));
                }
            }
            Err(e) => warn!(
                "seed: failed to get channels for {}: {e:#}",
                team.display_name
            ),
        }
    }

    info!(
        "seeding {} channels over {} weeks each",
        channels.len(),
        SEED_WEEKS
    );

    {
        let mut p = seeding_progress.write().await;
        p.in_progress = true;
        p.total_channels = channels.len();
        p.completed_channels = 0;
    }

    // Process channels sequentially — each LLM call already acquires the shared semaphore.
    for (team_name, channel_id, channel_name) in channels {
        // Yield while a live summarise request is in-flight so it gets priority.
        while summarise_active.load(Ordering::Acquire) > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }

        if let Err(e) = seed_channel(
            mm,
            llm,
            store,
            &team_name,
            &channel_id,
            &channel_name,
            seed_from_ms,
            now_ms,
            Arc::clone(&llm_sem),
            Arc::clone(&summarise_active),
        )
        .await
        {
            warn!("seed: channel {channel_name}: {e:#}");
        }

        {
            let mut p = seeding_progress.write().await;
            p.completed_channels += 1;
        }
    }

    store
        .set_pref("seeding_done", "1")
        .context("seed: failed to set seeding_done")?;
    {
        let mut p = seeding_progress.write().await;
        p.in_progress = false;
        p.seeded = true;
    }
    info!("first-run history seeding complete");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn seed_channel(
    mm: &MattermostClient,
    llm: &LlmClient,
    store: &Store,
    team_name: &str,
    channel_id: &str,
    channel_name: &str,
    seed_from_ms: i64,
    now_ms: i64,
    llm_sem: Arc<tokio::sync::Semaphore>,
    summarise_active: Arc<AtomicUsize>,
) -> Result<()> {
    // Fetch all posts within the seed window (page 0 = most recent, etc.)
    let mut all_posts: std::collections::HashMap<String, crate::mattermost_types::Post> =
        std::collections::HashMap::new();
    let mut all_order: Vec<String> = Vec::new();
    for page in 0..20u32 {
        let batch = mm
            .get_posts_for_channel(channel_id, None, Some(page), Some(200))
            .await
            .with_context(|| format!("failed to fetch posts page {page} for {channel_name}"))?;
        let oldest = batch
            .order
            .iter()
            .filter_map(|id| batch.posts.get(id))
            .map(|p| p.create_at)
            .min()
            .unwrap_or(now_ms);
        let is_last = batch.order.len() < 200;
        for id in &batch.order {
            if !all_order.contains(id) {
                all_order.push(id.clone());
            }
        }
        for (k, v) in batch.posts {
            all_posts.insert(k, v);
        }
        if oldest < seed_from_ms || is_last {
            break;
        }
    }

    // Filter to seed window and sort ascending
    let mut posts: Vec<&crate::mattermost_types::Post> = all_posts
        .values()
        .filter(|p| p.create_at >= seed_from_ms && !p.message.trim().is_empty())
        .collect();
    posts.sort_by_key(|p| p.create_at);

    if posts.is_empty() {
        debug!("seed: no posts for {channel_name} in range, skipping");
        return Ok(());
    }

    // Build a simple user cache from post usernames (we use user_id as fallback)
    let mut user_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // Split into weekly windows
    for week in 0..SEED_WEEKS {
        let window_start = seed_from_ms + week as i64 * WEEK_MS;
        let window_end = window_start + WEEK_MS;

        let window_posts: Vec<FormattedMessage> = posts
            .iter()
            .filter(|p| p.create_at >= window_start && p.create_at < window_end)
            .map(|p| {
                let ts = jiff::Timestamp::from_millisecond(p.create_at)
                    .ok()
                    .map(|t| t.strftime("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                let username = user_cache
                    .entry(p.user_id.clone())
                    .or_insert_with(|| p.user_id.clone())
                    .clone();
                FormattedMessage {
                    timestamp: ts,
                    username,
                    content: p.message.clone(),
                }
            })
            .collect();

        if window_posts.is_empty() {
            continue;
        }

        debug!(
            "seed: channel={channel_name} week={week} posts={}",
            window_posts.len()
        );

        let window_mid_ms = window_start + WEEK_MS / 2;

        // Priority loop: yield while live summarise is active, then race to acquire
        // the semaphore. If a new summarise started between the yield check and
        // acquire we release and yield again.
        let _permit = loop {
            while summarise_active.load(Ordering::Acquire) > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            let p = llm_sem.acquire().await.expect("LLM semaphore closed");
            if summarise_active.load(Ordering::Acquire) == 0 {
                break p;
            }
            // A live request snuck in — drop the permit and yield again.
            drop(p);
        };
        match llm
            .summarise(channel_name, &[], &window_posts, &[], &[], false, None)
            .await
        {
            Ok((result, _raw)) => {
                let id = format!("seed-{channel_id}-{window_start}");
                // Build summary_text from topic sections if the LLM returned them
                let summary_text = if !result.topics.is_empty() {
                    result
                        .topics
                        .iter()
                        .map(|t| format!("### {}\n\n{}", t.title, t.summary))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                } else {
                    result.summary.clone()
                };
                let insight_topics: Vec<String> =
                    result.topics.iter().map(|t| t.title.clone()).collect();
                let insight = ChannelInsight {
                    id,
                    channel_id: channel_id.to_string(),
                    channel_name: channel_name.to_string(),
                    team_name: team_name.to_string(),
                    summary_text,
                    action_items: result.action_items,
                    topics: insight_topics,
                    importance_score: 0.0,
                    risk_score: 0.0,
                    timestamp_ms: window_mid_ms,
                    seeded: true,
                    unread_count: 0,
                    mention_count: 0,
                };
                if let Err(e) = store.upsert_channel_insight(&insight) {
                    warn!("seed: failed to store insight for {channel_name} week {week}: {e:#}");
                }
            }
            Err(e) => warn!("seed: LLM failed for {channel_name} week {week}: {e:#}"),
        }
    }

    Ok(())
}
