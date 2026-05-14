<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @page ActionsPage
   * Two-section view for action items:
   * - Top: claimed items (flat list grouped by channel name)
   * - Bottom: channel cards that have pending action items
   */

  import { onMount } from "svelte";
  import ActionItemsList from "../lib/ActionItemsList.svelte";

  interface ActionItem {
    id: string;
    channel_id: string;
    text: string;
    created_at: number;
    resolved: boolean;
    ignored: boolean;
    claimed: boolean;
  }

  interface TopicSection {
    title: string;
    summary_html: string;
  }

  interface Summary {
    team_name: string;
    channel_name: string;
    channel_id: string;
    channel_url: string;
    unread_count: number;
    mention_count: number;
    summary: string;
    summary_html: string;
    topics?: TopicSection[];
    action_items: ActionItem[];
    topic?: string;
    participants?: string[];
  }

  let summaries: Summary[] = $state([]);
  let allItems: ActionItem[] = $state([]);
  let loading = $state(true);

  // Summaries that have at least one pending (not resolved, not ignored) action item
  const summariesWithActions = $derived(
    summaries.filter((s) =>
      s.action_items.some((i) => !i.resolved && !i.ignored),
    ),
  );

  // Claimed items from the global action items list, grouped by channel
  const claimedByChannel = $derived(() => {
    const claimed = allItems.filter(
      (i) => i.claimed && !i.resolved && !i.ignored,
    );
    const map = new Map<string, { channel_id: string; channel_name: string; items: ActionItem[] }>();
    for (const item of claimed) {
      const s = summaries.find((s) => s.channel_id === item.channel_id);
      const name = s?.channel_name ?? item.channel_id;
      if (!map.has(item.channel_id)) {
        map.set(item.channel_id, { channel_id: item.channel_id, channel_name: name, items: [] });
      }
      map.get(item.channel_id)!.items.push(item);
    }
    return Array.from(map.values());
  });

  async function loadData() {
    loading = true;
    await refreshData();
    loading = false;
  }

  async function refreshData() {
    await Promise.allSettled([
      fetch("/api/v1/summaries")
        .then((r) => (r.ok ? r.json() : null))
        .then((d: { summaries?: Summary[] } | null) => {
          if (d?.summaries) {
            // Merge action_items in-place to avoid re-ordering channels
            const freshMap = new Map(
              d.summaries.map((s: Summary) => [s.channel_id, s.action_items]),
            );
            summaries = summaries.map((s) => {
              const freshItems = freshMap.get(s.channel_id);
              return freshItems !== undefined
                ? { ...s, action_items: freshItems }
                : s;
            });
            // Add any newly appeared channels (shouldn't happen often in Actions view)
            const existing = new Set(summaries.map((s) => s.channel_id));
            for (const s of d.summaries) {
              if (!existing.has(s.channel_id)) summaries = [...summaries, s];
            }
          }
        }),
      fetch("/api/v1/action-items")
        .then((r) => (r.ok ? r.json() : null))
        .then((d: { items?: ActionItem[] } | null) => {
          if (d?.items) allItems = d.items;
        }),
    ]);
  }

  onMount(loadData);

  async function patchItem(id: string, action: "ignore" | "resolve" | "claim" | "unclaim") {
    await fetch(`/api/v1/action-items/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action }),
    });
    await refreshData();
  }
</script>

<div class="max-w-5xl mx-auto px-6 py-6">
  {#if loading}
    <div class="flex items-center justify-center py-32">
      <svg
        class="w-8 h-8 text-cyan-400 animate-spin"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        aria-hidden="true"
      >
        <path d="M21 2v6h-6" />
        <path d="M3 12a9 9 0 0 1 15-6.7L21 8" />
        <path d="M3 22v-6h6" />
        <path d="M21 12a9 9 0 0 1-15 6.7L3 16" />
      </svg>
    </div>
  {:else}
    <!-- ── Claimed section ──────────────────────────────────────────── -->
    <section class="mb-8" aria-labelledby="claimed-heading">
      <h2
        id="claimed-heading"
        class="text-sm font-semibold text-blue-400 uppercase tracking-widest mb-3 flex items-center gap-2"
      >
        <!-- Bookmark icon -->
        <svg viewBox="0 0 24 24" class="w-4 h-4 fill-none stroke-current" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" />
        </svg>
        Claimed by me
      </h2>

      {#if claimedByChannel().length === 0}
        <p class="text-sm text-gray-500 italic">
          No claimed items yet. Click <span class="text-blue-400">Claim</span> on any action item to take ownership.
        </p>
      {:else}
        <div class="space-y-3">
          {#each claimedByChannel() as group}
            <div class="bg-blue-950/30 border border-blue-800/40 rounded-lg p-4">
              <h3 class="text-xs font-semibold text-blue-300 mb-2">
                #{group.channel_name}
              </h3>
              <ul class="space-y-2">
                {#each group.items as item (item.id)}
                  <li class="flex items-start gap-2 text-sm text-blue-100">
                    <span class="mt-0.5 shrink-0 text-blue-400" aria-hidden="true">→</span>
                    <span class="flex-1">{item.text}</span>
                    <button
                      onclick={() => patchItem(item.id, "unclaim")}
                      class="shrink-0 text-xs text-blue-400 hover:text-blue-200 transition-colors"
                      aria-label="Unclaim: {item.text}"
                    >Unclaim</button>
                    <button
                      onclick={() => patchItem(item.id, "ignore")}
                      class="shrink-0 text-xs text-gray-500 hover:text-gray-300 transition-colors"
                      aria-label="Ignore: {item.text}"
                    >Ignore</button>
                    <button
                      onclick={() => patchItem(item.id, "resolve")}
                      class="shrink-0 text-xs text-green-600 hover:text-green-400 transition-colors"
                      aria-label="Done: {item.text}"
                    >Done</button>
                  </li>
                {/each}
              </ul>
            </div>
          {/each}
        </div>
      {/if}
    </section>

    <hr class="border-gray-800 mb-8" />

    <!-- ── All pending action items by channel ──────────────────────── -->
    <section aria-labelledby="pending-heading">
      <h2
        id="pending-heading"
        class="text-sm font-semibold text-yellow-400 uppercase tracking-widest mb-3 flex items-center gap-2"
      >
        <!-- Checklist icon -->
        <svg viewBox="0 0 24 24" class="w-4 h-4 fill-none stroke-current" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="9 11 12 14 22 4" />
          <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
        </svg>
        Pending action items
      </h2>

      {#if summariesWithActions.length === 0}
        <p class="text-sm text-gray-500 italic">No pending action items across any channel.</p>
      {:else}
        <div class="space-y-4">
          {#each summariesWithActions as s}
            <article
              class="bg-gray-800 rounded-lg border border-gray-700 p-4"
              aria-labelledby="actions-channel-{s.channel_id}"
            >
              <div class="flex items-center gap-2 mb-2">
                {#if s.channel_url}
                  <a
                    href={s.channel_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-green-400 font-bold hover:underline text-sm"
                    id="actions-channel-{s.channel_id}"
                  >
                    #{s.channel_name}
                  </a>
                {:else}
                  <span
                    class="text-green-400 font-bold text-sm"
                    id="actions-channel-{s.channel_id}"
                  >
                    #{s.channel_name}
                  </span>
                {/if}
                <span class="text-xs text-gray-500">{s.team_name}</span>
              </div>

              <ActionItemsList
                channelId={s.channel_id}
                items={s.action_items}
                onupdate={refreshData}
              />
            </article>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</div>
