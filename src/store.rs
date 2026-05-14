// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! SQLite persistence layer for channel watermarks and action items.
//!
//! [`Store`] wraps a [`rusqlite::Connection`] behind an `Arc<Mutex<_>>` so it can
//! be cloned cheaply and shared across async tasks.  Schema is created on first open
//! via `CREATE TABLE IF NOT EXISTS`.
//!
//! Action item IDs are a 64-bit hash of `(channel_id, text)`, giving stable,
//! collision-resistant deduplication across repeated summarise runs.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A tracked action item extracted from a channel summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItem {
    pub id: String,
    pub channel_id: String,
    pub text: String,
    pub created_at: i64,
    pub resolved: bool,
    pub ignored: bool,
    pub claimed: bool,
    /// Origin of this item: "mattermost" or "email".
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "mattermost".to_string()
}

/// A per-channel insight snapshot stored after each summarise run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInsight {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub team_name: String,
    pub summary_text: String,
    pub action_items: Vec<String>,
    pub topics: Vec<String>,
    pub importance_score: f64,
    pub risk_score: f64,
    pub timestamp_ms: i64,
    /// True when this record was written by first-run history seeding.
    /// Seeded records are preserved by `clear_all` / `clear_channel` so that
    /// the Insights page always has a historical baseline.
    #[serde(default)]
    pub seeded: bool,
    /// Unread message count at the time of summarisation (0 for seeded rows).
    #[serde(default)]
    pub unread_count: i64,
    /// Mention count at the time of summarisation (0 for seeded rows).
    #[serde(default)]
    pub mention_count: i64,
}

/// A locally-favourited channel (tool-internal, not synced to Mattermost).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavouriteChannel {
    pub channel_id: String,
    pub channel_name: String,
    pub team_name: String,
    pub added_at: i64,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dirs for {}", path.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite db at {}", path.display()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channel_watermark (
                channel_id   TEXT PRIMARY KEY,
                last_post_at INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS action_item (
                id          TEXT PRIMARY KEY,
                channel_id  TEXT NOT NULL,
                text        TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                resolved    INTEGER NOT NULL DEFAULT 0,
                ignored     INTEGER NOT NULL DEFAULT 0,
                resolved_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS user_prefs (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS channel_insights (
                id                  TEXT PRIMARY KEY,
                channel_id          TEXT NOT NULL,
                channel_name        TEXT NOT NULL,
                team_name           TEXT NOT NULL,
                summary_text        TEXT NOT NULL,
                action_items_json   TEXT NOT NULL DEFAULT '[]',
                topics_json         TEXT NOT NULL DEFAULT '[]',
                importance_score    REAL NOT NULL DEFAULT 0.0,
                risk_score          REAL NOT NULL DEFAULT 0.0,
                timestamp_ms        INTEGER NOT NULL,
                created_at          INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_channel_insights_ts ON channel_insights(timestamp_ms);
            CREATE TABLE IF NOT EXISTS favourite_channel (
                channel_id   TEXT PRIMARY KEY,
                channel_name TEXT NOT NULL,
                team_name    TEXT NOT NULL,
                added_at     INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS cached_summary (
                channel_id   TEXT PRIMARY KEY,
                summary_json TEXT NOT NULL,
                updated_at   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS email_watermark (
                mailbox    TEXT NOT NULL PRIMARY KEY,
                last_uid   INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS cached_email_summary (
                mailbox      TEXT NOT NULL PRIMARY KEY,
                summary_json TEXT NOT NULL,
                updated_at   INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS source_config (
                source_type  TEXT NOT NULL,
                name         TEXT NOT NULL,
                team         TEXT NOT NULL DEFAULT '',
                enabled      INTEGER NOT NULL DEFAULT 1,
                instructions TEXT,
                updated_at   INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (source_type, name, team)
            );",
        )
        .context("failed to initialise database schema")?;

        // Migration: add `seeded` column if it doesn't exist yet (safe on fresh DBs too —
        // SQLite returns "duplicate column name" on an existing column; we ignore it).
        let _ = conn.execute(
            "ALTER TABLE channel_insights ADD COLUMN seeded INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channel_insights ADD COLUMN unread_count INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE channel_insights ADD COLUMN mention_count INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE action_item ADD COLUMN claimed INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE action_item ADD COLUMN source TEXT NOT NULL DEFAULT 'mattermost'",
            [],
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // --- Watermarks ---

    pub fn get_watermark(&self, channel_id: &str) -> Option<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT last_post_at FROM channel_watermark WHERE channel_id = ?1",
            params![channel_id],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set_watermark(&self, channel_id: &str, last_post_at: i64) -> Result<()> {
        let now = jiff::Timestamp::now().as_millisecond();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channel_watermark (channel_id, last_post_at, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(channel_id) DO UPDATE SET last_post_at = ?2, updated_at = ?3",
            params![channel_id, last_post_at, now],
        )
        .context("failed to set watermark")?;
        Ok(())
    }

    pub fn clear_all(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM channel_watermark", [])
            .context("failed to clear all watermarks")?;
        conn.execute("DELETE FROM action_item", [])
            .context("failed to clear all action items")?;
        // Preserve seeded insights (historical baseline); drop only live-summarise records.
        conn.execute("DELETE FROM channel_insights WHERE seeded = 0", [])
            .context("failed to clear non-seeded channel insights")?;
        Ok(())
    }

    pub fn clear_channel(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM channel_watermark WHERE channel_id = ?1",
            params![channel_id],
        )
        .context("failed to clear watermark")?;
        conn.execute(
            "DELETE FROM action_item WHERE channel_id = ?1",
            params![channel_id],
        )
        .context("failed to clear channel action items")?;
        // Preserve seeded insights for this channel.
        conn.execute(
            "DELETE FROM channel_insights WHERE channel_id = ?1 AND seeded = 0",
            params![channel_id],
        )
        .context("failed to clear non-seeded channel insights")?;
        Ok(())
    }

    // --- Action items ---

    /// Insert or update action items for a channel.
    /// `source` should be "mattermost" or "email".
    pub fn upsert_action_items(
        &self,
        channel_id: &str,
        texts: &[String],
        created_at: i64,
        source: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        for text in texts {
            let id = action_item_id(channel_id, text);
            conn.execute(
                "INSERT INTO action_item (id, channel_id, text, created_at, source)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO NOTHING",
                params![id, channel_id, text, created_at, source],
            )
            .context("failed to upsert action item")?;
        }
        Ok(())
    }

    /// Pending items: not resolved, not ignored.
    pub fn get_pending_action_items(&self, channel_id: &str) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, text, created_at, resolved, ignored, claimed, source
             FROM action_item
             WHERE channel_id = ?1 AND resolved = 0 AND ignored = 0
             ORDER BY created_at",
        )?;
        let items = stmt
            .query_map(params![channel_id], |row| {
                Ok(ActionItem {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    text: row.get(2)?,
                    created_at: row.get(3)?,
                    resolved: row.get::<_, i64>(4)? != 0,
                    ignored: row.get::<_, i64>(5)? != 0,
                    claimed: row.get::<_, i64>(6)? != 0,
                    source: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| "mattermost".to_string()),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query action items")?;
        Ok(items)
    }

    /// All action items for a channel (for the API).
    pub fn get_all_action_items(&self, channel_id: &str) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, text, created_at, resolved, ignored, claimed, source
             FROM action_item WHERE channel_id = ?1 ORDER BY created_at",
        )?;
        let items = stmt
            .query_map(params![channel_id], |row| {
                Ok(ActionItem {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    text: row.get(2)?,
                    created_at: row.get(3)?,
                    resolved: row.get::<_, i64>(4)? != 0,
                    ignored: row.get::<_, i64>(5)? != 0,
                    claimed: row.get::<_, i64>(6)? != 0,
                    source: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| "mattermost".to_string()),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query action items")?;
        Ok(items)
    }

    /// All action items across all channels (for the REST API).
    pub fn get_all_action_items_global(&self) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, text, created_at, resolved, ignored, claimed, source
             FROM action_item ORDER BY created_at",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(ActionItem {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    text: row.get(2)?,
                    created_at: row.get(3)?,
                    resolved: row.get::<_, i64>(4)? != 0,
                    ignored: row.get::<_, i64>(5)? != 0,
                    claimed: row.get::<_, i64>(6)? != 0,
                    source: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| "mattermost".to_string()),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query all action items")?;
        Ok(items)
    }

    pub fn set_action_item_ignored(&self, id: &str, ignored: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE action_item SET ignored = ?1 WHERE id = ?2",
            params![ignored as i64, id],
        )
        .context("failed to update action item ignored flag")?;
        Ok(())
    }

    pub fn set_action_item_resolved(&self, id: &str, resolved: bool) -> Result<()> {
        let now = if resolved {
            Some(jiff::Timestamp::now().as_millisecond())
        } else {
            None
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE action_item SET resolved = ?1, resolved_at = ?2 WHERE id = ?3",
            params![resolved as i64, now, id],
        )
        .context("failed to update action item resolved flag")?;
        Ok(())
    }

    pub fn set_action_item_claimed(&self, id: &str, claimed: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE action_item SET claimed = ?1 WHERE id = ?2",
            params![claimed as i64, id],
        )
        .context("failed to update action item claimed flag")?;
        Ok(())
    }

    // --- User preferences ---

    pub fn get_pref(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM user_prefs WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set_pref(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO user_prefs (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .context("failed to set user pref")?;
        Ok(())
    }

    pub fn get_all_prefs(&self) -> Result<std::collections::HashMap<String, String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM user_prefs")?;
        let prefs = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
            .context("failed to query user prefs")?;
        Ok(prefs)
    }

    // --- Channel insights ---

    pub fn upsert_channel_insight(&self, insight: &ChannelInsight) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let action_items_json = serde_json::to_string(&insight.action_items).unwrap_or_default();
        let topics_json = serde_json::to_string(&insight.topics).unwrap_or_default();
        let now = jiff::Timestamp::now().as_millisecond();
        conn.execute(
            "INSERT INTO channel_insights
                (id, channel_id, channel_name, team_name, summary_text,
                 action_items_json, topics_json, importance_score, risk_score,
                 timestamp_ms, seeded, unread_count, mention_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                channel_name      = excluded.channel_name,
                team_name         = excluded.team_name,
                summary_text      = excluded.summary_text,
                action_items_json = excluded.action_items_json,
                topics_json       = excluded.topics_json,
                importance_score  = excluded.importance_score,
                risk_score        = excluded.risk_score,
                unread_count      = excluded.unread_count,
                mention_count     = excluded.mention_count",
            params![
                insight.id,
                insight.channel_id,
                insight.channel_name,
                insight.team_name,
                insight.summary_text,
                action_items_json,
                topics_json,
                insight.importance_score,
                insight.risk_score,
                insight.timestamp_ms,
                insight.seeded as i64,
                insight.unread_count,
                insight.mention_count,
                now
            ],
        )
        .context("failed to upsert channel insight")?;
        Ok(())
    }

    pub fn get_channel_insights_in_range(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ChannelInsight>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, channel_name, team_name, summary_text,
                    action_items_json, topics_json, importance_score, risk_score,
                    timestamp_ms, seeded, unread_count, mention_count
             FROM channel_insights
             WHERE timestamp_ms >= ?1 AND timestamp_ms <= ?2
             ORDER BY mention_count DESC, unread_count DESC, timestamp_ms DESC",
        )?;
        let rows = stmt
            .query_map(params![from_ms, to_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, f64>(7)?,
                    row.get::<_, f64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query channel insights")?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    channel_id,
                    channel_name,
                    team_name,
                    summary_text,
                    aij,
                    tj,
                    importance_score,
                    risk_score,
                    timestamp_ms,
                    seeded_i64,
                    unread_count,
                    mention_count,
                )| {
                    ChannelInsight {
                        id,
                        channel_id,
                        channel_name,
                        team_name,
                        summary_text,
                        action_items: serde_json::from_str(&aij).unwrap_or_default(),
                        topics: serde_json::from_str(&tj).unwrap_or_default(),
                        importance_score,
                        risk_score,
                        timestamp_ms,
                        seeded: seeded_i64 != 0,
                        unread_count,
                        mention_count,
                    }
                },
            )
            .collect())
    }
    // --- Favourites ---

    pub fn add_favourite(
        &self,
        channel_id: &str,
        channel_name: &str,
        team_name: &str,
    ) -> Result<()> {
        let now = jiff::Timestamp::now().as_millisecond();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO favourite_channel (channel_id, channel_name, team_name, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(channel_id) DO UPDATE SET channel_name = ?2, team_name = ?3",
            params![channel_id, channel_name, team_name, now],
        )
        .context("failed to add favourite")?;
        Ok(())
    }

    pub fn remove_favourite(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM favourite_channel WHERE channel_id = ?1",
            params![channel_id],
        )
        .context("failed to remove favourite")?;
        Ok(())
    }

    pub fn get_favourites(&self) -> Result<Vec<FavouriteChannel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, channel_name, team_name, added_at
             FROM favourite_channel ORDER BY team_name, channel_name",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(FavouriteChannel {
                    channel_id: row.get(0)?,
                    channel_name: row.get(1)?,
                    team_name: row.get(2)?,
                    added_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query favourites")?;
        Ok(items)
    }

    pub fn is_favourite(&self, channel_id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM favourite_channel WHERE channel_id = ?1",
            params![channel_id],
            |_| Ok(()),
        )
        .is_ok()
    }

    // --- Cached summaries ---

    /// Store a channel summary (serialised as JSON) in the cache.
    pub fn set_cached_summary(&self, channel_id: &str, json: &str) -> Result<()> {
        let now = jiff::Timestamp::now().as_millisecond();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cached_summary (channel_id, summary_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(channel_id) DO UPDATE SET summary_json = ?2, updated_at = ?3",
            params![channel_id, json, now],
        )
        .context("failed to set cached summary")?;
        Ok(())
    }

    /// Load all cached summaries from the store.
    pub fn get_cached_summaries(&self) -> Result<Vec<(String, String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, summary_json, updated_at FROM cached_summary ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query cached summaries")?;
        Ok(rows)
    }

    /// Remove a channel from the summary cache.
    pub fn remove_cached_summary(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM cached_summary WHERE channel_id = ?1",
            params![channel_id],
        )
        .context("failed to remove cached summary")?;
        Ok(())
    }

    /// Remove all cached summaries.
    pub fn clear_cached_summaries(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM cached_summary", [])
            .context("failed to clear cached summaries")?;
        Ok(())
    }

    // --- Email watermarks ---

    /// Get the last-seen IMAP UID for a mailbox (0 = never fetched).
    pub fn get_email_watermark(&self, mailbox: &str) -> u32 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT last_uid FROM email_watermark WHERE mailbox = ?1",
            params![mailbox],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .map(|v| v as u32)
        .unwrap_or(0)
    }

    pub fn set_email_watermark(&self, mailbox: &str, last_uid: u32) -> Result<()> {
        let now = jiff::Timestamp::now().as_millisecond();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO email_watermark (mailbox, last_uid, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(mailbox) DO UPDATE SET last_uid = ?2, updated_at = ?3",
            params![mailbox, last_uid as i64, now],
        )
        .context("failed to set email watermark")?;
        Ok(())
    }

    // --- Cached email summaries ---

    pub fn set_cached_email_summary(&self, mailbox: &str, json: &str) -> Result<()> {
        let now = jiff::Timestamp::now().as_millisecond();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cached_email_summary (mailbox, summary_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(mailbox) DO UPDATE SET summary_json = ?2, updated_at = ?3",
            params![mailbox, json, now],
        )
        .context("failed to set cached email summary")?;
        Ok(())
    }

    pub fn get_cached_email_summaries(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT mailbox, summary_json FROM cached_email_summary ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query cached email summaries")?;
        Ok(rows)
    }

    pub fn remove_cached_email_summary(&self, mailbox: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM cached_email_summary WHERE mailbox = ?1",
            params![mailbox],
        )
        .context("failed to remove cached email summary")?;
        Ok(())
    }
}

// --- Source config ---

/// A single row from the `source_config` table.
#[derive(Debug, Clone)]
pub struct SourceConfigRow {
    pub source_type: String,
    pub name: String,
    pub team: String,
    pub enabled: bool,
    pub instructions: Option<String>,
}

impl Store {
    /// Return all source_config rows for `source_type` (e.g. "mm_channel").
    pub fn get_source_configs(&self, source_type: &str) -> Result<Vec<SourceConfigRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source_type, name, team, enabled, instructions
             FROM source_config WHERE source_type = ?1",
        )?;
        let rows = stmt
            .query_map(params![source_type], |row| {
                Ok(SourceConfigRow {
                    source_type: row.get(0)?,
                    name: row.get(1)?,
                    team: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                    instructions: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to query source_config")?;
        Ok(rows)
    }

    /// Insert or update a source_config entry.
    pub fn upsert_source_config(
        &self,
        source_type: &str,
        name: &str,
        team: &str,
        enabled: bool,
        instructions: Option<&str>,
    ) -> Result<()> {
        let now = jiff::Timestamp::now().as_millisecond();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO source_config (source_type, name, team, enabled, instructions, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_type, name, team)
             DO UPDATE SET enabled = ?4, instructions = ?5, updated_at = ?6",
            params![source_type, name, team, enabled as i64, instructions, now],
        )
        .context("failed to upsert source_config")?;
        Ok(())
    }
}

fn action_item_id(channel_id: &str, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(channel_id.as_bytes());
    h.update(text.as_bytes());
    let result = h.finalize();
    result[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Store {
        Store::open(Path::new(":memory:")).expect("open in-memory store")
    }

    #[test]
    fn watermark_round_trip() {
        let s = test_store();
        assert_eq!(s.get_watermark("ch1"), None);
        s.set_watermark("ch1", 1000).unwrap();
        assert_eq!(s.get_watermark("ch1"), Some(1000));
        s.set_watermark("ch1", 2000).unwrap();
        assert_eq!(s.get_watermark("ch1"), Some(2000));
    }

    #[test]
    fn clear_channel_resets_watermark() {
        let s = test_store();
        s.set_watermark("ch1", 100).unwrap();
        s.set_watermark("ch2", 200).unwrap();
        s.clear_channel("ch1").unwrap();
        assert_eq!(s.get_watermark("ch1"), None);
        assert_eq!(s.get_watermark("ch2"), Some(200));
    }

    #[test]
    fn clear_all_resets_watermarks() {
        let s = test_store();
        s.set_watermark("ch1", 100).unwrap();
        s.set_watermark("ch2", 200).unwrap();
        s.clear_all().unwrap();
        assert_eq!(s.get_watermark("ch1"), None);
        assert_eq!(s.get_watermark("ch2"), None);
    }

    #[test]
    fn action_items_upsert_and_pending() {
        let s = test_store();
        let items = vec!["Fix bug".to_string(), "Deploy v2".to_string()];
        s.upsert_action_items("ch1", &items, 1000, "mattermost").unwrap();

        let pending = s.get_pending_action_items("ch1").unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().any(|i| i.text == "Fix bug"));
        assert!(pending.iter().any(|i| i.text == "Deploy v2"));
    }

    #[test]
    fn action_items_dedup() {
        let s = test_store();
        let items = vec!["Fix bug".to_string()];
        s.upsert_action_items("ch1", &items, 1000, "mattermost").unwrap();
        s.upsert_action_items("ch1", &items, 2000, "mattermost").unwrap();

        let all = s.get_all_action_items("ch1").unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn action_item_resolve_and_ignore() {
        let s = test_store();
        let items = vec!["Task A".to_string()];
        s.upsert_action_items("ch1", &items, 1000, "mattermost").unwrap();

        let pending = s.get_pending_action_items("ch1").unwrap();
        let id = &pending[0].id;

        s.set_action_item_resolved(id, true).unwrap();
        assert!(s.get_pending_action_items("ch1").unwrap().is_empty());

        s.set_action_item_resolved(id, false).unwrap();
        assert_eq!(s.get_pending_action_items("ch1").unwrap().len(), 1);

        s.set_action_item_ignored(id, true).unwrap();
        assert!(s.get_pending_action_items("ch1").unwrap().is_empty());
    }

    #[test]
    fn action_item_id_is_sha256() {
        let id = action_item_id("channel_abc", "some action text");
        assert_eq!(id.len(), 16, "ID should be 16 hex chars (8 bytes)");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // Stable across calls
        assert_eq!(id, action_item_id("channel_abc", "some action text"));
        // Different inputs produce different IDs
        assert_ne!(id, action_item_id("channel_abc", "other text"));
        assert_ne!(id, action_item_id("other_channel", "some action text"));
    }

    #[test]
    fn cached_summary_crud() {
        let s = test_store();
        assert!(s.get_cached_summaries().unwrap().is_empty());

        s.set_cached_summary("ch1", r#"{"summary":"hello"}"#)
            .unwrap();
        let cached = s.get_cached_summaries().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].0, "ch1");
        assert_eq!(cached[0].1, r#"{"summary":"hello"}"#);

        // Upsert overwrites
        s.set_cached_summary("ch1", r#"{"summary":"updated"}"#)
            .unwrap();
        let cached = s.get_cached_summaries().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].1, r#"{"summary":"updated"}"#);

        s.remove_cached_summary("ch1").unwrap();
        assert!(s.get_cached_summaries().unwrap().is_empty());
    }

    #[test]
    fn clear_cached_summaries() {
        let s = test_store();
        s.set_cached_summary("ch1", "{}").unwrap();
        s.set_cached_summary("ch2", "{}").unwrap();
        s.clear_cached_summaries().unwrap();
        assert!(s.get_cached_summaries().unwrap().is_empty());
    }

    #[test]
    fn favourites_round_trip() {
        let s = test_store();
        assert!(s.get_favourites().unwrap().is_empty());
        assert!(!s.is_favourite("ch1"));

        s.add_favourite("ch1", "general", "TeamA").unwrap();
        assert!(s.is_favourite("ch1"));
        assert_eq!(s.get_favourites().unwrap().len(), 1);

        s.remove_favourite("ch1").unwrap();
        assert!(!s.is_favourite("ch1"));
    }

    #[test]
    fn prefs_round_trip() {
        let s = test_store();
        assert_eq!(s.get_pref("sidebar_collapsed"), None);
        s.set_pref("sidebar_collapsed", "true").unwrap();
        assert_eq!(s.get_pref("sidebar_collapsed"), Some("true".to_string()));
        s.set_pref("sidebar_collapsed", "false").unwrap();
        assert_eq!(s.get_pref("sidebar_collapsed"), Some("false".to_string()));

        let all = s.get_all_prefs().unwrap();
        assert_eq!(
            all.get("sidebar_collapsed").map(|s| s.as_str()),
            Some("false"),
        );
    }

    #[test]
    fn action_item_claim_and_unclaim() {
        let s = test_store();
        s.upsert_action_items("ch1", &items(&["Task A"]), 1000, "mattermost")
            .unwrap();

        let pending = s.get_pending_action_items("ch1").unwrap();
        let id = pending[0].id.clone();
        assert!(!pending[0].claimed);

        // Claiming keeps the item in the pending list (someone is working on it)
        s.set_action_item_claimed(&id, true).unwrap();
        let pending = s.get_pending_action_items("ch1").unwrap();
        assert_eq!(pending.len(), 1, "claimed item must still appear as pending");
        assert!(pending[0].claimed);

        // Unclaiming clears the flag
        s.set_action_item_claimed(&id, false).unwrap();
        let pending = s.get_pending_action_items("ch1").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].claimed);
    }

    #[test]
    fn action_item_claimed_visible_in_global_list() {
        let s = test_store();
        s.upsert_action_items("ch1", &items(&["Task A"]), 1000, "mattermost")
            .unwrap();
        let id = s.get_pending_action_items("ch1").unwrap()[0].id.clone();

        s.set_action_item_claimed(&id, true).unwrap();

        let global = s.get_all_action_items_global().unwrap();
        assert_eq!(global.len(), 1);
        assert!(global[0].claimed);
    }

    #[test]
    fn email_watermark_defaults_to_zero() {
        let s = test_store();
        assert_eq!(s.get_email_watermark("INBOX"), 0);
    }

    #[test]
    fn email_watermark_round_trip() {
        let s = test_store();
        s.set_email_watermark("INBOX", 42).unwrap();
        assert_eq!(s.get_email_watermark("INBOX"), 42);
    }

    #[test]
    fn email_watermark_update() {
        let s = test_store();
        s.set_email_watermark("INBOX", 10).unwrap();
        s.set_email_watermark("INBOX", 99).unwrap();
        assert_eq!(s.get_email_watermark("INBOX"), 99);
    }

    #[test]
    fn email_watermark_per_mailbox() {
        let s = test_store();
        s.set_email_watermark("INBOX", 1).unwrap();
        s.set_email_watermark("INBOX.Work", 2).unwrap();
        assert_eq!(s.get_email_watermark("INBOX"), 1);
        assert_eq!(s.get_email_watermark("INBOX.Work"), 2);
    }

    #[test]
    fn cached_email_summary_crud() {
        let s = test_store();
        assert!(s.get_cached_email_summaries().unwrap().is_empty());

        s.set_cached_email_summary("INBOX", r#"{"summary":"hello"}"#)
            .unwrap();
        let cached = s.get_cached_email_summaries().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].0, "INBOX");
        assert_eq!(cached[0].1, r#"{"summary":"hello"}"#);

        s.set_cached_email_summary("INBOX", r#"{"summary":"updated"}"#)
            .unwrap();
        let cached = s.get_cached_email_summaries().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].1, r#"{"summary":"updated"}"#);

        s.remove_cached_email_summary("INBOX").unwrap();
        assert!(s.get_cached_email_summaries().unwrap().is_empty());
    }

    #[test]
    fn cached_email_summary_multiple_mailboxes() {
        let s = test_store();
        s.set_cached_email_summary("INBOX", "{}").unwrap();
        s.set_cached_email_summary("INBOX.Work", "{}").unwrap();
        assert_eq!(s.get_cached_email_summaries().unwrap().len(), 2);
        s.remove_cached_email_summary("INBOX").unwrap();
        let cached = s.get_cached_email_summaries().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].0, "INBOX.Work");
    }

    #[test]
    fn source_config_upsert_and_get() {
        let s = test_store();
        assert!(s.get_source_configs("mm_channel").unwrap().is_empty());

        s.upsert_source_config("mm_channel", "general", "TeamA", true, None)
            .unwrap();
        let rows = s.get_source_configs("mm_channel").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "general");
        assert_eq!(rows[0].team, "TeamA");
        assert!(rows[0].enabled);
        assert!(rows[0].instructions.is_none());
    }

    #[test]
    fn source_config_enabled_toggle() {
        let s = test_store();
        s.upsert_source_config("mm_channel", "general", "TeamA", true, None)
            .unwrap();
        s.upsert_source_config("mm_channel", "general", "TeamA", false, None)
            .unwrap();
        let rows = s.get_source_configs("mm_channel").unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].enabled);
    }

    #[test]
    fn source_config_instructions() {
        let s = test_store();
        s.upsert_source_config(
            "mm_channel",
            "support",
            "TeamA",
            true,
            Some("Focus on escalations."),
        )
        .unwrap();
        let rows = s.get_source_configs("mm_channel").unwrap();
        assert_eq!(rows[0].instructions.as_deref(), Some("Focus on escalations."));
    }

    #[test]
    fn source_config_filtered_by_source_type() {
        let s = test_store();
        s.upsert_source_config("mm_channel", "general", "TeamA", true, None)
            .unwrap();
        s.upsert_source_config("email_mailbox", "INBOX", "", true, None)
            .unwrap();
        assert_eq!(s.get_source_configs("mm_channel").unwrap().len(), 1);
        assert_eq!(s.get_source_configs("email_mailbox").unwrap().len(), 1);
        assert!(s.get_source_configs("other").unwrap().is_empty());
    }
}
