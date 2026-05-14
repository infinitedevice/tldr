// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! OpenAI-compatible LLM client for channel summarisation.
//!
//! [`LlmClient::summarise`] sends a structured prompt that asks the model to
//! respond with a JSON object `{"summary": "...", "action_items": ["..."]}`.  If
//! the model ignores the format instruction, the raw text is used as the summary
//! with an empty action-items list — this ensures we degrade gracefully across
//! different model backends (Qwen, GPT-4o, etc.).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Strip markdown code fences and attempt to deserialise the LLM's raw text as
/// `T`.  Tries three passes in order:
///
/// 1. Parse the raw text directly (fast path when the model behaves).
/// 2. Strip leading ` ```json ` / ` ``` ` and trailing ` ``` ` fences, then parse.
/// 3. Find the first `{` and last `}` in the (possibly fence-stripped) text and
///    parse just that substring — catches responses where the model emits a
///    preamble sentence or a stray character outside the JSON object.
///
/// Returns `None` only if all three passes fail.
fn try_parse_llm_json<T: DeserializeOwned>(raw: &str) -> Option<T> {
    // Pass 1: direct parse
    if let Ok(v) = serde_json::from_str(raw) {
        return Some(v);
    }

    // Strip fences
    let s = raw.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .trim();
    let stripped = s.strip_suffix("```").unwrap_or(s).trim();

    // Pass 2: parse after fence stripping
    if let Ok(v) = serde_json::from_str(stripped) {
        return Some(v);
    }

    // Pass 3: scan for the outermost JSON object boundaries
    if let (Some(start), Some(end)) = (stripped.find('{'), stripped.rfind('}')) {
        if start <= end {
            if let Ok(v) = serde_json::from_str(&stripped[start..=end]) {
                return Some(v);
            }
        }
    }

    None
}

#[derive(Clone)]
pub struct LlmClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: MessageContent,
}

#[derive(Debug, Deserialize)]
struct MessageContent {
    content: String,
}

pub struct FormattedMessage {
    pub timestamp: String,
    /// Full display string, e.g. "John Doe (@jdoe)"
    pub username: String,
    pub content: String,
}

/// Accepts either a JSON string or an array of strings (joining with newlines).
/// Some LLMs return `"summary": ["bullet 1", "bullet 2"]` despite instructions.
fn deserialize_string_or_vec<'de, D>(de: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(de)?;
    match value {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Array(arr) => {
            let parts: std::result::Result<Vec<String>, _> = arr
                .into_iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| Error::custom("summary array element is not a string"))
                })
                .collect();
            Ok(parts?.join("\n"))
        }
        _ => Err(Error::custom(
            "summary must be a string or array of strings",
        )),
    }
}

/// A single named topic within a channel summary.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TopicPoint {
    pub title: String,
    /// Markdown bullet-point summary for this topic.
    #[serde(deserialize_with = "deserialize_string_or_vec", default)]
    pub summary: String,
}

/// Structured output from the LLM: a list of named topics and extracted action items.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmSummary {
    /// Named topic sections — the primary output format.
    #[serde(default)]
    pub topics: Vec<TopicPoint>,
    /// Flat summary kept for backward-compat fallback (old responses / parse failures).
    #[serde(deserialize_with = "deserialize_string_or_vec", default)]
    pub summary: String,
    #[serde(default)]
    pub action_items: Vec<String>,
    /// Short one-line topic inferred by the LLM; only requested for DM/group channels.
    #[serde(default)]
    pub topic: Option<String>,
}

/// Cross-channel synthesis produced by [`LlmClient::synthesize_insights`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InsightsSummary {
    pub synthesis: String,
    pub themes: Vec<String>,
    pub important_channels: Vec<String>,
    pub open_questions: Vec<String>,
    pub at_risk_items: Vec<String>,
}

impl LlmClient {
    pub fn new(base_url: &str, model: &str, bearer_token: Option<&str>) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = bearer_token {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                    .context("invalid bearer token")?,
            );
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build LLM HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
        })
    }

    /// Summarise unread messages for a channel.
    ///
    /// - `context_messages`: historical posts before the unread window, used as background only
    /// - `unread_messages`: posts the user has not yet read — the focus of the summary
    /// - `prior_action_items`: unresolved action items from previous runs; LLM assesses resolution
    /// - `priority_users`: @usernames whose messages should receive extra attention
    /// - `is_dm`: when true, the LLM is also asked to infer a short conversation topic
    /// - `historical_context`: optional RAG context injected before the unread messages section
    /// - `extra_instructions`: optional per-channel instructions appended to the system prompt
    ///
    /// Returns `(LlmSummary, raw_response)` where `raw_response` is the unprocessed model output
    /// (useful for storing in the vector store as `raw_insight`).
    #[allow(clippy::too_many_arguments)]
    pub async fn summarise(
        &self,
        channel_name: &str,
        context_messages: &[FormattedMessage],
        unread_messages: &[FormattedMessage],
        prior_action_items: &[String],
        priority_users: &[String],
        is_dm: bool,
        historical_context: Option<&str>,
        extra_instructions: Option<&str>,
    ) -> Result<(LlmSummary, String)> {
        if unread_messages.is_empty() {
            return Ok((LlmSummary::default(), String::new()));
        }

        let fmt = |msgs: &[FormattedMessage]| -> String {
            msgs.iter()
                .map(|m| format!("[{}] {}: {}", m.timestamp, m.username, m.content))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut user_prompt = if context_messages.is_empty() {
            format!(
                "Summarise the unread messages from the channel \"{}\":\n\n{}",
                channel_name,
                fmt(unread_messages)
            )
        } else {
            format!(
                "Summarise the unread messages from the channel \"{channel_name}\".\n\n\
                 [CONTEXT] (historical background — do not summarise this):\n{context}\n\n\
                 --- unread messages ---\n{unread}",
                channel_name = channel_name,
                context = fmt(context_messages),
                unread = fmt(unread_messages),
            )
        };

        if !prior_action_items.is_empty() {
            user_prompt.push_str(
                "\n\n[OPEN ACTION ITEMS from previous summaries — assess if resolved]:\n",
            );
            for item in prior_action_items {
                user_prompt.push_str(&format!("- {item}\n"));
            }
        }

        // Inject RAG historical context when available
        if let Some(ctx) = historical_context
            && !ctx.is_empty()
        {
            user_prompt.push_str(
                "\n\n[HISTORICAL CONTEXT — most similar past analyses for this channel]:\n",
            );
            user_prompt.push_str(ctx);
            user_prompt.push_str(
                "\nUse this to: identify recurring vs one-off risk patterns; \
                     compare current activity against historical baseline; \
                     weight topics by past frequency.",
            );
        }

        let topic_instruction = if is_dm {
            " Also infer a short one-line topic (5–10 words) that describes what this conversation \
             is about and include it as the \"topic\" field in the JSON."
        } else {
            ""
        };

        let topic_schema = if is_dm {
            ", \"topic\": \"short topic\""
        } else {
            ""
        };

        let mut system_prompt = format!(
            "You are a concise chat summariser. \
            When a [CONTEXT] section is present, use it only as background — do not summarise it. \
            Focus your summary on the messages after '--- unread messages ---'. \
            If [OPEN ACTION ITEMS] are listed, note which appear resolved vs still pending in your summary. \
            IMPORTANT: Always refer to people by their actual display name as it appears in the messages \
            (format \"Display Name (@username)\") — never write \"a user\", \"a participant\", or \
            \"someone\". \
            Respond with ONLY a JSON object — no markdown fences, no preamble. \
            Format: {{\"topics\": [{{\"title\": \"Short topic title\", \"summary\": \"- bullet\\n- bullet\"}}], \
            \"action_items\": [\"item 1\", \"item 2\"]{topic_schema}}}. \
            Group the conversation into 1–5 named topic sections. Each topic's summary field uses \
            markdown bullet points (start each point with \"- \"). \
            The action_items array contains only new or still-pending action items, \
            as short imperative sentences.{topic_instruction}"
        );

        if !priority_users.is_empty() {
            let names = priority_users
                .iter()
                .map(|u| format!("@{}", u.trim_start_matches('@')))
                .collect::<Vec<_>>()
                .join(", ");
            system_prompt.push_str(&format!(
                " Pay special attention to messages from the following users: {names}. \
                 Highlight their contributions and any decisions or requests they make."
            ));
        }

        if let Some(instructions) = extra_instructions
            && !instructions.is_empty()
        {
            system_prompt.push_str(&format!(
                "\n\nAdditional context for this channel: {instructions}"
            ));
        }

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_prompt },
            ],
            "temperature": 0.3,
        });

        let url = format!("{}/v1/chat/completions", self.base_url);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to send LLM request")?;

        let completion: ChatCompletionResponse = resp
            .error_for_status()
            .context("LLM chat completion request failed")?
            .json()
            .await
            .context("failed to parse LLM response")?;

        let raw = completion
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        // Strip fences / extract JSON; fall back to raw text if all passes fail.
        let result = try_parse_llm_json::<LlmSummary>(&raw).unwrap_or_else(|| LlmSummary {
            topics: vec![TopicPoint {
                title: "Summary".into(),
                summary: raw.clone(),
            }],
            summary: raw.clone(),
            action_items: Vec::new(),
            topic: None,
        });

        Ok((result, raw))
    }

    /// Summarise a batch of emails from a mailbox.
    ///
    /// `emails` is a list of `(from, subject, date, body)` tuples.  Returns the
    /// same [`LlmSummary`] format as [`Self::summarise`], with topics and action items.
    /// `extra_instructions` is optional per-mailbox context injected into the system prompt.
    pub async fn summarise_emails(
        &self,
        mailbox_name: &str,
        emails: &[(String, String, String, String)], // (from, subject, date, body)
        prior_action_items: &[String],
        extra_instructions: Option<&str>,
    ) -> Result<(LlmSummary, String)> {
        if emails.is_empty() {
            return Ok((LlmSummary::default(), String::new()));
        }

        let email_text = emails
            .iter()
            .map(|(from, subject, date, body)| {
                let trimmed_body = body.chars().take(1500).collect::<String>();
                format!(
                    "---\nFrom: {from}\nSubject: {subject}\nDate: {date}\n\n{trimmed_body}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut user_prompt = format!(
            "Summarise the following unread emails from mailbox \"{mailbox_name}\":\n\n{email_text}"
        );

        if !prior_action_items.is_empty() {
            user_prompt.push_str(
                "\n\n[OPEN ACTION ITEMS from previous summaries — assess if resolved]:\n",
            );
            for item in prior_action_items {
                user_prompt.push_str(&format!("- {item}\n"));
            }
        }

        let mut system_prompt =
            "You are a concise email summariser. \
            Summarise the provided emails, grouping related threads by topic. \
            IMPORTANT: Always include the sender's email address and the subject line when \
            referring to specific emails. Never write \"a sender\" or \"someone\" — always \
            use the actual name and address from the From header. \
            Respond with ONLY a JSON object — no markdown fences, no preamble. \
            Format: {\"topics\": [{\"title\": \"Short topic title\", \"summary\": \"- bullet\\n- bullet\"}], \
            \"action_items\": [\"item 1\", \"item 2\"]}. \
            Group emails into 1–5 named topic sections. Each topic's summary field uses \
            markdown bullet points (start each point with \"- \"). \
            The action_items array contains only new or still-pending action items from the emails, \
            as short imperative sentences."
            .to_string();

        if let Some(instructions) = extra_instructions
            && !instructions.is_empty()
        {
            system_prompt.push_str(&format!(
                "\n\nAdditional context for this mailbox: {instructions}"
            ));
        }

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_prompt },
            ],
            "temperature": 0.3,
        });

        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to send email LLM request")?;

        let completion: ChatCompletionResponse = resp
            .error_for_status()
            .context("email LLM chat completion request failed")?
            .json()
            .await
            .context("failed to parse email LLM response")?;

        let raw = completion
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        let result = try_parse_llm_json::<LlmSummary>(&raw).unwrap_or_else(|| LlmSummary {
            topics: vec![TopicPoint {
                title: "Summary".into(),
                summary: raw.clone(),
            }],
            summary: raw.clone(),
            action_items: Vec::new(),
            topic: None,
        });

        Ok((result, raw))
    }

    /// Cross-channel synthesis: asks the LLM to produce a structured narrative from a set of
    /// per-channel insight snapshots. Returns an [`InsightsSummary`] with themes, risks, etc.
    pub async fn synthesize_insights(
        &self,
        insights: &[crate::store::ChannelInsight],
        user_role: &str,
    ) -> Result<InsightsSummary> {
        if insights.is_empty() {
            return Ok(InsightsSummary::default());
        }

        let channel_summaries = insights
            .iter()
            .map(|s| format!("## {}/{}\n{}", s.team_name, s.channel_name, s.summary_text))
            .collect::<Vec<_>>()
            .join("\n\n");

        let role_context = if user_role.trim().is_empty() {
            String::new()
        } else {
            format!(" The recipient is: {user_role}.")
        };

        let user_prompt = format!(
            "Synthesise the following channel summaries into a cross-channel insight report.{role_context}\n\n{channel_summaries}"
        );

        let system_prompt = "You are a cross-channel intelligence analyst. \
            Given a set of per-channel summaries, produce a concise synthesis. \
            Respond with ONLY a JSON object — no markdown fences, no preamble. \
            Format: {\"synthesis\": \"...\", \"themes\": [\"...\"], \
            \"important_channels\": [\"...\"], \"open_questions\": [\"...\"], \
            \"at_risk_items\": [\"...\"]}. \
            synthesis: 2–4 sentence narrative across all channels. \
            themes: top recurring topics (short noun phrases). \
            important_channels: channels requiring immediate attention, as \"team/channel\" strings. \
            open_questions: unresolved cross-channel questions needing follow-up. \
            at_risk_items: blockers or risks that need action.";

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_prompt },
            ],
            "temperature": 0.3,
        });

        let url = format!("{}/v1/chat/completions", self.base_url);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to send insights LLM request")?;

        let completion: ChatCompletionResponse = resp
            .error_for_status()
            .context("insights LLM request failed")?
            .json()
            .await
            .context("failed to parse insights LLM response")?;

        let raw = completion
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        Ok(
            try_parse_llm_json::<InsightsSummary>(&raw).unwrap_or_else(|| InsightsSummary {
                synthesis: raw,
                ..Default::default()
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_json() {
        let raw = r#"{"summary":"All good","action_items":[]}"#;
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        assert_eq!(r.unwrap().summary, "All good");
    }

    #[test]
    fn parse_json_fenced_with_json_tag() {
        let raw = "```json\n{\"summary\":\"fenced\",\"action_items\":[]}\n```";
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        assert_eq!(r.unwrap().summary, "fenced");
    }

    #[test]
    fn parse_json_fenced_without_tag() {
        let raw = "```\n{\"summary\":\"bare\",\"action_items\":[]}\n```";
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        assert_eq!(r.unwrap().summary, "bare");
    }

    #[test]
    fn parse_json_with_preamble_and_trailing_text() {
        let raw = "Here is the summary:\n{\"summary\":\"preamble\",\"action_items\":[]}\nDone.";
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        assert_eq!(r.unwrap().summary, "preamble");
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        let r: Option<LlmSummary> = try_parse_llm_json("not json at all");
        assert!(r.is_none());
    }

    #[test]
    fn parse_action_items_plain_strings() {
        let raw = r#"{"summary":"ok","action_items":["Task A","Task B"]}"#;
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        let items = r.unwrap().action_items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "Task A");
        assert!(items[0].source_ids.is_empty());
    }

    #[test]
    fn parse_action_items_with_source_ids() {
        let raw = r#"{"summary":"ok","action_items":[{"text":"Fix it","source_ids":["msg1","msg2"]}]}"#;
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        let item = &r.unwrap().action_items[0];
        assert_eq!(item.text, "Fix it");
        assert_eq!(item.source_ids, vec!["msg1", "msg2"]);
    }

    #[test]
    fn parse_summary_as_array_of_strings() {
        let raw = r#"{"summary":["line one","line two"],"action_items":[]}"#;
        let r: Option<LlmSummary> = try_parse_llm_json(raw);
        assert!(r.is_some());
        assert_eq!(r.unwrap().summary, "line one\nline two");
    }
}
