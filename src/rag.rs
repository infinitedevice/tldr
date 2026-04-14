// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Retrieval-Augmented Generation (RAG) support via LanceDB + fastembed.
//!
//! [`VectorStore`] stores channel analysis results as embeddings in a local LanceDB
//! database and retrieves the most similar historical analyses to inject into the LLM
//! prompt as context.  This lets the model identify recurring patterns, compare against
//! historical baselines, and track topic evolution over time.
//!
//! Storage lives at `~/.config/tldr/data/vectors/` by default (configurable via
//! `paths.vectors_dir` in the config file).
//!
//! The embedding model (NomicEmbedTextV15, 768 dims) is downloaded on first use via
//! fastembed's built-in download mechanism.  Expect ~274 MB on first run.
//!
//! # Threading
//! fastembed's `TextEmbedding::embed()` is synchronous ONNX inference; it is always
//! wrapped in [`tokio::task::spawn_blocking`] to avoid blocking the async runtime.

use anyhow::{Context, Result};
use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
    types::Float32Type,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use futures::TryStreamExt;
use lancedb::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Embedding dimension for NomicEmbedTextV15.
const EMBED_DIM: i32 = 768;

/// Name of the channel summaries table in LanceDB.
const TABLE_CHANNEL_SUMMARIES: &str = "channel_summaries";

/// A stored analysis record for a channel analysis run.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub timestamp_ms: i64,
    pub summary: String,
    /// JSON-serialized `Vec<String>` of topics.
    pub topics: String,
    pub raw_insight: String,
    /// Placeholder until `insights.rs` computes real scores.
    pub risk_score: f32,
    /// Placeholder until `insights.rs` computes real scores.
    pub importance_score: f32,
}

/// Embedded, persistent vector store for channel analysis history.
#[derive(Clone)]
pub struct VectorStore {
    conn: Connection,
    embedder: Arc<Mutex<TextEmbedding>>,
    schema: SchemaRef,
}

impl VectorStore {
    /// Open (or create) the LanceDB database at `path`.
    ///
    /// On first call the fastembed model files are downloaded (~274 MB).
    /// Subsequent calls use the cached model.
    pub async fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path).with_context(|| {
            format!(
                "failed to create vector store directory: {}",
                path.display()
            )
        })?;

        let path_str = path
            .to_str()
            .context("vector store path contains non-UTF-8 characters")?;

        info!(path = %path_str, "opening LanceDB vector store");

        let conn = lancedb::connect(path_str)
            .execute()
            .await
            .context("failed to connect to LanceDB")?;

        let schema = Arc::new(channel_summaries_schema());

        // Ensure the table exists; create it empty if not.
        let existing = conn
            .table_names()
            .execute()
            .await
            .context("failed to list LanceDB tables")?;

        if !existing.contains(&TABLE_CHANNEL_SUMMARIES.to_string()) {
            info!("creating new LanceDB table: {TABLE_CHANNEL_SUMMARIES}");
            conn.create_empty_table(TABLE_CHANNEL_SUMMARIES, Arc::clone(&schema))
                .execute()
                .await
                .context("failed to create channel_summaries table")?;
        }

        // Initialise fastembed in a blocking thread — model download may take a while.
        info!(
            "initialising fastembed embedding model (NomicEmbedTextV15) — on first run this downloads ~274 MB and may take a few minutes"
        );
        let model_cache_dir = path.parent().unwrap_or(path).join("fastembed_models");
        std::fs::create_dir_all(&model_cache_dir).with_context(|| {
            format!(
                "failed to create model cache dir: {}",
                model_cache_dir.display()
            )
        })?;
        let embedder = tokio::task::spawn_blocking(move || -> Result<TextEmbedding> {
            let mut opts = InitOptions::default();
            opts.model_name = EmbeddingModel::NomicEmbedTextV15;
            opts.show_download_progress = true;
            opts.cache_dir = model_cache_dir;
            TextEmbedding::try_new(opts).context("failed to initialise fastembed NomicEmbedTextV15")
        })
        .await
        .context("embedding model init task panicked")??;

        info!("vector store ready");

        Ok(Self {
            conn,
            embedder: Arc::new(Mutex::new(embedder)),
            schema,
        })
    }

    /// Embed `text` and return the up-to-`limit` most similar historical records
    /// for `channel_id`.  Returns an empty Vec if the table has no data yet.
    pub async fn query_similar(
        &self,
        channel_id: &str,
        text: &str,
        limit: usize,
    ) -> Result<Vec<ChannelRecord>> {
        let text = text.to_string();
        let embedder = Arc::clone(&self.embedder);

        let embedding: Vec<f32> = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let guard = embedder.lock().expect("embedder mutex poisoned");
            let mut vecs = guard
                .embed(vec![text], None)
                .context("embedding query text failed")?;
            Ok(vecs.swap_remove(0))
        })
        .await
        .context("embedding task panicked")??;

        let channel_id = channel_id.to_string();
        let table = self
            .conn
            .open_table(TABLE_CHANNEL_SUMMARIES)
            .execute()
            .await
            .context("failed to open channel_summaries table")?;

        let row_count = table
            .count_rows(None)
            .await
            .context("failed to count rows")?;

        if row_count == 0 {
            debug!("vector store is empty, skipping query");
            return Ok(Vec::new());
        }

        // Filter to records belonging to this channel before ANN search.
        let filter = format!("channel_id = '{}'", channel_id.replace('\'', "''"));

        let channel_count = table.count_rows(Some(filter.clone())).await.unwrap_or(0);

        if channel_count == 0 {
            debug!(channel_id = %channel_id, "no historical records for channel");
            return Ok(Vec::new());
        }

        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(filter)
            .nearest_to(embedding.as_slice())
            .context("failed to build nearest-to query")?
            .limit(limit)
            .execute()
            .await
            .context("vector query failed")?
            .try_collect()
            .await
            .context("failed to collect query results")?;

        let mut records = Vec::new();
        for batch in &batches {
            records.extend(extract_records(batch)?);
        }
        Ok(records)
    }

    /// Embed `record.summary` and persist `record` to the vector store.
    pub async fn upsert(&self, record: ChannelRecord) -> Result<()> {
        let summary_text = record.summary.clone();
        let embedder = Arc::clone(&self.embedder);

        let embedding: Vec<f32> = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let guard = embedder.lock().expect("embedder mutex poisoned");
            let mut vecs = guard
                .embed(vec![summary_text], None)
                .context("embedding summary failed")?;
            Ok(vecs.swap_remove(0))
        })
        .await
        .context("embedding task panicked")??;

        let schema = Arc::clone(&self.schema);
        let batch = record_to_batch(&record, &embedding, &schema)?;

        let table = self
            .conn
            .open_table(TABLE_CHANNEL_SUMMARIES)
            .execute()
            .await
            .context("failed to open channel_summaries table for upsert")?;

        let iter = RecordBatchIterator::new(vec![Ok(batch)], Arc::clone(&schema));

        // Upsert: update row if same id exists, insert otherwise.
        let mut merge = table.merge_insert(&["id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge
            .execute(Box::new(iter))
            .await
            .context("failed to upsert record into vector store")?;

        debug!(id = %record.id, channel = %record.channel_name, "upserted record");
        Ok(())
    }

    /// Format retrieved records as a markdown-style context string for the LLM prompt.
    pub fn format_context(records: &[ChannelRecord]) -> String {
        if records.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for (i, r) in records.iter().enumerate() {
            let ts = jiff::Timestamp::from_millisecond(r.timestamp_ms)
                .ok()
                .map(|t| t.strftime("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "unknown date".to_string());
            // Truncate long summaries so the context block stays under ~1000 tokens
            let excerpt: String = r.summary.chars().take(600).collect();
            let ellipsis = if r.summary.len() > 600 { "…" } else { "" };
            out.push_str(&format!("{}. [{}] {}{}\n", i + 1, ts, excerpt, ellipsis));
        }
        out
    }
}

/// Compute a stable record ID: first 16 hex chars of a simple hash of channel_id + timestamp.
pub fn record_id(channel_id: &str, timestamp_ms: i64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(channel_id.as_bytes());
    h.update(timestamp_ms.to_le_bytes());
    let result = h.finalize();
    result[..8].iter().map(|b| format!("{b:02x}")).collect()
}

// ─── Arrow schema ──────────────────────────────────────────────────────────────

fn channel_summaries_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("channel_id", DataType::Utf8, false),
        Field::new("channel_name", DataType::Utf8, false),
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("summary", DataType::Utf8, false),
        Field::new("topics", DataType::Utf8, false),
        Field::new("raw_insight", DataType::Utf8, false),
        Field::new("risk_score", DataType::Float32, true),
        Field::new("importance_score", DataType::Float32, true),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBED_DIM,
            ),
            false,
        ),
    ])
}

// ─── Helpers ───────────────────────────────────────────────────────────────────

fn record_to_batch(
    record: &ChannelRecord,
    embedding: &[f32],
    schema: &SchemaRef,
) -> Result<RecordBatch> {
    let embedding_list = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        std::iter::once(Some(embedding.iter().map(|&v| Some(v)).collect::<Vec<_>>())),
        EMBED_DIM,
    );

    RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(StringArray::from(vec![record.id.as_str()])),
            Arc::new(StringArray::from(vec![record.channel_id.as_str()])),
            Arc::new(StringArray::from(vec![record.channel_name.as_str()])),
            Arc::new(Int64Array::from(vec![record.timestamp_ms])),
            Arc::new(StringArray::from(vec![record.summary.as_str()])),
            Arc::new(StringArray::from(vec![record.topics.as_str()])),
            Arc::new(StringArray::from(vec![record.raw_insight.as_str()])),
            Arc::new(Float32Array::from(vec![record.risk_score])),
            Arc::new(Float32Array::from(vec![record.importance_score])),
            Arc::new(embedding_list),
        ],
    )
    .context("failed to build Arrow RecordBatch")
}

fn extract_records(batch: &RecordBatch) -> Result<Vec<ChannelRecord>> {
    macro_rules! col_str {
        ($batch:expr, $name:expr) => {
            $batch
                .column_by_name($name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| anyhow::anyhow!("missing column: {}", $name))?
        };
    }
    macro_rules! col_i64 {
        ($batch:expr, $name:expr) => {
            $batch
                .column_by_name($name)
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
                .ok_or_else(|| anyhow::anyhow!("missing column: {}", $name))?
        };
    }
    macro_rules! col_f32 {
        ($batch:expr, $name:expr) => {
            $batch
                .column_by_name($name)
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .ok_or_else(|| anyhow::anyhow!("missing column: {}", $name))?
        };
    }

    let ids = col_str!(batch, "id");
    let channel_ids = col_str!(batch, "channel_id");
    let channel_names = col_str!(batch, "channel_name");
    let timestamps = col_i64!(batch, "timestamp_ms");
    let summaries = col_str!(batch, "summary");
    let topics_col = col_str!(batch, "topics");
    let raw_insights = col_str!(batch, "raw_insight");
    let risk_scores = col_f32!(batch, "risk_score");
    let importance_scores = col_f32!(batch, "importance_score");

    let mut out = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        out.push(ChannelRecord {
            id: ids.value(i).to_string(),
            channel_id: channel_ids.value(i).to_string(),
            channel_name: channel_names.value(i).to_string(),
            timestamp_ms: timestamps.value(i),
            summary: summaries.value(i).to_string(),
            topics: topics_col.value(i).to_string(),
            raw_insight: raw_insights.value(i).to_string(),
            risk_score: risk_scores.value(i),
            importance_score: importance_scores.value(i),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_id_is_sha256_and_stable() {
        let id = record_id("ch123", 1_700_000_000_000);
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, record_id("ch123", 1_700_000_000_000));
        assert_ne!(id, record_id("ch123", 1_700_000_000_001));
        assert_ne!(id, record_id("ch456", 1_700_000_000_000));
    }

    #[test]
    fn channel_summaries_schema_has_embedding() {
        let schema = channel_summaries_schema();
        assert!(schema.field_with_name("embedding").is_ok());
        assert!(schema.field_with_name("channel_id").is_ok());
        assert!(schema.field_with_name("summary").is_ok());
    }
}
