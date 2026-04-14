// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Serde-deserializable types mirroring the Mattermost REST API v4 response shapes.
//!
//! Only the fields used by this project are declared; unknown fields are silently
//! ignored thanks to serde's default behaviour.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub email: String,
}

impl User {
    pub fn display_name(&self) -> &str {
        if !self.nickname.is_empty() {
            &self.nickname
        } else if !self.first_name.is_empty() {
            &self.first_name
        } else {
            &self.username
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Team {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(rename = "type", default)]
    pub channel_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub header: String,
}

impl Channel {
    pub fn label(&self) -> &str {
        if !self.display_name.is_empty() {
            &self.display_name
        } else {
            &self.name
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelUnread {
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub msg_count: i64,
    #[serde(default)]
    pub mention_count: i64,
    #[serde(default)]
    pub last_view_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Post {
    pub id: String,
    #[serde(default)]
    pub create_at: i64,
    #[serde(default)]
    pub update_at: i64,
    #[serde(default)]
    pub delete_at: i64,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub root_id: String,
    #[serde(default)]
    pub message: String,
    #[serde(rename = "type", default)]
    pub post_type: String,
}

impl Post {
    /// Returns true if this is a regular user message (not a system post).
    pub fn is_user_message(&self) -> bool {
        self.post_type.is_empty() && self.delete_at == 0
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostList {
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub posts: HashMap<String, Post>,
    #[serde(default)]
    pub next_post_id: String,
    #[serde(default)]
    pub prev_post_id: String,
    #[serde(default)]
    pub has_next: bool,
}

impl PostList {
    /// Returns posts sorted by create_at ascending (oldest first).
    pub fn sorted_posts(&self) -> Vec<&Post> {
        let mut posts: Vec<&Post> = self.posts.values().collect();
        posts.sort_by_key(|p| p.create_at);
        posts
    }
}

/// A single Mattermost sidebar category (e.g. "Favorites", "Channels", "Direct Messages", custom).
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelCategory {
    pub id: String,
    #[serde(rename = "type", default)]
    pub category_type: String, // "favorites" | "channels" | "direct_messages" | "custom"
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub channel_ids: Vec<String>,
}

/// Response wrapper from `GET /users/{uid}/teams/{tid}/channels/categories`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelCategoryList {
    pub categories: Vec<ChannelCategory>,
    #[serde(default)]
    pub order: Vec<String>,
}
