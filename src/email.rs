// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! IMAP client for fetching unread emails.
//!
//! Each call to [`ImapClient::connect`] establishes a fresh TLS connection and
//! logs in.  The daemon reconnects on every background cycle rather than
//! maintaining a long-lived IMAP session to avoid idle-timeout complexity.

use anyhow::{Context, Result};
use async_imap::Session;
use async_native_tls::TlsStream;
use futures::TryStreamExt;
use mail_parser::MessageParser;
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

use crate::config::EmailConfig;

type ImapSession = Session<TlsStream<Compat<TcpStream>>>;

pub struct ImapClient {
    session: ImapSession,
    current_mailbox: Option<String>,
}

/// A fully-parsed email message.
#[derive(Debug)]
pub struct FullMessage {
    pub uid: u32,
    pub subject: String,
    pub from: String,
    pub date: String,
    pub body: String,
}

impl ImapClient {
    pub async fn connect(config: &EmailConfig) -> Result<Self> {
        let addr = format!("{}:{}", config.server, config.port);
        let tcp = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("failed to connect to IMAP server {addr}"))?;
        let tcp = tcp.compat();

        let tls = async_native_tls::connect(&config.server, tcp)
            .await
            .context("TLS handshake with IMAP server failed")?;

        let mut client = async_imap::Client::new(tls);
        client
            .read_response()
            .await
            .context("failed to read IMAP greeting")?;

        let session = client
            .login(&config.username, &config.password)
            .await
            .map_err(|(err, _)| err)
            .context("IMAP login failed")?;

        Ok(Self {
            session,
            current_mailbox: None,
        })
    }

    async fn select_mailbox(&mut self, mailbox: &str) -> Result<()> {
        if self.current_mailbox.as_deref() == Some(mailbox) {
            return Ok(());
        }
        self.session
            .select(mailbox)
            .await
            .with_context(|| format!("failed to select mailbox: {mailbox}"))?;
        self.current_mailbox = Some(mailbox.to_string());
        Ok(())
    }

    /// Fetch unread (UNSEEN) messages from `mailbox` with UID > `since_uid`.
    ///
    /// When `since_uid == 0` (first run), falls back to fetching messages
    /// received in the last `days_back` days instead, to avoid pulling the
    /// entire mailbox history.
    pub async fn fetch_unread_since_uid(
        &mut self,
        mailbox: &str,
        since_uid: u32,
        days_back: u32,
    ) -> Result<Vec<FullMessage>> {
        self.select_mailbox(mailbox).await?;

        // Build search criteria
        let query = if since_uid > 0 {
            format!("UNSEEN UID {}:*", since_uid + 1)
        } else {
            // First run: fetch UNSEEN messages from the last N days
            let since_date = {
                let now = jiff::Zoned::now();
                let past = now
                    .checked_sub(jiff::Span::new().days(days_back as i64))
                    .unwrap_or(now);
                // IMAP date format: "DD-Mon-YYYY"
                past.strftime("%d-%b-%Y").to_string()
            };
            format!("UNSEEN SINCE {since_date}")
        };

        let mut uids: Vec<u32> = self
            .session
            .uid_search(&query)
            .await
            .context("IMAP search failed")?
            .into_iter()
            .collect();
        uids.sort();

        if uids.is_empty() {
            return Ok(Vec::new());
        }

        // Cap at 50 messages per mailbox to avoid sending huge prompts to the LLM
        let cap = 50usize;
        if uids.len() > cap {
            uids = uids.split_off(uids.len() - cap);
        }

        let uid_set = compress_uid_set(&uids);
        let fetches: Vec<_> = self
            .session
            .uid_fetch(&uid_set, "(UID FLAGS RFC822)")
            .await?
            .try_collect()
            .await?;

        let parser = MessageParser::default();
        let mut messages = Vec::new();

        for fetch in &fetches {
            let raw = fetch.body().unwrap_or_default();
            let uid = fetch.uid.unwrap_or(0);

            let (subject, from, date, body) = if let Some(msg) = parser.parse(raw) {
                let subject = msg.subject().unwrap_or("(no subject)").to_string();
                let from = format_address(msg.from());
                let date = msg
                    .date()
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "unknown date".to_string());
                let body = msg
                    .body_text(0)
                    .map(|c| c.into_owned())
                    .unwrap_or_default();
                (subject, from, date, body)
            } else {
                (
                    "(no subject)".to_string(),
                    String::new(),
                    String::new(),
                    String::from_utf8_lossy(raw).into_owned(),
                )
            };

            messages.push(FullMessage {
                uid,
                subject,
                from,
                date,
                body,
            });
        }

        Ok(messages)
    }

    pub async fn logout(&mut self) -> Result<()> {
        self.session.logout().await?;
        Ok(())
    }

    /// List all mailboxes visible to the authenticated user.
    ///
    /// Runs `LIST "" "*"` which returns the full hierarchy. Mailboxes with
    /// the `\Noselect` attribute are excluded (they are namespace containers,
    /// not real mailboxes).
    pub async fn list_mailboxes(&mut self) -> Result<Vec<String>> {
        use futures::TryStreamExt;
        use async_imap::types::NameAttribute;

        let names: Vec<_> = self
            .session
            .list(None, Some("*"))
            .await
            .context("IMAP LIST failed")?
            .try_collect()
            .await
            .context("failed to collect IMAP LIST response")?;

        let mut mailboxes: Vec<String> = names
            .iter()
            .filter(|n| !n.attributes().contains(&NameAttribute::NoSelect))
            .map(|n| n.name().to_string())
            .collect();

        mailboxes.sort();
        Ok(mailboxes)
    }
}

/// Compress a sorted list of UIDs into IMAP range notation (e.g. "1:5,7,9:12").
fn compress_uid_set(uids: &[u32]) -> String {
    if uids.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    let mut start = uids[0];
    let mut end = uids[0];
    for &uid in &uids[1..] {
        if uid == end + 1 {
            end = uid;
        } else {
            if start == end {
                parts.push(start.to_string());
            } else {
                parts.push(format!("{start}:{end}"));
            }
            start = uid;
            end = uid;
        }
    }
    if start == end {
        parts.push(start.to_string());
    } else {
        parts.push(format!("{start}:{end}"));
    }
    parts.join(",")
}

fn format_address(addr: Option<&mail_parser::Address>) -> String {
    use mail_parser::Address;
    match addr {
        Some(Address::List(list)) => list
            .iter()
            .map(|a| {
                if let Some(name) = &a.name {
                    format!("{name} <{}>", a.address.as_deref().unwrap_or(""))
                } else {
                    a.address.as_deref().unwrap_or("").to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        Some(Address::Group(groups)) => groups
            .iter()
            .map(|g| {
                let members = g
                    .addresses
                    .iter()
                    .map(|a| a.address.as_deref().unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}: {members}", g.name.as_deref().unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join("; "),
        None => String::new(),
    }
}
