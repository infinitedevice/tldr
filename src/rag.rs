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
//!
//! # Embedding pool
//! [`EmbedderPool`] holds one or more `TextEmbedding` instances, gated by a tokio
//! [`Semaphore`], so that multiple concurrent callers can embed in parallel when more
//! than one instance is provisioned.
//!
//! # Caching
//! Already-computed embeddings are kept in a bounded in-memory cache
//! (capped at [`EMBED_CACHE_MAX`] entries) so repeated summarise runs for
//! identical text incur no ONNX inference overhead.

use anyhow::{Context, Result};
use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
    types::Float32Type,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use futures::TryStreamExt;
use lancedb::Connection;
use lancedb::Table;
use lancedb::index::{Index, vector::IvfPqIndexBuilder};
use lancedb::query::{ExecutableQuery, QueryBase};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Embedding dimension for NomicEmbedTextV15.
const EMBED_DIM: i32 = 768;

/// Name of the channel summaries table in LanceDB.
const TABLE_CHANNEL_SUMMARIES: &str = "channel_summaries";

/// Minimum row count required before an IVF-PQ ANN index is created on open.
const MIN_INDEX_ROWS: usize = 256;

/// Maximum number of embedding vectors held in the in-memory cache.
const EMBED_CACHE_MAX: usize = 1024;

/// Number of messages packed into each stored thread chunk.
const CHUNK_SIZE: usize = 20;

/// A stored analysis record for a channel analysis run.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub timestamp_ms: i64,
    /// Text stored in the database and used as the embedding input.
    ///
    /// Records produced by [`VectorStore::store_thread_chunks`] store a chunk
    /// of raw message content here; records produced by [`VectorStore::upsert`]
    /// store the LLM summary text.
    pub summary: String,
    /// JSON-serialized `Vec<String>` of topics.
    pub topics: String,
    /// For chunk records this holds the LLM summary so [`VectorStore::format_context`]
    /// can surface a human-readable insight rather than raw messages.
    pub raw_insight: String,
    /// Placeholder until `insights.rs` computes real scores.
    pub risk_score: f32,
    /// Placeholder until `insights.rs` computes real scores.
    pub importance_score: f32,
}

// ─── Embedder pool ─────────────────────────────────────────────────────────────

/// A pool of `TextEmbedding` instances for concurrent embedding.
///
/// Each call to [`EmbedderPool::embed_batch`] acquires one instance via a tokio
/// [`Semaphore`], runs ONNX inference inside [`tokio::task::spawn_blocking`],
/// then returns the instance to the pool before resolving.  Increasing the
/// initial pool size (by passing more instances to [`EmbedderPool::new`]) allows
/// multiple concurrent embed calls to run in parallel.
struct EmbedderPool {
    slots: Arc<Mutex<Vec<TextEmbedding>>>,
    available: Arc<tokio::sync::Semaphore>,
}

impl EmbedderPool {
    fn new(instances: Vec<TextEmbedding>) -> Self {
        let n = instances.len();
        debug_assert!(n > 0, "EmbedderPool must be initialised with at least one instance");
        Self {
            slots: Arc::new(Mutex::new(instances)),
            available: Arc::new(tokio::sync::Semaphore::new(n)),
        }
    }

    /// Embed `texts` in a single ONNX inference call.
    ///
    /// Checks out one `TextEmbedding` instance from the pool (waiting if none
    /// are currently available), runs inference in a blocking thread, returns
    /// the instance to the pool, and resolves with the embedding vectors.
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let _permit = self
            .available
            .acquire()
            .await
            .context("embedder pool semaphore closed")?;

        let embedder = self
            .slots
            .lock()
            .expect("embedder pool mutex poisoned")
            .pop()
            .context("embedder pool unexpectedly empty")?;

        let (embedder, vecs) =
            tokio::task::spawn_blocking(move || -> Result<(TextEmbedding, Vec<Vec<f32>>)> {
                let vecs = embedder
                    .embed(texts, None)
                    .context("ONNX embedding failed")?;
                Ok((embedder, vecs))
            })
            .await
            .context("embed task panicked")??;

        self.slots
            .lock()
            .expect("embedder pool mutex poisoned")
            .push(embedder);

        Ok(vecs)
    }
}

/// Embedded, persistent vector store for channel analysis history.
#[derive(Clone)]
pub struct VectorStore {
    conn: Connection,
    table: Table,
    embedder: Arc<EmbedderPool>,
    schema: SchemaRef,
    /// Bounded in-memory cache: text → embedding vector.
    cache: Arc<Mutex<HashMap<String, Vec<f32>>>>,
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

        let table_is_new = !existing.contains(&TABLE_CHANNEL_SUMMARIES.to_string());

        if table_is_new {
            info!("creating new LanceDB table: {TABLE_CHANNEL_SUMMARIES}");
            conn.create_empty_table(TABLE_CHANNEL_SUMMARIES, Arc::clone(&schema))
                .execute()
                .await
                .context("failed to create channel_summaries table")?;
        }

        let table = conn
            .open_table(TABLE_CHANNEL_SUMMARIES)
            .execute()
            .await
            .context("failed to open channel_summaries table")?;

        // Build an IVF-PQ ANN index when the table already has enough data.
        // On a fresh empty table this is intentionally skipped; the index will
        // be created on the next daemon restart once enough rows have accumulated.
        // `replace(false)` ensures we do not needlessly rebuild an existing index.
        if !table_is_new {
            match table.count_rows(None).await {
                Ok(row_count) if row_count >= MIN_INDEX_ROWS => {
                    debug!(rows = row_count, "attempting to create IVF-PQ ANN index");
                    match table
                        .create_index(
                            &["embedding"],
                            Index::IvfPq(IvfPqIndexBuilder::default()),
                        )
                        .replace(false)
                        .execute()
                        .await
                    {
                        Ok(()) => info!("IVF-PQ ANN index created on embedding column"),
                        // `replace(false)` causes an error when the index already exists;
                        // that is the expected steady-state after the first successful run.
                        Err(e) => debug!("ANN index already exists, skipping creation: {e}"),
                    }
                }
                Ok(row_count) => {
                    debug!(
                        rows = row_count,
                        needed = MIN_INDEX_ROWS,
                        "skipping ANN index: not enough rows yet"
                    );
                }
                Err(e) => {
                    debug!("could not count rows for ANN index decision: {e}");
                }
            }
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
            table,
            embedder: Arc::new(EmbedderPool::new(vec![embedder])),
            schema,
            cache: Arc::new(Mutex::new(HashMap::new())),
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
        validate_channel_id(channel_id)?;

        let embedding = self
            .embed_texts(&[text.to_string()])
            .await?
            .swap_remove(0);

        // Filter to records belonging to this channel before the ANN search.
        // channel_id has been validated above so no quoting is required, but
        // we keep the single-quote delimiters required by LanceDB's SQL dialect.
        let filter = format!("channel_id = '{channel_id}'");

        let channel_count = self
            .table
            .count_rows(Some(filter.clone()))
            .await
            .unwrap_or(0);

        if channel_count == 0 {
            debug!(channel_id = %channel_id, "no historical records for channel");
            return Ok(Vec::new());
        }

        let batches: Vec<RecordBatch> = self
            .table
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
        self.batch_upsert(vec![record]).await
    }

    /// Embed all record summaries in a single ONNX batch call and persist every
    /// record to the vector store in one `merge_insert` operation.
    ///
    /// Embeddings for texts already held in the in-memory cache are reused
    /// without re-running inference.  Only the truly novel texts are passed to
    /// the embedder pool.
    pub async fn batch_upsert(&self, records: Vec<ChannelRecord>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let texts: Vec<String> = records.iter().map(|r| r.summary.clone()).collect();
        let embeddings = self.embed_texts(&texts).await?;

        let schema = Arc::clone(&self.schema);
        let batch = records_to_batch(&records, &embeddings, &schema)?;

        let iter = RecordBatchIterator::new(vec![Ok(batch)], Arc::clone(&schema));

        let mut merge = self.table.merge_insert(&["id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge
            .execute(Box::new(iter))
            .await
            .context("failed to batch upsert records into vector store")?;

        debug!(count = records.len(), "batch upserted records");
        Ok(())
    }

    /// Split `messages` into chunks of [`CHUNK_SIZE`] and store each chunk as a
    /// separate embedding keyed by its raw message content.
    ///
    /// Embedding the raw message text (rather than the LLM summary) lets the
    /// vector search find historical discussions that are semantically similar to
    /// the current conversation, not just similar-sounding summaries.
    ///
    /// `llm_summary` is stored in `raw_insight` so that
    /// [`VectorStore::format_context`] can surface a concise human-readable
    /// insight rather than raw message fragments when building the LLM prompt.
    pub async fn store_thread_chunks(
        &self,
        channel_id: &str,
        channel_name: &str,
        timestamp_ms: i64,
        messages: &[String],
        llm_summary: &str,
    ) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }

        validate_channel_id(channel_id)?;

        let records: Vec<ChannelRecord> = messages
            .chunks(CHUNK_SIZE)
            .enumerate()
            .map(|(i, chunk)| ChannelRecord {
                id: record_id(&format!("{channel_id}-chunk-{i}"), timestamp_ms),
                channel_id: channel_id.to_string(),
                channel_name: channel_name.to_string(),
                timestamp_ms,
                summary: chunk.join("\n"),
                topics: String::new(),
                raw_insight: llm_summary.to_string(),
                risk_score: 0.0,
                importance_score: 0.0,
            })
            .collect();

        self.batch_upsert(records).await
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
            // Prefer the LLM summary stored in raw_insight (set by store_thread_chunks);
            // fall back to summary for legacy records where raw_insight held raw JSON.
            let display = if r.raw_insight.is_empty() {
                &r.summary
            } else {
                &r.raw_insight
            };
            // Truncate long summaries so the context block stays under ~1000 tokens
            let excerpt: String = display.chars().take(600).collect();
            let ellipsis = if display.len() > 600 { "…" } else { "" };
            out.push_str(&format!("{}. [{}] {}{}\n", i + 1, ts, excerpt, ellipsis));
        }
        out
    }

    // ─── Internal helpers ────────────────────────────────────────────────────

    /// Embed `texts`, returning one vector per input string.
    ///
    /// Texts already present in the in-memory cache are returned immediately.
    /// The remaining texts are embedded together in a single batch call to the
    /// pool and then stored in the cache before returning.  The cache is bounded
    /// by [`EMBED_CACHE_MAX`]; when an insert would exceed the cap, the oldest
    /// entries are evicted to make room.
    async fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut result: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut uncached: Vec<(usize, String)> = Vec::new();

        {
            let guard = self.cache.lock().expect("embed cache mutex poisoned");
            for (i, text) in texts.iter().enumerate() {
                if let Some(v) = guard.get(text.as_str()) {
                    result[i] = Some(v.clone());
                } else {
                    uncached.push((i, text.clone()));
                }
            }
        }

        if !uncached.is_empty() {
            let batch_texts: Vec<String> = uncached.iter().map(|(_, t)| t.clone()).collect();
            let embeddings = self.embedder.embed_batch(batch_texts).await?;

            let mut guard = self.cache.lock().expect("embed cache mutex poisoned");
            // Evict entries if the new batch would push the cache over capacity.
            // `retain` makes a single O(n) pass, avoiding repeated rehashing.
            let new_total = guard.len() + embeddings.len();
            if new_total > EMBED_CACHE_MAX {
                let excess = new_total - EMBED_CACHE_MAX;
                let mut removed = 0_usize;
                guard.retain(|_, _| {
                    if removed < excess {
                        removed += 1;
                        false
                    } else {
                        true
                    }
                });
            }
            for ((i, text), embedding) in uncached.into_iter().zip(embeddings.into_iter()) {
                guard.insert(text, embedding.clone());
                result[i] = Some(embedding);
            }
        }

        result
            .into_iter()
            .enumerate()
            .map(|(i, opt)| {
                opt.ok_or_else(|| anyhow::anyhow!("missing embedding at position {i}"))
            })
            .collect()
    }
}

// ─── Channel ID validation ──────────────────────────────────────────────────

/// Validate that `channel_id` is safe for use as a literal value in a LanceDB
/// SQL filter string.
///
/// Mattermost channel IDs are 26-character lowercase alphanumeric strings.
/// Hyphens and underscores are explicitly allowed here because synthetic chunk
/// IDs (produced by [`VectorStore::store_thread_chunks`]) append `-chunk-N` to
/// the original Mattermost ID.  Anything else is rejected to prevent SQL injection
/// even after the single-quote delimiters used in the filter expression.
fn validate_channel_id(channel_id: &str) -> Result<()> {
    if channel_id.is_empty() {
        anyhow::bail!("channel_id must not be empty");
    }
    if channel_id.len() > 64 {
        anyhow::bail!("channel_id is too long ({} chars)", channel_id.len());
    }
    if !channel_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("channel_id contains invalid characters: {channel_id}");
    }
    Ok(())
}

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

/// Build a multi-row [`RecordBatch`] from `records` and their pre-computed `embeddings`.
fn records_to_batch(
    records: &[ChannelRecord],
    embeddings: &[Vec<f32>],
    schema: &SchemaRef,
) -> Result<RecordBatch> {
    let embedding_list = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        embeddings
            .iter()
            .map(|emb| Some(emb.iter().map(|&v| Some(v)).collect::<Vec<_>>())),
        EMBED_DIM,
    );

    RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(StringArray::from(
                records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|r| r.channel_id.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|r| r.channel_name.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                records.iter().map(|r| r.timestamp_ms).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|r| r.summary.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|r| r.topics.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|r| r.raw_insight.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float32Array::from(
                records.iter().map(|r| r.risk_score).collect::<Vec<_>>(),
            )),
            Arc::new(Float32Array::from(
                records
                    .iter()
                    .map(|r| r.importance_score)
                    .collect::<Vec<_>>(),
            )),
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

    #[test]
    fn validate_channel_id_accepts_valid_ids() {
        assert!(validate_channel_id("abc123").is_ok());
        assert!(validate_channel_id("ch-id_42").is_ok());
        assert!(validate_channel_id("ABCD1234efgh5678ijkl9012mn").is_ok());
    }

    #[test]
    fn validate_channel_id_rejects_invalid_ids() {
        assert!(validate_channel_id("").is_err());
        assert!(validate_channel_id("chan'id").is_err());
        assert!(validate_channel_id("chan id").is_err());
        assert!(validate_channel_id("chan;DROP").is_err());
        assert!(validate_channel_id(&"x".repeat(65)).is_err());
    }

    #[test]
    fn records_to_batch_builds_multi_row_batch() {
        let schema = Arc::new(channel_summaries_schema());
        let records = vec![
            ChannelRecord {
                id: "id1".to_string(),
                channel_id: "ch1".to_string(),
                channel_name: "Channel 1".to_string(),
                timestamp_ms: 1_000,
                summary: "summary one".to_string(),
                topics: "[]".to_string(),
                raw_insight: "".to_string(),
                risk_score: 0.0,
                importance_score: 0.0,
            },
            ChannelRecord {
                id: "id2".to_string(),
                channel_id: "ch1".to_string(),
                channel_name: "Channel 1".to_string(),
                timestamp_ms: 2_000,
                summary: "summary two".to_string(),
                topics: "[]".to_string(),
                raw_insight: "insight".to_string(),
                risk_score: 0.1,
                importance_score: 0.2,
            },
        ];
        let embeddings = vec![vec![0.0_f32; 768], vec![1.0_f32; 768]];
        let batch = records_to_batch(&records, &embeddings, &schema).unwrap();
        assert_eq!(batch.num_rows(), 2);
    }
}
