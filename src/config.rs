// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Configuration loading, validation, and persistence.
//!
//! Config is stored as TOML at `~/.config/tldr/config.toml` (tilde-expanded via
//! [`expand_tilde`]).  [`Config::load`] returns an error if the file is absent —
//! the daemon handles that by falling back to [`Config::default`] for degraded-mode
//! startup, and the user completes setup through the web UI wizard.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    pub mattermost: MattermostConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub server: ServerConfig,
    /// Mattermost @usernames whose messages should be highlighted in summaries.
    #[serde(default)]
    pub priority_users: Vec<String>,
    /// Free-text description of the user's role, used to personalise insight synthesis.
    #[serde(default)]
    pub user_role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MattermostConfig {
    #[serde(default = "default_mm_server_url")]
    pub server_url: String,
    #[serde(default = "default_mm_token")]
    pub token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(
        default = "default_llm_bearer_token",
        skip_serializing_if = "Option::is_none"
    )]
    pub bearer_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PathsConfig {
    #[serde(default = "default_state_file")]
    pub state_file: String,
    /// SQLite database for channel watermarks. Set to "" to disable.
    #[serde(default = "default_state_db")]
    pub state_db: String,
    /// Directory for LanceDB vector store (RAG historical context). Set to "" to disable.
    #[serde(default = "default_vectors_dir")]
    pub vectors_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slash_token: Option<String>,
    /// Background summarisation polling interval in seconds (0 = disabled).
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
}

fn default_mm_server_url() -> String {
    String::new()
}

fn default_mm_token() -> String {
    String::new()
}

fn default_llm_base_url() -> String {
    String::new()
}

fn default_llm_model() -> String {
    String::new()
}

fn default_llm_bearer_token() -> Option<String> {
    None
}

fn default_state_file() -> String {
    "~/.config/tldr/state.json".to_string()
}

fn default_state_db() -> String {
    "~/.config/tldr/state.db".to_string()
}

fn default_vectors_dir() -> String {
    "~/.config/tldr/data/vectors".to_string()
}

fn default_listen_addr() -> String {
    "127.0.0.1:8765".to_string()
}

fn default_poll_interval_secs() -> u64 {
    600
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: default_llm_base_url(),
            model: default_llm_model(),
            bearer_token: default_llm_bearer_token(),
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            state_file: default_state_file(),
            state_db: default_state_db(),
            vectors_dir: default_vectors_dir(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            slash_token: None,
            poll_interval_secs: default_poll_interval_secs(),
        }
    }
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs_home()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl Default for MattermostConfig {
    fn default() -> Self {
        Self {
            server_url: default_mm_server_url(),
            token: default_mm_token(),
        }
    }
}

impl Config {
    /// Load config from file. Returns an error if the file does not exist.
    ///
    /// Call [`Config::validate`] on the result to confirm required fields are populated.
    /// The daemon uses [`Config::default`] (env-var fallback) when the file is absent
    /// so the web UI can still be served for the initial setup wizard.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!(
                "Config file not found at {}.\n\
                 Run the web UI setup wizard or create it manually.\n\
                 Minimum required fields: [mattermost] server_url and token.",
                path.display()
            );
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let config: Config =
            toml::from_str(&contents).with_context(|| "failed to parse config file")?;
        Ok(config)
    }

    /// Return a list of human-readable validation errors for required fields.
    ///
    /// An empty list means the config is ready for use.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.mattermost.server_url.is_empty() {
            errors.push("mattermost.server_url is not set".to_string());
        }
        if self.mattermost.token.is_empty() {
            errors.push("mattermost.token is not set".to_string());
        }
        if self.llm.base_url.is_empty() {
            errors.push("llm.base_url is not set".to_string());
        }
        if self.llm.model.is_empty() {
            errors.push("llm.model is not set".to_string());
        }
        errors
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).context("failed to serialise config")?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn state_file_path(&self) -> PathBuf {
        expand_tilde(&self.paths.state_file)
    }

    pub fn state_db_path(&self) -> Option<PathBuf> {
        if self.paths.state_db.is_empty() {
            None
        } else {
            Some(expand_tilde(&self.paths.state_db))
        }
    }

    /// Path to the LanceDB vector store directory, or `None` if RAG is disabled.
    pub fn vectors_dir_path(&self) -> Option<PathBuf> {
        if self.paths.vectors_dir.is_empty() {
            None
        } else {
            Some(expand_tilde(&self.paths.vectors_dir))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_empty() {
        let config = Config::default();
        assert!(config.mattermost.server_url.is_empty());
        assert!(config.mattermost.token.is_empty());
        assert!(config.llm.base_url.is_empty());
        assert!(config.llm.model.is_empty());
        assert!(config.llm.bearer_token.is_none());
    }

    #[test]
    fn validate_catches_empty_fields() {
        let config = Config::default();
        let errors = config.validate();
        assert_eq!(errors.len(), 4);
        assert!(errors.iter().any(|e| e.contains("server_url")));
        assert!(errors.iter().any(|e| e.contains("token")));
        assert!(errors.iter().any(|e| e.contains("base_url")));
        assert!(errors.iter().any(|e| e.contains("model")));
    }

    #[test]
    fn validate_passes_when_configured() {
        let config = Config {
            mattermost: MattermostConfig {
                server_url: "https://chat.example.com".into(),
                token: "tok".into(),
            },
            llm: LlmConfig {
                base_url: "https://llm.example.com".into(),
                model: "gpt-4o".into(),
                bearer_token: None,
            },
            ..Config::default()
        };
        assert!(config.validate().is_empty());
    }

    #[test]
    fn load_from_toml_string() {
        let toml = r#"
[mattermost]
server_url = "https://chat.example.com"
token = "tok123"

[llm]
base_url = "https://llm.example.com"
model = "gpt-4o"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.mattermost.server_url, "https://chat.example.com");
        assert_eq!(config.llm.model, "gpt-4o");
        assert!(config.validate().is_empty());
    }

    #[test]
    fn poll_interval_defaults_to_600() {
        let toml = r#"
[mattermost]
server_url = "x"
token = "x"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.poll_interval_secs, 600);
    }

    #[test]
    fn save_round_trip() {
        let config = Config {
            mattermost: MattermostConfig {
                server_url: "https://chat.example.com".into(),
                token: "tok".into(),
            },
            llm: LlmConfig {
                base_url: "https://llm.example.com".into(),
                model: "gpt-4o".into(),
                bearer_token: Some("secret".into()),
            },
            priority_users: vec!["alice".into(), "bob".into()],
            ..Config::default()
        };
        let dir = std::env::temp_dir().join("tldr_test_config");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.mattermost.server_url, config.mattermost.server_url);
        assert_eq!(loaded.llm.model, config.llm.model);
        assert_eq!(loaded.llm.bearer_token, Some("secret".into()));
        assert_eq!(loaded.priority_users, vec!["alice", "bob"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_tilde_resolves_home() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let p = expand_tilde("~/foo/bar");
            assert!(p.to_string_lossy().starts_with(&home));
            assert!(p.to_string_lossy().ends_with("foo/bar"));
        }
    }
}
