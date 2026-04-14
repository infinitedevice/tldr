// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **tldr** — Mattermost chat summarisation daemon library.
//!
//! Re-exports all public modules so the two binary crates (`tldr-daemon`, `tldr-cli`)
//! and any integration tests share a single source tree.

pub mod config;
pub mod daemon;
pub mod llm;
pub mod mattermost;
pub mod mattermost_types;
pub mod output;
pub mod rag;
pub mod seeding;
pub mod server;
pub mod store;
pub mod summarise;
