// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Email summarisation: fetches unread IMAP messages, calls the LLM, and
//! produces [`EmailSummary`] values to be cached and broadcast via SSE.
//!
//! The background loop mirrors the Mattermost [`background_summarise_loop`]
//! pattern: sleep → summarise all mailboxes → persist + broadcast.

use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::EmailConfig;
use crate::email::ImapClient;
use crate::llm::LlmClient;
use crate::output::{ActionItemSummary, EmailMeta, EmailSummary, TopicSection};
use crate::store::Store;
use crate::summarise::markdown_to_html;

/// Summarise all configured mailboxes and return one [`EmailSummary`] per mailbox
/// that had unread messages.
pub async fn summarise_all_mailboxes(
    config: &EmailConfig,
    llm: &LlmClient,
    store: &Store,
    llm_sem: Arc<tokio::sync::Semaphore>,
    mailboxes_override: Option<&[crate::config::MailboxConfig]>,
) -> Result<Vec<EmailSummary>> {
    let mut results = Vec::new();

    let mailboxes = mailboxes_override.unwrap_or(&config.mailboxes);
    for mailbox in mailboxes {
        if !mailbox.enabled {
            info!(mailbox = %mailbox.name, "skipping disabled mailbox");
            continue;
        }
        match summarise_mailbox(config, llm, store, mailbox.name.as_str(), mailbox.instructions.as_deref(), Arc::clone(&llm_sem)).await {
            Ok(Some(s)) => results.push(s),
            Ok(None) => {}
            Err(e) => warn!(mailbox = %mailbox.name, "email summarise failed: {e:#}"),
        }
    }

    Ok(results)
}

async fn summarise_mailbox(
    config: &EmailConfig,
    llm: &LlmClient,
    store: &Store,
    mailbox: &str,
    instructions: Option<&str>,
    llm_sem: Arc<tokio::sync::Semaphore>,
) -> Result<Option<EmailSummary>> {
    let since_uid = store.get_email_watermark(mailbox);

    // Connect fresh each cycle; simpler than maintaining idle IMAP sessions
    let mut client = ImapClient::connect(config)
        .await
        .with_context(|| format!("IMAP connect failed for mailbox {mailbox}"))?;

    // On first run (since_uid == 0) fetch up to 7 days back
    let messages = client
        .fetch_unread_since_uid(mailbox, since_uid, 7)
        .await
        .with_context(|| format!("failed to fetch messages from {mailbox}"))?;

    let _ = client.logout().await;

    if messages.is_empty() {
        return Ok(None);
    }

    let unread_count = messages.len();
    let max_uid = messages.iter().map(|m| m.uid).max().unwrap_or(0);

    // Build (from, subject, date, body) tuples for the LLM
    let email_tuples: Vec<(String, String, String, String)> = messages
        .iter()
        .map(|m| (m.from.clone(), m.subject.clone(), m.date.clone(), m.body.clone()))
        .collect();

    let email_metas: Vec<EmailMeta> = messages
        .iter()
        .map(|m| EmailMeta {
            subject: m.subject.clone(),
            from: m.from.clone(),
            date: m.date.clone(),
        })
        .collect();

    // Load prior action items for this mailbox
    let prior_items: Vec<String> = store
        .get_pending_action_items(mailbox)
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.text)
        .collect();

    // Acquire LLM semaphore (shared with Mattermost summariser)
    let _permit = llm_sem.acquire().await.context("LLM semaphore closed")?;

    let (llm_result, _raw) = llm
        .summarise_emails(mailbox, &email_tuples, &prior_items, instructions)
        .await
        .with_context(|| format!("LLM summarise_emails failed for {mailbox}"))?;

    drop(_permit);

    // Persist action items
    let now = jiff::Timestamp::now().as_millisecond();
    if let Err(e) = store.upsert_action_items(mailbox, &llm_result.action_items, now, "email") {
        warn!(mailbox = %mailbox, "failed to upsert email action items: {e:#}");
    }

    // NOTE: watermark is NOT advanced here. It only advances when the user
    // explicitly clicks "Mark as read", so that summaries persist across cycles
    // until the user dismisses them — matching the chat channel model.

    // Build action item summaries from live store (includes claimed/resolved state)
    let action_items: Vec<ActionItemSummary> = store
        .get_pending_action_items(mailbox)
        .unwrap_or_default()
        .into_iter()
        .map(|a| ActionItemSummary {
            id: a.id,
            text: a.text,
            claimed: a.claimed,
        })
        .collect();

    // Render topic sections
    let topic_sections: Vec<TopicSection> = llm_result
        .topics
        .iter()
        .map(|t| {
            TopicSection {
                title: t.title.clone(),
                summary_html: markdown_to_html(&t.summary),
                summary_md: t.summary.clone(),
            }
        })
        .collect();

    let summary_html = if !topic_sections.is_empty() {
        topic_sections
            .iter()
            .map(|t| format!("<h4>{}</h4>{}", t.title, t.summary_html))
            .collect::<Vec<_>>()
            .join("")
    } else {
        markdown_to_html(&llm_result.summary)
    };

    let mailbox_id = mailbox
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect::<String>()
        .to_lowercase();

    Ok(Some(EmailSummary {
        mailbox: mailbox.to_string(),
        mailbox_id,
        unread_count,
        summary: llm_result.summary,
        summary_html,
        topics: topic_sections,
        action_items,
        emails: email_metas,
        max_uid,
    }))
}

/// Periodic background loop: summarise all configured mailboxes every
/// `poll_interval_secs` seconds, persist to cache, and broadcast via SSE.
pub async fn background_email_summarise_loop(
    config: EmailConfig,
    llm: LlmClient,
    store: Store,
    llm_sem: Arc<tokio::sync::Semaphore>,
    email_cache: Arc<tokio::sync::RwLock<Vec<EmailSummary>>>,
    email_tx: tokio::sync::broadcast::Sender<EmailSummary>,
    poll_interval_secs: u64,
    email_mailbox_config: Arc<tokio::sync::RwLock<Vec<crate::config::MailboxConfig>>>,
) {
    if poll_interval_secs == 0 {
        info!("email background loop disabled (poll_interval_secs = 0)");
        return;
    }

    let interval = std::time::Duration::from_secs(poll_interval_secs);
    info!(
        interval_secs = poll_interval_secs,
        mailboxes = config.mailboxes.len(),
        "email summarise loop starting"
    );

    loop {
        tokio::time::sleep(interval).await;

        info!("email summarise: starting cycle");
        let live_mailboxes = email_mailbox_config.read().await.clone();
        match summarise_all_mailboxes(&config, &llm, &store, Arc::clone(&llm_sem), Some(&live_mailboxes)).await {
            Ok(summaries) => {
                info!(count = summaries.len(), "email summarise: cycle complete");
                // Merge into the existing cache instead of replacing wholesale.
                // Mailboxes with no new unread messages are not returned by
                // summarise_all_mailboxes, but their previous summaries should
                // remain visible until the user marks them as read.
                let mut cache = email_cache.write().await;
                for s in summaries {
                    if let Ok(json) = serde_json::to_string(&s)
                        && let Err(e) = store.set_cached_email_summary(&s.mailbox, &json)
                    {
                        warn!(mailbox = %s.mailbox, "failed to cache email summary: {e:#}");
                    }
                    let _ = email_tx.send(s.clone());
                    if let Some(existing) = cache.iter_mut().find(|c| c.mailbox == s.mailbox) {
                        *existing = s;
                    } else {
                        cache.push(s);
                    }
                }
            }
            Err(e) => warn!("email summarise cycle failed: {e:#}"),
        }
    }
}
