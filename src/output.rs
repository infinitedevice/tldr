// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terminal output formatting and shared `ChannelSummary` type.
//!
//! [`ChannelSummary`] is the canonical transfer type used by both the HTTP JSON
//! API and the CLI terminal renderer.  [`print_summaries`] renders summaries to
//! stdout using `termimad` for markdown and OSC 8 hyperlinks for channel URLs.

use colored::Colorize;
use serde::{Deserialize, Serialize};
use termimad::MadSkin;
use terminal_size::{Width, terminal_size};

/// A single action item attached to a channel summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItemSummary {
    pub id: String,
    pub text: String,
}

/// A named topic section within a channel summary, with pre-rendered HTML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicSection {
    pub title: String,
    pub summary_html: String,
    /// Raw markdown for terminal rendering (termimad).
    #[serde(default)]
    pub summary_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSummary {
    pub team_name: String,
    pub channel_name: String,
    pub channel_id: String,
    pub channel_url: String,
    pub unread_count: i64,
    pub mention_count: i64,
    pub summary: String,
    pub summary_html: String,
    /// Named topic sections — primary display. Falls back to summary_html when empty.
    #[serde(default)]
    pub topics: Vec<TopicSection>,
    #[serde(default)]
    pub action_items: Vec<ActionItemSummary>,
    /// Conversation topic inferred by the LLM (DM / group channels only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Display names of participants who posted in the unread window (DM / group channels only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
}

pub fn print_summaries(summaries: &[ChannelSummary]) {
    if summaries.is_empty() {
        println!("{}", "No unread channels to summarise.".dimmed());
        return;
    }

    let term_width = terminal_size()
        .map(|(Width(w), _)| w as usize)
        .unwrap_or(80);

    let skin = MadSkin::default();
    let mut current_team = String::new();

    for s in summaries {
        if s.team_name != current_team {
            if !current_team.is_empty() {
                println!();
            }
            println!("{}", format!("═══ {} ═══", s.team_name).bold().cyan());
            current_team = s.team_name.clone();
        }

        println!();

        // Channel name as a link (OSC 8 hyperlink when URL is available)
        let channel_label = format!("#{}", s.channel_name);
        if !s.channel_url.is_empty() {
            // OSC 8 hyperlink escape sequence
            print!(
                "  \x1b]8;;{url}\x1b\\{label}\x1b]8;;\x1b\\",
                url = s.channel_url,
                label = channel_label.green().bold()
            );
        } else {
            print!("  {}", channel_label.green().bold());
        }

        let mut badges = Vec::new();
        if s.unread_count > 0 {
            badges.push(format!("{} unread", s.unread_count).yellow().to_string());
        }
        if s.mention_count > 0 {
            badges.push(
                format!("{} mentions", s.mention_count)
                    .red()
                    .bold()
                    .to_string(),
            );
        }
        if !badges.is_empty() {
            print!(" ({})", badges.join(", "));
        }
        println!();

        // Topic subtitle for DM / group channels
        if let Some(topic) = &s.topic {
            println!("    {}", topic.italic().dimmed());
        }
        // Participants for DM / group channels
        if !s.participants.is_empty() {
            println!(
                "    {} {}",
                "with:".dimmed(),
                s.participants.join(", ").dimmed()
            );
        }

        println!();

        // Render markdown with termimad, indented by 4 chars
        let indent_width = term_width.saturating_sub(4);
        if !s.topics.is_empty() {
            for topic in &s.topics {
                println!("    {}", topic.title.bold());
                let rendered = skin.text(&topic.summary_md, Some(indent_width));
                for line in format!("{}", rendered).lines() {
                    println!("    {line}");
                }
            }
        } else {
            let rendered = skin.text(&s.summary, Some(indent_width));
            for line in format!("{}", rendered).lines() {
                println!("    {line}");
            }
        }

        if !s.action_items.is_empty() {
            println!();
            println!("    {}", "Action items:".bold().yellow());
            for item in &s.action_items {
                println!("    {} {}", "→".yellow(), item.text);
            }
        }
    }

    println!();
    println!(
        "{}",
        format!("Summarised {} channel(s).", summaries.len()).dimmed()
    );
}
