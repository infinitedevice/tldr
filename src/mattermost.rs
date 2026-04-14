// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Async Mattermost REST API v4 client.
//!
//! [`MattermostClient`] wraps a [`reqwest::Client`] pre-configured with the
//! bearer token and a 30-second timeout.  A user-object cache avoids redundant
//! `/users/{id}` calls when the same author appears multiple times in a batch.
//!
//! **Important**: [`get_posts_for_channel`][`MattermostClient::get_posts_for_channel`]
//! uses page-based pagination (newest-first) rather than `since=`, because the
//! `since` parameter returns oldest-first and silently truncates at 200 posts,
//! which drops the most-recent (unread) messages — see `summarise.rs` for details.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::mattermost_types::*;

#[derive(Clone)]
pub struct MattermostClient {
    client: reqwest::Client,
    base_url: String,
    user_cache: Arc<Mutex<HashMap<String, User>>>,
}

impl MattermostClient {
    pub fn new(server_url: &str, token: &str) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .context("invalid token")?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;

        let base_url = server_url.trim_end_matches('/').to_string();

        Ok(Self {
            client,
            base_url,
            user_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/api/v4{}", self.base_url, path)
    }

    pub async fn get_me(&self) -> Result<User> {
        let resp = self
            .client
            .get(self.api_url("/users/me"))
            .send()
            .await
            .context("failed to request /users/me")?;
        let user: User = resp
            .error_for_status()
            .context("GET /users/me failed")?
            .json()
            .await
            .context("failed to parse user response")?;
        Ok(user)
    }

    pub async fn get_teams_for_user(&self, user_id: &str) -> Result<Vec<Team>> {
        let resp = self
            .client
            .get(self.api_url(&format!("/users/{user_id}/teams")))
            .send()
            .await
            .context("failed to request user teams")?;
        let teams: Vec<Team> = resp
            .error_for_status()
            .context("GET /users/{user_id}/teams failed")?
            .json()
            .await
            .context("failed to parse teams response")?;
        Ok(teams)
    }

    pub async fn get_channels_for_team_for_user(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Vec<Channel>> {
        let resp = self
            .client
            .get(self.api_url(&format!("/users/{user_id}/teams/{team_id}/channels")))
            .send()
            .await
            .context("failed to request user channels")?;
        let channels: Vec<Channel> = resp
            .error_for_status()
            .context("GET channels for team failed")?
            .json()
            .await
            .context("failed to parse channels response")?;
        Ok(channels)
    }

    pub async fn get_channel_unread(
        &self,
        user_id: &str,
        channel_id: &str,
    ) -> Result<ChannelUnread> {
        let resp = self
            .client
            .get(self.api_url(&format!("/users/{user_id}/channels/{channel_id}/unread")))
            .send()
            .await
            .context("failed to request channel unread")?;
        let unread: ChannelUnread = resp
            .error_for_status()
            .context("GET channel unread failed")?
            .json()
            .await
            .context("failed to parse channel unread response")?;
        Ok(unread)
    }

    pub async fn get_posts_around_last_unread(
        &self,
        user_id: &str,
        channel_id: &str,
        limit_before: u32,
        limit_after: u32,
    ) -> Result<PostList> {
        let resp = self
            .client
            .get(self.api_url(&format!(
                "/users/{user_id}/channels/{channel_id}/posts/unread"
            )))
            .query(&[
                ("limit_before", limit_before.to_string()),
                ("limit_after", limit_after.to_string()),
            ])
            .send()
            .await
            .context("failed to request posts around unread")?;
        let posts: PostList = resp
            .error_for_status()
            .context("GET posts around unread failed")?
            .json()
            .await
            .context("failed to parse posts around unread response")?;
        Ok(posts)
    }

    pub async fn get_posts_for_channel(
        &self,
        channel_id: &str,
        since: Option<i64>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<PostList> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(s) = since {
            query.push(("since", s.to_string()));
        }
        if let Some(p) = page {
            query.push(("page", p.to_string()));
        }
        if let Some(pp) = per_page {
            query.push(("per_page", pp.to_string()));
        }

        let resp = self
            .client
            .get(self.api_url(&format!("/channels/{channel_id}/posts")))
            .query(&query)
            .send()
            .await
            .context("failed to request channel posts")?;
        let posts: PostList = resp
            .error_for_status()
            .context("GET channel posts failed")?
            .json()
            .await
            .context("failed to parse channel posts response")?;
        Ok(posts)
    }

    pub async fn get_user(&self, user_id: &str) -> Result<User> {
        // Check cache first
        {
            let cache = self.user_cache.lock().await;
            if let Some(user) = cache.get(user_id) {
                return Ok(user.clone());
            }
        }

        let resp = self
            .client
            .get(self.api_url(&format!("/users/{user_id}")))
            .send()
            .await
            .context("failed to request user")?;
        let user: User = resp
            .error_for_status()
            .context("GET user failed")?
            .json()
            .await
            .context("failed to parse user response")?;

        // Store in cache
        {
            let mut cache = self.user_cache.lock().await;
            cache.insert(user_id.to_string(), user.clone());
        }

        Ok(user)
    }

    pub async fn get_post_thread(&self, post_id: &str) -> Result<PostList> {
        let resp = self
            .client
            .get(self.api_url(&format!("/posts/{post_id}/thread")))
            .send()
            .await
            .context("failed to request post thread")?;
        let posts: PostList = resp
            .error_for_status()
            .context("GET post thread failed")?
            .json()
            .await
            .context("failed to parse post thread response")?;
        Ok(posts)
    }

    pub async fn create_post(&self, channel_id: &str, message: &str) -> Result<Post> {
        let body = serde_json::json!({
            "channel_id": channel_id,
            "message": message,
        });

        let resp = self
            .client
            .post(self.api_url("/posts"))
            .json(&body)
            .send()
            .await
            .context("failed to create post")?;
        let post: Post = resp
            .error_for_status()
            .context("POST /posts failed")?
            .json()
            .await
            .context("failed to parse create post response")?;
        Ok(post)
    }

    /// Verify connectivity by calling the Mattermost system ping endpoint.
    pub async fn health_check(&self) -> Result<()> {
        let resp = self
            .client
            .get(self.api_url("/system/ping"))
            .send()
            .await
            .context("Mattermost health check: network error")?;
        if !resp.status().is_success() {
            anyhow::bail!("Mattermost health check failed: HTTP {}", resp.status());
        }
        Ok(())
    }

    /// Fetch the raw avatar image bytes for a user.
    ///
    /// Returns `(bytes, content_type)`. The content-type is taken from the
    /// Mattermost response header, defaulting to `"image/jpeg"`.
    pub async fn get_user_avatar_bytes(&self, user_id: &str) -> Result<(Vec<u8>, String)> {
        let resp = self
            .client
            .get(self.api_url(&format!("/users/{user_id}/image")))
            .send()
            .await
            .context("failed to request user avatar")?;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let bytes = resp
            .error_for_status()
            .context("avatar request failed")?
            .bytes()
            .await
            .context("failed to read avatar bytes")?
            .to_vec();
        Ok((bytes, content_type))
    }

    /// Fetch sidebar channel categories for a user + team.
    ///
    /// Calls `GET /api/v4/users/{uid}/teams/{tid}/channels/categories`.
    pub async fn get_channel_categories(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Vec<crate::mattermost_types::ChannelCategory>> {
        use crate::mattermost_types::ChannelCategoryList;
        let resp = self
            .client
            .get(self.api_url(&format!(
                "/users/{user_id}/teams/{team_id}/channels/categories"
            )))
            .send()
            .await
            .context("failed to request channel categories")?;
        let list: ChannelCategoryList = resp
            .error_for_status()
            .context("GET channel categories failed")?
            .json()
            .await
            .context("failed to parse channel categories")?;
        Ok(list.categories)
    }
}
