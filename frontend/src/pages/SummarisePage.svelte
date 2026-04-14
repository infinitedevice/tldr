<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @page SummarisePage
   * Displays cached channel summaries from the daemon and subscribes to SSE
   * for real-time updates as the background summarise loop completes channels.
   */

  import { onMount, onDestroy } from "svelte";
  import TeamSection from "../lib/TeamSection.svelte";

  interface ActionItem {
    id: string;
    channel_id: string;
    text: string;
    created_at: number;
    resolved: boolean;
    ignored: boolean;
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
    action_items: ActionItem[];
    topic?: string;
    participants?: string[];
  }

  let summaries: Summary[] = $state([]);
  let favouriteIds: Set<string> = $state(new Set());
  let loading = $state(true);
  let error: string | null = $state(null);
  let eventSource: EventSource | null = null;

  // Countdown to next background summarise cycle
  let pollIntervalSecs = $state(120);
  let nextUpdateSecs = $state<number | null>(null);
  let countdownTimer: ReturnType<typeof setInterval> | null = null;

  function fmtCountdown(secs: number): string {
    if (secs <= 0) return "Updating\u2026";
    if (secs >= 60) {
      const m = Math.floor(secs / 60);
      const s = secs % 60;
      return s > 0 ? `${m}m ${s}s` : `${m}m`;
    }
    return `${secs}s`;
  }

  function resetCountdown() {
    if (countdownTimer !== null) clearInterval(countdownTimer);
    if (pollIntervalSecs === 0) {
      nextUpdateSecs = null;
      return;
    }
    nextUpdateSecs = pollIntervalSecs;
    countdownTimer = setInterval(() => {
      if (nextUpdateSecs !== null && nextUpdateSecs > 0) {
        nextUpdateSecs -= 1;
      } else if (countdownTimer !== null) {
        clearInterval(countdownTimer);
      }
    }, 1000);
  }

  // Combined team view: summaries grouped by team, favourites first
  const allTeams = $derived(() => {
    const teamMap = new Map<string, { summaries: Summary[] }>();
    for (const s of summaries) {
      if (!teamMap.has(s.team_name))
        teamMap.set(s.team_name, { summaries: [] });
      teamMap.get(s.team_name)!.summaries.push(s);
    }
    // Within each team: favourites first, then by mention count descending
    for (const [, data] of teamMap) {
      data.summaries.sort((a, b) => {
        const af = favouriteIds.has(a.channel_id) ? 0 : 1;
        const bf = favouriteIds.has(b.channel_id) ? 0 : 1;
        if (af !== bf) return af - bf;
        return b.mention_count - a.mention_count;
      });
    }
    return Array.from(teamMap.entries()).map(([name, data]) => ({
      name,
      ...data,
    }));
  });

  const allTeamNames = $derived(allTeams().map((t) => t.name));
  const multiTeam = $derived(allTeamNames.length > 1);

  function mergeSummary(incoming: Summary) {
    const idx = summaries.findIndex(
      (s) => s.channel_id === incoming.channel_id,
    );
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
    const es = new EventSource("/api/v1/summaries/subscribe");
    es.onmessage = (ev) => {
      try {
        const s = JSON.parse(ev.data) as Summary;
        mergeSummary(s);
        // A new summary just arrived — the cycle completed, reset the clock
        resetCountdown();
      } catch {
        /* ignore malformed */
      }
    };
    eventSource = es;
  }

  onMount(async () => {
    loading = true;
    error = null;

    // Load cached summaries, favourites, and poll interval in parallel
    await Promise.allSettled([
      fetch("/api/v1/health")
        .then((r) => (r.ok ? r.json() : null))
        .then((d: { poll_interval_secs?: number } | null) => {
          pollIntervalSecs = d?.poll_interval_secs ?? 120;
        }),
      fetch("/api/v1/summaries")
        .then((r) => (r.ok ? r.json() : null))
        .then((d: { summaries?: Summary[] } | null) => {
          if (d?.summaries) {
            summaries = d.summaries.sort(
              (a: Summary, b: Summary) => b.mention_count - a.mention_count,
            );
          }
        }),
      fetch("/api/v1/channels/categories")
        .then((r) => (r.ok ? r.json() : null))
        .then(
          (
            d: {
              categories?: Array<{ type: string; channel_ids?: string[] }>;
            } | null,
          ) => {
            if (d?.categories) {
              const ids = new Set<string>();
              for (const cat of d.categories) {
                if (cat.type === "favorites") {
                  for (const id of cat.channel_ids ?? []) ids.add(id);
                }
              }
              favouriteIds = ids;
            }
          },
        ),
      fetch("/api/v1/favourites")
        .then((r) => (r.ok ? r.json() : null))
        .then((d: { favourites?: Array<{ channel_id: string }> } | null) => {
          if (d?.favourites) {
            const ids = new Set(favouriteIds);
            for (const f of d.favourites) ids.add(f.channel_id);
            favouriteIds = ids;
          }
        }),
    ]);

    loading = false;
    resetCountdown();

    // Subscribe to real-time updates
    connectSSE();
  });

  onDestroy(() => {
    eventSource?.close();
    eventSource = null;
    if (countdownTimer !== null) clearInterval(countdownTimer);
  });

  function onMarkRead(channelId: string) {
    summaries = summaries.filter((s) => s.channel_id !== channelId);
  }

  async function onToggleFavourite(
    channelId: string,
    channelName: string,
    teamName: string,
  ) {
    const isFav = favouriteIds.has(channelId);
    const next = new Set(favouriteIds);
    if (isFav) {
      next.delete(channelId);
      favouriteIds = next;
      await fetch(`/api/v1/favourites/${encodeURIComponent(channelId)}`, {
        method: "DELETE",
      });
    } else {
      next.add(channelId);
      favouriteIds = next;
      await fetch(`/api/v1/favourites/${encodeURIComponent(channelId)}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          channel_name: channelName,
          team_name: teamName,
        }),
      });
    }
  }

  function onActionItemUpdate() {
    // Re-fetch cached summaries to reflect action item state changes
    fetch("/api/v1/summaries")
      .then((r) => (r.ok ? r.json() : null))
      .then((d: { summaries?: Summary[] } | null) => {
        if (d?.summaries) {
          summaries = d.summaries.sort(
            (a: Summary, b: Summary) => b.mention_count - a.mention_count,
          );
        }
      })
      .catch(() => {});
  }

  function scrollTo(id: string) {
    document.getElementById(id)?.scrollIntoView({ behavior: "smooth" });
  }
</script>

<!-- Team jump nav when multiple teams -->
{#if multiTeam && summaries.length > 0}
  <nav
    class="flex gap-4 overflow-x-auto text-sm px-6 py-2 border-b border-gray-800"
    aria-label="Jump to team"
  >
    {#each allTeamNames as name}
      <button
        onclick={() => scrollTo("team-" + name)}
        class="text-gray-400 hover:text-cyan-300 whitespace-nowrap transition-colors cursor-pointer"
      >
        {name}
      </button>
    {/each}
  </nav>
{/if}

<div class="max-w-5xl mx-auto px-6 py-6">
  <!-- Next-update countdown -->
  {#if summaries.length > 0 && pollIntervalSecs > 0 && nextUpdateSecs !== null}
    <div class="flex items-center gap-2 mb-4">
      <span class="text-xs text-gray-500">
        {nextUpdateSecs > 0
          ? `Next update in ${fmtCountdown(nextUpdateSecs)}`
          : "Updating\u2026"}
      </span>
    </div>
  {/if}

  {#if error}
    <div
      class="bg-red-900/50 border border-red-500 text-red-200 rounded-lg p-4 mb-6"
      role="alert"
    >
      {error}
    </div>
  {/if}

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
      <p class="text-gray-400 text-sm">Loading summaries…</p>
    </div>
  {:else if summaries.length === 0 && !error}
    <div class="flex flex-col items-center justify-center py-32 text-center">
      <!-- Inbox icon -->
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
        <polyline points="22 12 16 12 14 15 10 15 8 12 2 12" />
        <path
          d="M5.45 5.11L2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"
        />
      </svg>
      <h2 class="text-2xl font-semibold text-gray-300 mb-2">
        Waiting for first summary cycle
      </h2>
      <p class="text-gray-500 max-w-sm">
        The daemon summarises unread channels automatically. Summaries will
        appear here as they are generated.
      </p>
      {#if pollIntervalSecs > 0 && nextUpdateSecs !== null}
        <span
          class="mt-4 inline-flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-full bg-gray-800 text-gray-400"
        >
          First update in {fmtCountdown(nextUpdateSecs)}
        </span>
      {/if}
    </div>
  {/if}

  {#each allTeams() as team}
    <section
      id={"team-" + team.name}
      class="mt-2"
      aria-labelledby={"heading-" + team.name}
    >
      {#if multiTeam}
        <h2
          id={"heading-" + team.name}
          class="text-lg font-semibold text-cyan-400 mb-4 border-b border-gray-700 pb-2"
        >
          {team.name}
        </h2>
      {/if}
      <TeamSection
        summaries={team.summaries}
        {favouriteIds}
        {onActionItemUpdate}
        {onMarkRead}
        {onToggleFavourite}
      />
    </section>
  {/each}
</div>
