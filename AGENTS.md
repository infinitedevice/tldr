# AGENTS.md — tldr Architecture Guide

This file is written for AI coding agents (GitHub Copilot, Claude, etc.) picking up the codebase.
It captures design intent, module structure, data flow, and key decisions so you can make changes
without re-deriving them from the code.

---

## What this project does

**tldr** is a Mattermost chat summarisation daemon. It:

1. Authenticates to a Mattermost instance via a Personal Access Token
2. Enumerates teams → channels → unread counts for the bot user
3. For channels with unread messages, fetches recent history (≥ 1 working day)
4. Splits posts into *context* (already seen, for background) and *unread* (what to summarise)
5. Optionally fetches parent threads for orphan replies outside the history window
6. Calls an OpenAI-compatible LLM API to produce a structured JSON summary:
   `{ "summary": "...", "action_items": ["..."] }`
7. Stores channel watermarks and action items in SQLite so subsequent runs are incremental
8. **Background loop** re-summarises channels automatically every N seconds (configurable);
   cached results are persisted to SQLite and broadcast via SSE
9. Exposes results via an axum HTTP server (JSON API + SSE + static file serving)
10. A CLI (`tldr-cli`) reads cached summaries instantly; a Svelte frontend subscribes via SSE

---

## Module map

```text
src/
  lib.rs              — re-exports all modules
  config.rs           — Config struct, TOML load/save via serde, tilde expansion, validate()
  mattermost.rs       — MattermostClient (REST v4, user cache, 30s timeout)
  mattermost_types.rs — User, Team, Channel, ChannelUnread, Post, PostList
  llm.rs              — LlmClient, LlmSummary{topics, action_items}, JSON prompt
  summarise.rs        — summarise_all_unread(), summarise_channel(), background_summarise_loop()
  output.rs           — ChannelSummary (shared type), print_summaries() with termimad
  store.rs            — Store (SQLite), channel_watermark + action_item + cached_summary tables
  server.rs           — axum Router, AppState, all HTTP handlers, get_user_channels()
  daemon.rs           — run_daemon(): opens Store, wires AppState, binds listener
  rag.rs              — VectorStore (LanceDB + fastembed), channel embedding + retrieval
  seeding.rs          — First-run history seeding: backfills insights + RAG from older posts

src/bin/
  tldr-daemon.rs      — binary entry point; parses args, sets up tracing, calls run_daemon()
  tldr-cli.rs         — HTTP client; subcommands: summarise, status, clear-state,
                        config, favourites, completions, install-service
```

---

## Key data flow

### Background summarise loop (primary)

```text
daemon startup
  → load cached_summary rows from SQLite into AppState.summary_cache
  → spawn background_summarise_loop(poll_interval_secs)
      loop:
        sleep(interval)
        summarise_all_unread(mm, llm, store, rag)
        for each ChannelSummary:
          store.set_cached_summary(channel_id, json)
          broadcast via summary_tx (SSE)
        replace in-memory summary_cache
```

### Frontend (reads cached + SSE)

```text
onMount
  → GET /api/v1/summaries → display cached summaries
  → open EventSource(/api/v1/summaries/subscribe)
      on message: merge ChannelSummary by channel_id
```

### CLI (reads cached)

```text
tldr summarise
  → GET /api/v1/summaries → print_summaries()
```

### Channel summarisation (unchanged)

```text
summarise_channel(mm, llm, store, channel)
  1. get_channel_unread → msg_count, last_view_at
  2. paginated fetch (newest-first, pages 0…N until oldest < since_ms)
  3. filter to since_ms, sort ascending
  4. partition by last_view_at → context_posts | unread_posts
  5. fetch orphan parent threads
  6. store.get_pending_action_items(channel_id) → prior items
  7. llm.summarise(context_msgs, unread_msgs, prior_items)
     → LlmSummary { summary, action_items }
  8. store.upsert_action_items(channel_id, items)
  9. store.set_watermark(channel_id, latest_create_at)
 10. build ChannelSummary { …, action_items }
```

---

## Design decisions

### Pagination strategy (important — avoid regressing this)

The Mattermost `/channels/{id}/posts?since=X&per_page=200` endpoint returns posts
**oldest-first**. If there are more than 200 posts since the watermark, the 200-post cap
silently drops the newest (unread) messages — exactly backwards. Instead, we:

- Fetch **page 0** (most recent 200), **page 1**, … until the oldest post in a batch
  predates `since_ms` or we hit 10 pages (2000 posts)
- Collect all, then filter `create_at >= since_ms` and sort ascending

### LLM JSON contract

System prompt asks for `{"summary":"...","action_items":["..."]}` with no preamble sentence.
`serde_json::from_str` with fallback: if parsing fails, treat the full response as `summary`
with empty `action_items`. This works across Qwen, OpenAI, and any compatible API.

### Action item IDs

IDs are a 16-char hex prefix of `sha256(channel_id || text)`. This makes them stable across
runs and naturally deduplicates identical items from re-summarising the same period.

### Watermarks and history window

- History window = start of previous working day (Mon→Fri, otherwise yesterday)
- Stored watermark wins if it's more recent than the working-day window
- Both ensure the LLM always sees at least some context, never just a single message

### Config storage

`~/.config/tldr/config.toml` — the single source of configuration.  `expand_tilde()` in
`config.rs` uses `$HOME`, which is correctly set inside Flatpak's XDG bind-mounts.
`Config::save()` uses `toml::to_string_pretty()` for round-trip fidelity.

### Frontend ↔ daemon communication

Vite dev proxy: `/api` and `/slash` → `http://127.0.0.1:8765`
Production: `TLDR_FRONTEND_DIR` env var points the ServeDir fallback at built assets.

### Push-based architecture

- Background loop (`background_summarise_loop`) runs every `poll_interval_secs` (default 600)
- Results cached in SQLite `cached_summary` table and in-memory `AppState.summary_cache`
- SSE endpoint `/api/v1/summaries/subscribe` broadcasts each new summary via `tokio::sync::broadcast`
- Frontend loads cached state on mount, then subscribes to SSE for live updates
- CLI reads cached summaries instantly — no streaming, no waiting
- On-demand `/api/v1/summarise/stream` still available; also updates cache + broadcasts SSE
- `poll_interval_secs = 0` disables the background loop

### Tracing levels

- Release daemon default: `warn` — silent unless something breaks
- `--debug` flag forces `debug` level
- `RUST_LOG` env var overrides everything (standard tracing-subscriber behaviour)
- `just dev` injects `RUST_LOG=debug`

---

## Adding new REST endpoints

1. Add route in `server.rs` `create_router()`
2. Write handler `async fn handle_X(State(state): State<Arc<AppState>>, ...) -> impl IntoResponse`
3. If it needs the store: `let Some(store) = &state.store else { return 503 };`
4. Return `(StatusCode::XYZ, Json(json!({...})))` tuples for non-200 paths
5. Update `README.md` architecture section

## Adding new CLI subcommands

1. Add variant to `Command` enum in `src/bin/tldr-cli.rs`
2. Add match arm in `main()`
3. If it needs a daemon call: follow the `client.get(&url).send()` pattern
4. Add shell completion hint with `#[arg(value_hint = ValueHint::Other)]` if needed
5. Update README shell completion section

## Changing the LLM output schema

If you need to add more fields to `LlmSummary`:

1. Extend the struct in `llm.rs`
2. Update the system prompt JSON schema description
3. Update the fallback path (the `Err` arm of `serde_json::from_str`)
4. Update `ChannelSummary` in `output.rs` and the frontend `Summary` interface

## Adding or changing product tour steps

The guided tour is in `frontend/src/lib/ProductTour.ts`.  Update the `STEPS` array
there whenever a new user-facing feature is added to the web UI:

- Each step needs a unique `id`, an `attachTo.element` selector using a `data-tour="…"`
  attribute on the target element, a short `title`, and a `text` description.
- Add the matching `data-tour="…"` attribute to the relevant element in the Svelte
  component.
- Steps that target dynamic content (e.g. channel cards) include a `when.show()` guard
  that calls `this.tour.next()` if the element is absent — keep this pattern.

---

## What NOT to do

- Do not add `since=X` + `per_page=200` back to `get_posts_for_channel` — this truncates unread posts
- Do not remove the JSON parse fallback in `llm.rs` — some LLMs ignore format instructions
- Do not hardcode config paths — always go through `expand_tilde()` / `config.state_db_path()`
- Do not add environment-variable fallback defaults with hardcoded URLs — config.toml is canonical
- Do not use `unwrap()` on lock results in `store.rs` — the Mutex is only poisoned on panic,
  which already terminates the request; this is acceptable, but log it if you add recovery
- Do not remove the `data-tour="…"` attributes from Svelte templates — they are used by
  both the product tour and automated acceptance tests
- Do not use `channel.label()` or `channel.display_name` for DM/GM channels — the API
  returns an empty `display_name` and `label()` falls back to the raw `userid1__userid2`
  slug.  Always resolve DM names via `resolve_dm_display_name()` in `summarise.rs` or
  equivalent user-lookup logic.  Every code path that stores or displays a channel name
  (summaries, RAG records, insights, synthesis prompts) must go through this resolution.
