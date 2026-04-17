// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Channel summarisation orchestration.
//!
//! [`summarise_all_unread`] is the top-level entry point.  It enumerates teams
//! and channels, then spawns one Tokio task per channel via [`JoinSet`] so
//! Mattermost API fetches run concurrently.  A single-permit [`Semaphore`] gates
//! LLM calls to serialise them and avoid overwhelming the model server.
//!
//! History window: the start of the previous working day (Mon → Fri, otherwise
//! yesterday), or the stored watermark if it is more recent.  Posts are fetched
//! newest-first with page-based pagination to avoid the silent 200-post truncation
//! that the `since=` parameter causes.

use anyhow::{Context, Result};
use std::cmp::Reverse;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::llm::{FormattedMessage, LlmClient, TopicPoint};
use crate::mattermost::MattermostClient;
use crate::mattermost_types::{Channel, Post, Team, User};
use crate::output::{ActionItemSummary, ChannelSummary, TopicSection};
use crate::rag::{VectorStore, record_id};
use crate::store::{ChannelInsight, Store};

/// Format structured [`TopicPoint`]s as a Markdown string.
///
/// Falls back to `fallback` when the topics list is empty.
fn format_topics_markdown(topics: &[TopicPoint], fallback: &str) -> String {
    if topics.is_empty() {
        return fallback.to_string();
    }
    topics
        .iter()
        .map(|t| format!("### {}\n\n{}", t.title, t.summary))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Summarise all channels with unread messages across all teams.
///
/// Mattermost reads run concurrently per channel; LLM calls are serialised
/// through a single-permit semaphore to avoid overwhelming the model server.
#[allow(clippy::too_many_arguments)]
pub async fn summarise_all_unread(
    mm: &MattermostClient,
    llm: &LlmClient,
    channel_filter: Option<&str>,
    server_url: &str,
    store: Option<&Store>,
    priority_users: &[String],
    rag: Option<Arc<VectorStore>>,
    llm_sem: Arc<Semaphore>,
) -> Result<Vec<ChannelSummary>> {
    let me = mm.get_me().await.context("failed to get current user")?;
    info!(user = %me.username, id = %me.id, "authenticated as");

    let teams = mm
        .get_teams_for_user(&me.id)
        .await
        .context("failed to get teams")?;

    info!(count = teams.len(), "found teams");

    // Collect all (team, channel) pairs before spawning
    let mut work_items: Vec<(Team, Channel)> = Vec::new();
    for team in &teams {
        let channels = mm
            .get_channels_for_team_for_user(&me.id, &team.id)
            .await
            .context("failed to get channels")?;

        info!(team = %team.display_name, channels = channels.len(), "scanning team");

        for channel in channels {
            if let Some(filter) = channel_filter
                && channel.name != filter
                && channel.display_name != filter
            {
                continue;
            }
            work_items.push((team.clone(), channel));
        }
    }

    // One permit: LLM calls serialised while Mattermost fetches run concurrently
    let mut join_set: JoinSet<Option<ChannelSummary>> = JoinSet::new();

    for (team, channel) in work_items {
        let mm = mm.clone();
        let llm = llm.clone();
        let store = store.cloned();
        let user_id = me.id.clone();
        let server_url = server_url.to_string();
        let sem = Arc::clone(&llm_sem);
        let priority_users = priority_users.to_vec();
        let channel_label = channel.label().to_string();
        let channel_type = channel.channel_type.clone();
        let rag_clone = rag.as_ref().map(Arc::clone);

        join_set.spawn(async move {
            match summarise_channel(
                mm,
                llm,
                sem,
                user_id,
                team.display_name,
                team.name,
                server_url,
                channel,
                store,
                priority_users,
                rag_clone,
            )
            .await
            {
                Ok(result) => result,
                Err(e) => {
                    warn!(
                        channel = %channel_label,
                        channel_type = %channel_type,
                        "skipping channel: {e:#}"
                    );
                    None
                }
            }
        });
    }

    let mut summaries = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Some(s)) => summaries.push(s),
            Ok(None) => {}
            Err(e) => warn!("channel task panicked: {e:?}"),
        }
    }

    // Channels with mentions come first, then by descending mention count
    summaries.sort_by_key(|s| Reverse(s.mention_count));

    Ok(summaries)
}

#[allow(clippy::too_many_arguments)]
async fn summarise_channel(
    mm: MattermostClient,
    llm: LlmClient,
    llm_sem: Arc<Semaphore>,
    user_id: String,
    team_name: String,
    team_slug: String,
    server_url: String,
    channel: Channel,
    store: Option<Store>,
    priority_users: Vec<String>,
    rag: Option<Arc<VectorStore>>,
) -> Result<Option<ChannelSummary>> {
    let unread = mm
        .get_channel_unread(&user_id, &channel.id)
        .await
        .context("failed to get channel unread")?;

    info!(
        channel = %channel.label(),
        channel_type = %channel.channel_type,
        msg_count = unread.msg_count,
        mention_count = unread.mention_count,
        "unread check"
    );

    if unread.msg_count == 0 {
        return Ok(None);
    }

    info!(
        channel = %channel.label(),
        unread = unread.msg_count,
        mentions = unread.mention_count,
        "fetching posts"
    );

    // History boundary: start of previous working day, or stored watermark if more recent
    let working_day_since = history_since_ms();
    let since_ms: i64 = if let Some(s) = &store {
        match s.get_watermark(&channel.id) {
            Some(wm) if wm > working_day_since => wm,
            _ => working_day_since,
        }
    } else {
        working_day_since
    };

    // Fetch pages newest-first (page 0 = most recent). Using `since` param with per_page
    // returns oldest-first and silently truncates recent (unread) posts when the window
    // contains >200 posts — the opposite of what we need. Walk pages backward until
    // we pass the history boundary or exhaust the channel.
    let mut collected: Vec<Post> = Vec::new();
    for page in 0..10u32 {
        let batch = mm
            .get_posts_for_channel(&channel.id, None, Some(page), Some(200))
            .await
            .with_context(|| format!("failed to get posts page {page}"))?;

        if batch.posts.is_empty() {
            break;
        }

        let oldest_in_batch = batch
            .posts
            .values()
            .map(|p| p.create_at)
            .min()
            .unwrap_or(i64::MAX);
        collected.extend(batch.posts.into_values());

        if oldest_in_batch <= since_ms {
            break;
        }
    }

    // Sort ascending; discard anything that predates the history window
    collected.sort_by_key(|p| p.create_at);
    collected.retain(|p| p.create_at >= since_ms);

    let all_post_ids: HashSet<String> = collected.iter().map(|p| p.id.clone()).collect();
    let last_view = unread.last_view_at;

    // Partition: posts at or before last_view_at are context; later posts are unread
    let (context_posts, unread_posts): (Vec<Post>, Vec<Post>) = collected
        .into_iter()
        .filter(|p| p.is_user_message())
        .partition(|p| p.create_at <= last_view);

    // Fetch parent threads for unread replies whose root is outside the history window
    let mut extra_context: Vec<Post> = Vec::new();
    let mut fetched_roots: HashSet<String> = HashSet::new();
    for post in &unread_posts {
        if post.root_id.is_empty() || all_post_ids.contains(&post.root_id) {
            continue;
        }
        if !fetched_roots.insert(post.root_id.clone()) {
            continue;
        }
        match mm.get_post_thread(&post.root_id).await {
            Ok(thread) => {
                for tp in thread.sorted_posts() {
                    if tp.is_user_message() && tp.create_at <= last_view {
                        extra_context.push(tp.clone());
                    }
                }
            }
            Err(e) => warn!("failed to fetch thread {}: {e:#}", post.root_id),
        }
    }

    let mut context_formatted = build_formatted(&mm, &context_posts).await;
    let (extra_fmt, _, _) = build_formatted(&mm, &extra_context).await;
    context_formatted.0.extend(extra_fmt);
    let (unread_formatted, participants, mut known_usernames) =
        build_formatted(&mm, &unread_posts).await;
    // Merge usernames from context so mentions in unread messages are highlighted
    // even when the mentioned user only appears in the context window.
    let (_, _, ctx_usernames) = build_formatted(&mm, &context_posts).await;
    known_usernames.extend(ctx_usernames);

    if unread_formatted.is_empty() {
        return Ok(None);
    }

    // Load unresolved action items from previous runs to give the LLM continuity
    let prior_items: Vec<String> = if let Some(s) = &store {
        s.get_pending_action_items(&channel.id)
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.text)
            .collect()
    } else {
        Vec::new()
    };

    let is_dm = channel.channel_type == "D" || channel.channel_type == "G";

    // Compute a human-friendly channel name once; re-used for RAG, insights, and the summary.
    let channel_display_name = if is_dm && !participants.is_empty() {
        participants.join(", ")
    } else if is_dm {
        resolve_dm_display_name(&mm, &channel, &user_id).await
    } else {
        channel.label().to_string()
    };

    // RAG: query historical context before acquiring the LLM semaphore
    let historical_ctx: Option<String> = if let Some(ref vs) = rag {
        let query_text: String = unread_formatted
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(2000)
            .collect();

        match vs.query_similar(&channel.id, &query_text, 5).await {
            Ok(records) if !records.is_empty() => {
                debug!(
                    channel = %channel.label(),
                    count = records.len(),
                    "injecting RAG historical context"
                );
                Some(VectorStore::format_context(&records))
            }
            Ok(_) => None,
            Err(e) => {
                warn!("RAG query failed for channel {}: {e:#}", channel.label());
                None
            }
        }
    } else {
        None
    };

    // Acquire the semaphore to serialise LLM calls (avoids memory spikes)
    let _permit = llm_sem.acquire().await.expect("LLM semaphore closed");
    let (llm_result, _) = llm
        .summarise(
            channel.label(),
            &context_formatted.0,
            &unread_formatted,
            &prior_items,
            &priority_users,
            is_dm,
            historical_ctx.as_deref(),
        )
        .await
        .context("LLM summarisation failed")?;
    drop(_permit);

    // Persist new action items and advance the watermark
    let action_items: Vec<ActionItemSummary> = if let Some(s) = &store {
        let now = jiff::Timestamp::now().as_millisecond();
        if let Err(e) = s.upsert_action_items(&channel.id, &llm_result.action_items, now) {
            warn!("failed to upsert action items for {}: {e:#}", channel.id);
        }
        if let Some(latest) = unread_posts.iter().map(|p| p.create_at).max()
            && let Err(e) = s.set_watermark(&channel.id, latest)
        {
            warn!("failed to store watermark for {}: {e:#}", channel.id);
        }
        s.get_pending_action_items(&channel.id)
            .unwrap_or_default()
            .into_iter()
            .map(|a| ActionItemSummary {
                id: a.id,
                text: a.text,
            })
            .collect()
    } else {
        llm_result
            .action_items
            .iter()
            .enumerate()
            .map(|(i, t)| ActionItemSummary {
                id: format!("ephemeral-{i}"),
                text: t.clone(),
            })
            .collect()
    };

    // RAG: store raw message chunks for future context enrichment
    if let Some(ref vs) = rag {
        let now_ms = jiff::Timestamp::now().as_millisecond();
        let summary_for_rag = format_topics_markdown(&llm_result.topics, &llm_result.summary);

        // Build formatted message strings so the embedding captures the actual
        // conversation content rather than (a summary of) the LLM output.
        let message_texts: Vec<String> = unread_formatted
            .iter()
            .map(|m| format!("[{}] {}: {}", m.timestamp, m.username, m.content))
            .collect();

        if let Err(e) = vs
            .store_thread_chunks(
                &channel.id,
                &channel_display_name,
                now_ms,
                &message_texts,
                &summary_for_rag,
            )
            .await
        {
            warn!(
                "failed to store RAG thread chunks for channel {}: {e:#}",
                channel.label()
            );
        }
    }

    // Persist channel insight snapshot to the store for cross-channel synthesis
    if let Some(ref s) = store {
        let now_ms = jiff::Timestamp::now().as_millisecond();
        // Use the latest unread post creation time so insights are filed under
        // when the conversation happened, not when we processed it.
        let insight_ts = unread_posts
            .iter()
            .map(|p| p.create_at)
            .max()
            .unwrap_or(now_ms);
        // Build summary_text from structured topics; fall back to flat summary
        let summary_text = format_topics_markdown(&llm_result.topics, &llm_result.summary);
        let insight_topics: Vec<String> =
            llm_result.topics.iter().map(|t| t.title.clone()).collect();
        let insight = ChannelInsight {
            id: record_id(&channel.id, insight_ts),
            channel_id: channel.id.clone(),
            channel_name: channel_display_name.clone(),
            team_name: team_name.clone(),
            summary_text,
            action_items: llm_result.action_items.clone(),
            topics: insight_topics,
            importance_score: 0.0,
            risk_score: 0.0,
            timestamp_ms: insight_ts,
            seeded: false,
            unread_count: unread.msg_count,
            mention_count: unread.mention_count,
        };
        if let Err(e) = s.upsert_channel_insight(&insight) {
            warn!(
                "failed to store channel insight for {}: {e:#}",
                channel.label()
            );
        }
    }

    let channel_url = if !team_slug.is_empty() {
        format!(
            "{}/{}/channels/{}",
            server_url.trim_end_matches('/'),
            team_slug,
            channel.name
        )
    } else {
        format!(
            "{}/messages/{}",
            server_url.trim_end_matches('/'),
            channel.name
        )
    };

    // For DM / group channels the API display_name is empty; use participant names instead.
    let channel_name = channel_display_name;

    let topic_sections: Vec<TopicSection> = llm_result
        .topics
        .iter()
        .map(|t| {
            let highlighted = highlight_mentions(&t.summary, &known_usernames);
            TopicSection {
                title: t.title.clone(),
                summary_html: markdown_to_html(&highlighted),
                summary_md: t.summary.clone(),
            }
        })
        .collect();

    // summary_html: join topic sections for CLI / fallback rendering
    let highlighted_summary = highlight_mentions(&llm_result.summary, &known_usernames);
    let summary_html = if !topic_sections.is_empty() {
        topic_sections
            .iter()
            .map(|t| format!("<h4>{}</h4>{}", t.title, t.summary_html))
            .collect::<Vec<_>>()
            .join("")
    } else {
        markdown_to_html(&highlighted_summary)
    };

    Ok(Some(ChannelSummary {
        team_name,
        channel_name,
        channel_id: channel.id.clone(),
        channel_url,
        unread_count: unread.msg_count,
        mention_count: unread.mention_count,
        summary: llm_result.summary,
        summary_html,
        topics: topic_sections,
        action_items,
        topic: llm_result.topic,
        participants: if is_dm { participants } else { Vec::new() },
    }))
}

/// Resolve a human-readable display name for DM channels whose `name` field is
/// `userid1__userid2`. Looks up both user IDs via the Mattermost API (cached)
/// and returns the other user's display name (or both if `my_user_id` doesn't
/// match either side).
async fn resolve_dm_display_name(
    mm: &MattermostClient,
    channel: &Channel,
    my_user_id: &str,
) -> String {
    let parts: Vec<&str> = channel.name.split("__").collect();
    if parts.len() == 2 {
        let mut names = Vec::new();
        for uid in &parts {
            if *uid == my_user_id {
                continue; // skip "me" — show only the other person
            }
            match mm.get_user(uid).await {
                Ok(user) => names.push(user.display_name().to_string()),
                Err(_) => names.push((*uid).to_string()),
            }
        }
        if !names.is_empty() {
            return names.join(", ");
        }
    }
    channel.label().to_string()
}

async fn build_formatted(
    mm: &MattermostClient,
    posts: &[Post],
) -> (
    Vec<FormattedMessage>,
    Vec<String>,
    std::collections::HashSet<String>,
) {
    let mut result = Vec::new();
    // Use IndexSet semantics via a Vec + seen-check to keep insertion order and dedup.
    let mut participants: Vec<String> = Vec::new();
    let mut seen_users: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut usernames: std::collections::HashSet<String> = std::collections::HashSet::new();

    for post in posts {
        let user = mm.get_user(&post.user_id).await.unwrap_or_else(|_| User {
            id: post.user_id.clone(),
            username: "unknown".to_string(),
            first_name: String::new(),
            last_name: String::new(),
            nickname: String::new(),
            email: String::new(),
        });

        let display = user.display_name().to_string();
        let uname = user.username.clone();

        // Track unique participant display names in the order first seen
        if seen_users.insert(user.id.clone()) {
            participants.push(display.clone());
            usernames.insert(uname.to_lowercase());
        }

        let ts = jiff::Timestamp::from_millisecond(post.create_at)
            .ok()
            .map(|t| t.strftime("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        result.push(FormattedMessage {
            timestamp: ts,
            username: format!("{display} (@{uname})"),
            content: post.message.clone(),
        });
    }
    (result, participants, usernames)
}

/// Wrap `@username` tokens in the markdown text with a `<span class="mention">` tag,
/// but only when the username (lowercased) is in `known_usernames`.
///
/// Works token-by-token so it avoids corrupting markdown syntax.
fn highlight_mentions(md: &str, known_usernames: &std::collections::HashSet<String>) -> String {
    if known_usernames.is_empty() {
        return md.to_string();
    }
    md.split_inclusive(|c: char| c.is_whitespace())
        .map(|token| {
            // Strip trailing punctuation/whitespace to isolate the word
            let word =
                token.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
            if let Some(uname) = word.strip_prefix('@')
                && known_usernames.contains(&uname.to_lowercase())
            {
                let suffix = &token[word.len()..];
                return format!("<span class=\"mention\">@{uname}</span>{suffix}");
            }
            token.to_string()
        })
        .collect()
}

/// Streaming variant of [`summarise_all_unread`].
///
/// Sends each completed [`ChannelSummary`] on `tx` as soon as it is ready
/// (i.e. in completion order, not sorted).  When the returned future resolves
/// the caller should emit a `{"done":true}` sentinel to signal end-of-stream.
#[allow(clippy::too_many_arguments)]
pub async fn summarise_all_unread_stream(
    tx: tokio::sync::mpsc::Sender<ChannelSummary>,
    mm: &MattermostClient,
    llm: &LlmClient,
    channel_filter: Option<&str>,
    server_url: &str,
    store: Option<&Store>,
    priority_users: &[String],
    rag: Option<Arc<VectorStore>>,
    llm_sem: Arc<Semaphore>,
) -> Result<()> {
    let me = mm.get_me().await.context("failed to get current user")?;
    info!(user = %me.username, id = %me.id, "authenticated as (stream)");

    let teams = mm
        .get_teams_for_user(&me.id)
        .await
        .context("failed to get teams")?;

    let mut work_items: Vec<(Team, Channel)> = Vec::new();
    for team in &teams {
        let channels = mm
            .get_channels_for_team_for_user(&me.id, &team.id)
            .await
            .context("failed to get channels")?;

        for channel in channels {
            if let Some(filter) = channel_filter
                && channel.name != filter
                && channel.display_name != filter
            {
                continue;
            }
            work_items.push((team.clone(), channel));
        }
    }

    let mut join_set: JoinSet<Option<ChannelSummary>> = JoinSet::new();

    for (team, channel) in work_items {
        let mm = mm.clone();
        let llm = llm.clone();
        let store = store.cloned();
        let rag = rag.as_ref().map(Arc::clone);
        let user_id = me.id.clone();
        let server_url = server_url.to_string();
        let sem = Arc::clone(&llm_sem);
        let priority_users = priority_users.to_vec();
        let channel_label = channel.label().to_string();
        let channel_type = channel.channel_type.clone();

        join_set.spawn(async move {
            match summarise_channel(
                mm,
                llm,
                sem,
                user_id,
                team.display_name,
                team.name,
                server_url,
                channel,
                store,
                priority_users,
                rag,
            )
            .await
            {
                Ok(result) => result,
                Err(e) => {
                    warn!(
                        channel = %channel_label,
                        channel_type = %channel_type,
                        "skipping channel: {e:#}"
                    );
                    None
                }
            }
        });
    }

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Some(s)) => {
                if tx.send(s).await.is_err() {
                    // Receiver dropped (client disconnected) — stop work
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => warn!("channel task panicked: {e:?}"),
        }
    }

    Ok(())
}

fn markdown_to_html(md: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(md, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Background poll loop: periodically re-summarises channels with unread messages.
///
/// Each completed summary is persisted to the store cache and broadcast via the
/// `summary_tx` channel for SSE subscribers.  The in-memory `summary_cache` is
/// *replaced* after each full cycle so the REST endpoint always returns the latest
/// snapshot.
#[allow(clippy::too_many_arguments)]
pub async fn background_summarise_loop(
    mm: MattermostClient,
    llm: LlmClient,
    store: Store,
    rag: Option<Arc<VectorStore>>,
    llm_sem: Arc<Semaphore>,
    summarise_active: Arc<std::sync::atomic::AtomicUsize>,
    summary_cache: Arc<tokio::sync::RwLock<Vec<crate::output::ChannelSummary>>>,
    summary_tx: tokio::sync::broadcast::Sender<crate::output::ChannelSummary>,
    server_url: String,
    priority_users: Vec<String>,
    poll_interval_secs: u64,
) {
    use std::sync::atomic::Ordering;

    if poll_interval_secs == 0 {
        info!("background summarise loop disabled (poll_interval_secs = 0)");
        return;
    }

    let interval = std::time::Duration::from_secs(poll_interval_secs);
    info!(
        interval_secs = poll_interval_secs,
        "background summarise loop starting"
    );

    loop {
        tokio::time::sleep(interval).await;

        debug!("background summarise: starting cycle");

        summarise_active.fetch_add(1, Ordering::AcqRel);
        let result = summarise_all_unread(
            &mm,
            &llm,
            None,
            &server_url,
            Some(&store),
            &priority_users,
            rag.as_ref().map(Arc::clone),
            Arc::clone(&llm_sem),
        )
        .await;
        summarise_active.fetch_sub(1, Ordering::AcqRel);

        match result {
            Ok(summaries) => {
                info!(
                    count = summaries.len(),
                    "background summarise: cycle complete"
                );

                // Persist each summary to the SQLite cache
                for s in &summaries {
                    if let Ok(json) = serde_json::to_string(s)
                        && let Err(e) = store.set_cached_summary(&s.channel_id, &json)
                    {
                        warn!(channel = %s.channel_name, "failed to cache summary: {e:#}");
                    }
                    // Broadcast to SSE subscribers (ignore error = no subscribers)
                    let _ = summary_tx.send(s.clone());
                }

                // Replace the in-memory cache
                {
                    let mut cache = summary_cache.write().await;
                    *cache = summaries;
                }
            }
            Err(e) => {
                warn!("background summarise cycle failed: {e:#}");
            }
        }
    }
}

/// Returns the start of the previous working day in milliseconds since epoch.
fn history_since_ms() -> i64 {
    use jiff::civil::Weekday;
    let now = jiff::Zoned::now();
    let days_back: i64 = match now.weekday() {
        Weekday::Monday => 3, // back to Friday
        Weekday::Saturday => 1,
        Weekday::Sunday => 2,
        _ => 1, // Tue–Fri: yesterday
    };
    now.checked_sub(jiff::Span::new().days(days_back))
        .and_then(|target| target.start_of_day())
        .map(|sod| sod.timestamp().as_millisecond())
        .unwrap_or_else(|_| now.timestamp().as_millisecond() - days_back * 86_400_000)
}
