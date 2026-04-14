<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @page InsightsPage
   * Cross-channel insight synthesis. Queries stored channel summaries in a date
   * range and asks the LLM to produce a structured cross-channel narrative.
   */

  import { onMount, onDestroy } from "svelte";
  import { marked } from "marked";
  import DateRangePicker from "../lib/DateRangePicker.svelte";

  let { userRole = "" } = $props<{ userRole?: string }>();

  interface SeedingStatus {
    seeded: boolean;
    in_progress: boolean;
    total_channels: number;
    completed_channels: number;
  }

  interface ChannelInsight {
    id: string;
    channel_id: string;
    channel_name: string;
    team_name: string;
    summary_text: string;
    action_items: string[];
    topics: string[];
    importance_score: number;
    risk_score: number;
    timestamp_ms: number;
    mention_count: number;
    unread_count: number;
    seeded: boolean;
  }

  interface Synthesis {
    synthesis: string;
    themes: string[];
    important_channels: string[];
    open_questions: string[];
    at_risk_items: string[];
  }

  // Default range: last 7 days
  const now = Date.now();
  let fromMs = $state(now - 7 * 24 * 60 * 60 * 1000);
  let toMs = $state(now);

  let insights: ChannelInsight[] = $state([]);
  let synthesis: Synthesis | null = $state(null);
  let loading = $state(false);
  let error: string | null = $state(null);
  let hasLoaded = $state(false);
  let minMs: number | null = $state(null);

  let noRoleSet = $derived(!userRole);
  let seedingStatus = $state<SeedingStatus | null>(null);
  let seedingPollId: ReturnType<typeof setInterval> | null = null;

  const pct = $derived(
    seedingStatus
      ? Math.round(
          (seedingStatus.completed_channels /
            Math.max(seedingStatus.total_channels, 1)) *
            100,
        )
      : 0,
  );

  async function checkSeedingStatus() {
    try {
      const sr = await fetch("/api/v1/seeding/status");
      if (!sr.ok) return;
      const sd = await sr.json();
      if (sd.seed_from_ms) minMs = sd.seed_from_ms;
      if (sd.seeding) {
        seedingStatus = sd.seeding as SeedingStatus;
        if (!seedingStatus.in_progress && seedingPollId !== null) {
          clearInterval(seedingPollId);
          seedingPollId = null;
          // Seeding just finished — auto-load insights
          fetchInsights();
        }
      }
    } catch {
      /* non-fatal */
    }
  }

  onMount(async () => {
    // Seeding status check (noRoleSet is derived from userRole prop, no fetch needed)

    // Fetch seeding status; start polling if seeding is still in progress
    await checkSeedingStatus();
    if (seedingStatus?.in_progress) {
      seedingPollId = setInterval(checkSeedingStatus, 3000);
    } else {
      // Not seeding — auto-load insights for the default range
      fetchInsights();
    }
  });

  onDestroy(() => {
    if (seedingPollId !== null) clearInterval(seedingPollId);
  });

  async function fetchInsights() {
    loading = true;
    error = null;
    synthesis = null;
    insights = [];
    hasLoaded = false;
    try {
      const url = `/api/v1/insights?from_ms=${fromMs}&to_ms=${toMs}`;
      const resp = await fetch(url);
      const data = await resp.json();
      if (!data.ok) throw new Error(data.error ?? `HTTP ${resp.status}`);
      insights = data.insights ?? [];
      synthesis = data.synthesis ?? null;
      // Float channels flagged by synthesis to the top
      if (synthesis?.important_channels?.length) {
        const important = new Set(
          synthesis.important_channels.map((s: string) => s.toLowerCase()),
        );
        const isImportant = (ins: ChannelInsight) =>
          important.has(ins.channel_name.toLowerCase()) ||
          important.has(`${ins.team_name}/${ins.channel_name}`.toLowerCase());
        insights = [
          ...insights.filter(isImportant),
          ...insights.filter((i) => !isImportant(i)),
        ];
      }
      hasLoaded = true;
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : "Failed to fetch insights";
    } finally {
      loading = false;
    }
  }

  function formatDate(ms: number): string {
    return new Date(ms).toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }
</script>

<div class="max-w-5xl mx-auto px-6 py-6">
  <!-- Controls row -->
  <div class="flex flex-col gap-4 mb-6">
    <DateRangePicker bind:fromMs bind:toMs {minMs} />

    <div class="flex items-center gap-3 flex-wrap">
      <button
        onclick={fetchInsights}
        disabled={loading || seedingStatus?.in_progress}
        class="p-2.5 bg-cyan-600 hover:bg-cyan-500 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg transition-colors cursor-pointer flex items-center justify-center"
        aria-label={seedingStatus?.in_progress
          ? "Seeding history…"
          : loading
            ? "Generating insights…"
            : "Generate insights"}
        title={seedingStatus?.in_progress
          ? "Seeding history…"
          : loading
            ? "Generating insights…"
            : "Generate insights"}
      >
        <svg
          class={`w-5 h-5 ${loading ? "animate-spin" : ""}`}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M21 2v6h-6" />
          <path d="M3 12a9 9 0 0 1 15-6.7L21 8" />
          <path d="M3 22v-6h6" />
          <path d="M21 12a9 9 0 0 1-15 6.7L3 16" />
        </svg>
      </button>
      {#if loading}
        <span class="text-sm text-gray-400 animate-pulse"
          >Synthesising insights…</span
        >
      {/if}
    </div>
  </div>

  <!-- Seeding progress banner -->
  {#if seedingStatus?.in_progress}
    <div
      class="bg-gray-800/60 border border-cyan-800/60 rounded-lg p-4 mb-6"
      data-tour="seeding-progress"
    >
      <p class="text-sm text-cyan-300 font-medium mb-2">
        ⏳ Building historical insights…
      </p>
      <div class="w-full bg-gray-700 rounded-full h-1.5 mb-2">
        <div
          class="bg-cyan-500 h-1.5 rounded-full transition-all duration-500"
          style="width: {pct}%"
        ></div>
      </div>
      <p class="text-xs text-gray-400">
        {seedingStatus.completed_channels} / {seedingStatus.total_channels} channels
        · {pct}%
      </p>
      <p class="text-xs text-gray-500 mt-1">
        Insights will load automatically when complete.
      </p>
    </div>
  {/if}

  <!-- No role hint -->
  {#if noRoleSet && !loading}
    <div
      class="bg-gray-800 border border-cyan-700/40 rounded-xl px-5 py-4 mb-6 flex items-start gap-3"
    >
      <svg
        viewBox="0 0 24 24"
        class="w-4 h-4 shrink-0 mt-0.5 fill-none stroke-cyan-400"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <circle cx="12" cy="12" r="10" /><path d="M12 8v4" /><line
          x1="12"
          y1="16"
          x2="12.01"
          y2="16"
          stroke-width="3"
        />
      </svg>
      <p class="text-sm text-gray-300">
        Set your role in the <strong class="text-cyan-400"
          >avatar menu → Your Role</strong
        > for personalised synthesis.
      </p>
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

  <!-- Empty state: loaded but no data -->
  {#if !loading && hasLoaded && insights.length === 0}
    <div class="text-center mt-24">
      <svg
        viewBox="0 0 24 24"
        class="w-12 h-12 mx-auto mb-4 fill-none stroke-gray-700"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path
          d="M9 18h6M10 22h4M12 2a7 7 0 0 1 7 7c0 2.5-1.3 4.7-3 6l-1 3H9l-1-3C6.3 13.7 5 11.5 5 9a7 7 0 0 1 7-7z"
        />
      </svg>
      <p class="text-gray-500 text-sm">No summaries in this date range.</p>
      <p class="text-gray-600 text-xs mt-1">
        Run <strong class="text-gray-500">Summarise</strong> first to build history,
        then come back here.
      </p>
    </div>
  {/if}

  {#if !loading && !hasLoaded && !error}
    <p class="text-gray-500 text-center mt-24 text-sm">
      Select a date range and click the refresh button to generate insights.
    </p>
  {/if}

  <!-- Synthesis section -->
  {#if synthesis}
    <div class="space-y-6 mb-8">
      <!-- Narrative -->
      <div class="bg-gray-800 border border-gray-700 rounded-xl p-5">
        <h2
          class="text-xs font-semibold uppercase tracking-widest text-cyan-400 mb-3"
        >
          Synthesis
        </h2>
        <p class="text-gray-200 text-sm leading-relaxed">
          {synthesis.synthesis}
        </p>
      </div>

      <!-- 3-col grid -->
      <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
        <!-- Themes -->
        <div class="bg-gray-800 border border-gray-700 rounded-xl p-5">
          <h3
            class="text-xs font-semibold uppercase tracking-widest text-purple-400 mb-3"
          >
            Themes
          </h3>
          {#if synthesis.themes.length > 0}
            <ul class="space-y-1.5">
              {#each synthesis.themes as theme}
                <li class="flex items-start gap-2 text-sm text-gray-300">
                  <span class="text-purple-500 mt-0.5 shrink-0">◆</span>{theme}
                </li>
              {/each}
            </ul>
          {:else}
            <p class="text-gray-600 text-xs">None identified</p>
          {/if}
        </div>

        <!-- Important channels -->
        <div class="bg-gray-800 border border-gray-700 rounded-xl p-5">
          <h3
            class="text-xs font-semibold uppercase tracking-widest text-yellow-400 mb-3"
          >
            Important channels
          </h3>
          {#if synthesis.important_channels.length > 0}
            <ul class="space-y-1.5">
              {#each synthesis.important_channels as ch}
                <li class="text-sm text-gray-300 font-mono text-xs">{ch}</li>
              {/each}
            </ul>
          {:else}
            <p class="text-gray-600 text-xs">None flagged</p>
          {/if}
        </div>

        <!-- Open questions -->
        <div class="bg-gray-800 border border-gray-700 rounded-xl p-5">
          <h3
            class="text-xs font-semibold uppercase tracking-widest text-blue-400 mb-3"
          >
            Open questions
          </h3>
          {#if synthesis.open_questions.length > 0}
            <ul class="space-y-1.5">
              {#each synthesis.open_questions as q}
                <li class="flex items-start gap-2 text-sm text-gray-300">
                  <span class="text-blue-500 mt-0.5 shrink-0">?</span>{q}
                </li>
              {/each}
            </ul>
          {:else}
            <p class="text-gray-600 text-xs">None identified</p>
          {/if}
        </div>
      </div>

      <!-- At-risk items -->
      {#if synthesis.at_risk_items.length > 0}
        <div class="bg-red-950/40 border border-red-800/60 rounded-xl p-5">
          <h3
            class="text-xs font-semibold uppercase tracking-widest text-red-400 mb-3"
          >
            At-risk items
          </h3>
          <ul class="space-y-2">
            {#each synthesis.at_risk_items as item}
              <li class="flex items-start gap-2 text-sm text-red-200">
                <svg
                  viewBox="0 0 24 24"
                  class="w-4 h-4 shrink-0 mt-0.5 fill-none stroke-red-400"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path
                    d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"
                  />
                  <line x1="12" y1="9" x2="12" y2="13" /><line
                    x1="12"
                    y1="17"
                    x2="12.01"
                    y2="17"
                  />
                </svg>
                {item}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Channel-level insights table -->
  {#if insights.length > 0}
    {@const importantSet = new Set(
      (synthesis?.important_channels ?? []).map((s: string) => s.toLowerCase()),
    )}
    <div class="bg-gray-800 border border-gray-700 rounded-xl overflow-hidden">
      <div class="px-5 py-3 border-b border-gray-700">
        <h2
          class="text-xs font-semibold uppercase tracking-widest text-gray-400"
        >
          Channel snapshots — {insights.length} record{insights.length === 1
            ? ""
            : "s"}
        </h2>
      </div>
      <div class="divide-y divide-gray-700/50">
        {#each insights as ins}
          {@const isImportant =
            importantSet.has(ins.channel_name.toLowerCase()) ||
            importantSet.has(
              `${ins.team_name}/${ins.channel_name}`.toLowerCase(),
            )}
          <div
            class={`px-5 py-4 ${isImportant ? "border-l-2 border-amber-500" : ""}`}
          >
            <div class="flex items-center gap-2 mb-2 flex-wrap">
              {#if isImportant}
                <span class="text-amber-400" title="Flagged as important"
                  >⚑</span
                >
              {/if}
              <span class="text-green-400 font-semibold text-sm"
                >#{ins.channel_name}</span
              >
              <span class="text-gray-600 text-xs">· {ins.team_name}</span>
              {#if ins.mention_count > 0}
                <span
                  class="px-1.5 py-0.5 bg-red-900/50 text-red-300 text-xs rounded font-medium"
                  >{ins.mention_count} mentions</span
                >
              {/if}
              {#if ins.unread_count > 0}
                <span
                  class="px-1.5 py-0.5 bg-yellow-900/40 text-yellow-400 text-xs rounded"
                  >{ins.unread_count} unread</span
                >
              {/if}
              {#if ins.seeded}
                <span
                  class="px-1.5 py-0.5 bg-gray-700 text-gray-500 text-xs rounded"
                  >history</span
                >
              {/if}
              <span class="ml-auto text-xs text-gray-600 shrink-0"
                >{formatDate(ins.timestamp_ms)}</span
              >
            </div>
            <div
              class="prose prose-invert prose-xs max-w-none text-gray-400 text-xs leading-relaxed [&>*:first-child]:mt-0 [&>*:last-child]:mb-0 line-clamp-6"
            >
              {@html marked.parse(ins.summary_text)}
            </div>
            {#if ins.topics.length > 0}
              <div class="flex gap-1.5 mt-2 flex-wrap">
                {#each ins.topics as t}
                  <span
                    class="px-2 py-0.5 bg-gray-700 rounded-full text-xs text-gray-300"
                    >{t}</span
                  >
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>
