# tldr

[![CI](https://github.com/infinitedevice/tldr/actions/workflows/ci.yml/badge.svg)](https://github.com/infinitedevice/tldr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/infinitedevice/tldr)](https://github.com/infinitedevice/tldr/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)

A Mattermost channel summarisation tool. A background daemon polls your unread channels, passes recent messages to a local or remote LLM (via an OpenAI-compatible API), and serves summaries both as a web UI and via a terminal CLI.

---

## Features

- **Web UI** — per-team cards with rendered markdown, channel hyperlinks, action item tracking, theme switcher (Skeleton UI)
- **CLI** — ANSI-rendered summaries with OSC 8 hyperlinks, mentions-prioritised ordering
- **Historical context** — each summary includes one working day of message history as LLM background; first-run seeding backfills ~4 weeks of insights
- **Action items** — extracted by the LLM, stored in SQLite, tracked across runs; mark as ignored via the web UI
- **Read-state watermarks** — SQLite DB so repeated runs are incremental
- **Config wizard** — first-run web wizard if no config file is present (Flatpak-compatible)
- **Shell completions** — bash, zsh, fish, PowerShell, Elvish
- **Systemd user service** — `tldr-cli install-service` for boot-time startup

---

## Quick Install

### Flatpak (recommended for desktop users)

Download `tldr.flatpak` from the [latest release](https://github.com/infinitedevice/tldr/releases/latest):

```bash
flatpak install --user tldr.flatpak
flatpak run dev.tldr.App
```

Open `http://localhost:8765` in your browser. The config wizard will appear on first run.

### Cargo

```bash
cargo install --git https://github.com/infinitedevice/tldr --bins
```

Start the daemon:

```bash
tldr-daemon
```

Optionally install as a systemd user service (runs on login, survives reboots):

```bash
tldr-cli install-service
```

Summarise via CLI:

```bash
tldr-cli summarise
```

---

## Shell Completions

```bash
# bash — add to ~/.bashrc
eval "$(tldr-cli completions bash)"

# zsh — add to ~/.zshrc
eval "$(tldr-cli completions zsh)"

# fish
tldr-cli completions fish > ~/.config/fish/completions/tldr.fish
```

---

## Configuration

Config is stored at `~/.config/tldr/config.toml` (or `$XDG_CONFIG_HOME/tldr/config.toml` inside Flatpak).

```toml
[mattermost]
server_url = "https://chat.example.com"
token      = "your-personal-access-token"

[llm]
# Any OpenAI-compatible endpoint
base_url     = "https://api.openai.com/v1"
model        = "gpt-4o"
bearer_token = "sk-..."

[paths]
state_db = "~/.config/tldr/state.db"   # set to "" to disable watermarks

[server]
listen_addr        = "127.0.0.1:8765"
poll_interval_secs = 600   # background update every 10 min; 0 = disabled
```

See [`config.example.toml`](config.example.toml) for all options.

---

## Dev Setup

Requirements: Rust stable, Node 22+, [just](https://github.com/casey/just), [cargo-watch](https://github.com/watchexec/cargo-watch), [systemfd](https://github.com/mitsuhiko/systemfd).

```bash
# Backend with hot-reload (debug logging enabled)
just dev

# Frontend dev server with HMR
just dev-frontend

# Both together
just dev-all

# Run tests + lint
just test
just lint
```

---

## Architecture

```text
tldr-daemon  (axum HTTP server, port 8765)
  ├── Background loop (every poll_interval_secs)
  │     summarise_all_unread → SQLite cache + SSE broadcast
  │
  ├── GET  /api/v1/summaries           — cached summaries (instant)
  ├── GET  /api/v1/summaries/subscribe — SSE stream of live updates
  ├── GET  /api/v1/summarise           — on-demand summarise (JSON)
  ├── GET  /api/v1/summarise/stream    — on-demand NDJSON summarise
  ├── GET  /api/v1/health              — daemon health + poll_interval_secs
  ├── GET  /api/v1/me                  — authenticated user info
  ├── GET  /api/v1/me/avatar           — user avatar proxy
  ├── GET  /api/v1/config/status       — check if configured
  ├── GET  /api/v1/config              — read config (tokens redacted)
  ├── PUT  /api/v1/config              — save config (wizard)
  ├── GET  /api/v1/channels            — all subscribed channels
  ├── GET  /api/v1/channels/unread     — unread counts per channel
  ├── GET  /api/v1/channels/categories — channel sidebar categories
  ├── POST /api/v1/channels/:id/read   — mark channel as read
  ├── GET  /api/v1/action-items        — pending action items
  ├── PATCH /api/v1/action-items/:id   — mark ignored/resolved
  ├── GET  /api/v1/user-prefs          — user preferences
  ├── PUT  /api/v1/user-prefs          — update user preferences
  ├── GET  /api/v1/insights            — historical channel insights
  ├── GET  /api/v1/favourites          — favourite channels list
  ├── POST /api/v1/favourites/:id      — add favourite
  ├── DELETE /api/v1/favourites/:id    — remove favourite
  ├── GET  /api/v1/seeding/status      — first-run seeding progress
  ├── DELETE /api/v1/state             — clear watermarks
  ├── GET  /llms.txt                   — LLM-readable site description
  ├── POST /slash/summarise            — Mattermost slash command
  └── /*                               — serve frontend/dist (ServeDir)

tldr-cli                               — HTTP client to the daemon
```

State is persisted in `~/.config/tldr/state.db` (SQLite). Tables: `channel_watermark`, `action_item`, `cached_summary`, `user_prefs`, `channel_insights`, `favourite_channel`.

See [AGENTS.md](AGENTS.md) for a detailed architecture guide aimed at AI coding agents.

---

## Contributing

- Commits must follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, etc.)
- Releases are automated via [release-please](https://github.com/googleapis/release-please)
- PRs must pass CI (cargo test + clippy + fmt + frontend build)

---

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
