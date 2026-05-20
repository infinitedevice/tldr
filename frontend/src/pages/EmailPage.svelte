<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @page EmailPage
   * Displays summarised email mailboxes fetched from the daemon's IMAP integration.
   * Subscribes to SSE for live updates as the background email summarise loop completes.
   */

  import { onMount, onDestroy } from "svelte";
  import ActionItemsList from "../lib/ActionItemsList.svelte";

  interface ActionItem {
    id: string;
    channel_id: string;
    text: string;
    created_at: number;
    resolved: boolean;
    ignored: boolean;
    claimed: boolean;
    source: string;
  }

  interface TopicSection {
    title: string;
    summary_html: string;
  }

  interface EmailMeta {
    subject: string;
    from: string;
    date: string;
  }

  interface EmailSummary {
    mailbox: string;
    mailbox_id: string;
    unread_count: number;
    summary: string;
    summary_html: string;
    topics?: TopicSection[];
    action_items: ActionItem[];
    emails: EmailMeta[];
  }

  let summaries: EmailSummary[] = $state([]);
  // Mailboxes the user has dismissed — SSE must not re-inject these
  let dismissedMailboxes: Set<string> = $state(new Set());
  let loading = $state(true);
  let error: string | null = $state(null);
  let eventSource: EventSource | null = null;

  // Which mailbox card is in "confirm disable" mode
  let confirmingDisableMailbox: string | null = $state(null);

  function mergeSummary(incoming: EmailSummary) {
    if (dismissedMailboxes.has(incoming.mailbox)) return;
    const idx = summaries.findIndex((s) => s.mailbox === incoming.mailbox);
    if (idx >= 0) {
      summaries = [
        ...summaries.slice(0, idx),
        incoming,
        ...summaries.slice(idx + 1),
      ];
    } else {
      summaries = [...summaries, incoming];
    }
  }

  function connectSSE() {
    if (eventSource) eventSource.close();
    const es = new EventSource("/api/v1/email/summaries/subscribe");
    es.onmessage = (ev) => {
      try {
        const s = JSON.parse(ev.data) as EmailSummary;
        mergeSummary(s);
      } catch {
        /* ignore malformed */
      }
    };
    eventSource = es;
  }

  onMount(async () => {
    loading = true;
    error = null;
    try {
      const resp = await fetch("/api/v1/email/summaries");
      if (resp.ok) {
        const d = await resp.json() as { summaries?: EmailSummary[] };
        if (d?.summaries) {
          summaries = d.summaries.sort(
            (a, b) => b.unread_count - a.unread_count,
          );
        }
      }
    } catch (e) {
      error = "Failed to load email summaries.";
    }
    loading = false;
    connectSSE();
  });

  onDestroy(() => {
    eventSource?.close();
    eventSource = null;
  });

  async function markRead(mailbox: string) {
    dismissedMailboxes = new Set([...dismissedMailboxes, mailbox]);
    summaries = summaries.filter((s) => s.mailbox !== mailbox);
    try {
      await fetch(
        `/api/v1/email/${encodeURIComponent(mailbox)}/read`,
        { method: "POST" },
      );
    } catch {
      // Non-fatal
    }
  }

  async function disableMailbox(mailbox: string) {
    confirmingDisableMailbox = null;
    dismissedMailboxes = new Set([...dismissedMailboxes, mailbox]);
    summaries = summaries.filter((s) => s.mailbox !== mailbox);
    await fetch("/api/v1/config/sources", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        email: { mailboxes: [{ name: mailbox, enabled: false }] },
      }),
    }).catch(() => {});
  }

  function onActionItemUpdate(mailboxId: string) {
    fetch("/api/v1/email/summaries")
      .then((r) => (r.ok ? r.json() : null))
      .then((d: { summaries?: EmailSummary[] } | null) => {
        if (!d?.summaries) return;
        const freshMap = new Map(
          d.summaries.map((s: EmailSummary) => [s.mailbox, s.action_items]),
        );
        summaries = summaries.map((s) => {
          const freshItems = freshMap.get(s.mailbox);
          return freshItems !== undefined
            ? { ...s, action_items: freshItems }
            : s;
        });
      })
      .catch(() => {});
  }
</script>

<div class="max-w-5xl mx-auto px-6 py-6">
  {#if loading}
    <div class="flex flex-col items-center justify-center py-32 text-center">
      <svg
        class="w-10 h-10 text-cyan-400 animate-spin mb-4"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M21 2v6h-6" />
        <path d="M3 12a9 9 0 0 1 15-6.7L21 8" />
        <path d="M3 22v-6h6" />
        <path d="M21 12a9 9 0 0 1-15 6.7L3 16" />
      </svg>
      <p class="text-gray-400 text-sm">Loading email summaries…</p>
    </div>
  {:else if error}
    <div
      class="bg-red-900/50 border border-red-500 text-red-200 rounded-lg p-4"
      role="alert"
    >
      {error}
    </div>
  {:else if summaries.length === 0}
    <div class="flex flex-col items-center justify-center py-32 text-center">
      <!-- Envelope icon -->
      <svg
        class="w-16 h-16 text-gray-600 mb-6"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <rect x="2" y="4" width="20" height="16" rx="2" />
        <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
      </svg>
      <h2 class="text-2xl font-semibold text-gray-300 mb-2">
        No email summaries yet
      </h2>
      <p class="text-gray-500 max-w-sm">
        The daemon will summarise unread emails from your configured mailboxes
        on the next background cycle.
      </p>
    </div>
  {:else}
    <div class="space-y-8">
      {#each summaries as s (s.mailbox)}
        <article
          class="bg-gray-800 rounded-lg border border-gray-700 p-5"
          aria-labelledby="mailbox-{s.mailbox_id}"
        >
          <!-- Header -->
          <div class="flex items-start gap-3 mb-3 flex-wrap">
            <div class="flex-1 min-w-0 flex items-center gap-2">
              <!-- Envelope icon -->
              <svg
                class="w-4 h-4 text-cyan-400 shrink-0"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <rect x="2" y="4" width="20" height="16" rx="2" />
                <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
              </svg>
              <h2
                class="text-green-400 font-bold text-base"
                id="mailbox-{s.mailbox_id}"
              >
                {s.mailbox}
              </h2>
            </div>

            <div class="flex items-center gap-2 shrink-0">
              {#if s.unread_count > 0}
                <span class="text-xs text-yellow-400"
                  >{s.unread_count} unread</span
                >
              {/if}
              <button
                onclick={() => markRead(s.mailbox)}
                class="text-xs text-gray-500 hover:text-gray-300 border border-gray-600 hover:border-gray-400 rounded px-2 py-0.5 transition-colors"
                aria-label="Mark {s.mailbox} as read"
              >
                Mark as read
              </button>
              <!-- Disable mailbox / inline confirm -->
              {#if confirmingDisableMailbox === s.mailbox}
                <span class="flex items-center gap-1 text-xs">
                  <span class="text-gray-400">Disable?</span>
                  <button
                    onclick={() => disableMailbox(s.mailbox)}
                    class="text-red-400 hover:text-red-300 font-medium transition-colors"
                  >Yes</button>
                  <span class="text-gray-600">/</span>
                  <button
                    onclick={() => (confirmingDisableMailbox = null)}
                    class="text-gray-400 hover:text-gray-200 transition-colors"
                  >No</button>
                </span>
              {:else}
                <button
                  onclick={() => (confirmingDisableMailbox = s.mailbox)}
                  class="text-gray-600 hover:text-red-400 transition-colors"
                  title="Disable {s.mailbox}"
                  aria-label="Disable {s.mailbox}"
                >
                  <svg viewBox="0 0 24 24" class="w-3.5 h-3.5 fill-none stroke-current" stroke-width="2" stroke-linecap="round">
                    <circle cx="12" cy="12" r="10"/>
                    <line x1="4.93" y1="4.93" x2="19.07" y2="19.07"/>
                  </svg>
                </button>
              {/if}
            </div>
          </div>

          <!-- Summary topics -->
          <div class="space-y-3 mb-4">
            {#if s.topics && s.topics.length > 0}
              {#each s.topics as topic, i}
                <div class={i > 0 ? "border-t border-gray-700/50 pt-3" : ""}>
                  <h4 class="text-sm font-semibold text-gray-300 mb-1">
                    {topic.title}
                  </h4>
                  <div
                    class="prose prose-invert prose-sm max-w-none text-gray-200"
                  >
                    {@html topic.summary_html}
                  </div>
                </div>
              {/each}
            {:else}
              <div class="prose prose-invert prose-sm max-w-none text-gray-200">
                {@html s.summary_html}
              </div>
            {/if}
          </div>

          <!-- Email list -->
          {#if s.emails && s.emails.length > 0}
            <details class="mb-3">
              <summary
                class="text-xs text-gray-500 hover:text-gray-300 cursor-pointer select-none"
              >
                {s.emails.length} email{s.emails.length === 1 ? "" : "s"} included
              </summary>
              <ul class="mt-2 space-y-1 pl-2 border-l border-gray-700">
                {#each s.emails as email}
                  <li class="text-xs text-gray-400">
                    <span class="text-gray-300">{email.subject}</span>
                    <span class="text-gray-600 mx-1">·</span>
                    <span class="text-gray-500">{email.from}</span>
                    <span class="text-gray-600 mx-1">·</span>
                    <span class="text-gray-600">{email.date}</span>
                  </li>
                {/each}
              </ul>
            </details>
          {/if}

          <!-- Action items -->
          {#if s.action_items?.length > 0}
            <div>
              <ActionItemsList
                channelId={s.mailbox}
                items={s.action_items}
                onupdate={() => onActionItemUpdate(s.mailbox)}
              />
            </div>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</div>
