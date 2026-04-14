<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: AGPL-3.0-or-later

  /**
   * @component TeamSection
   * Renders channel summary cards grouped by team.
   * Exposes `onMarkRead(channelId)` and `onActionItemUpdate()` callbacks
   * so the parent can refresh state without a full re-fetch.
   */

  import ActionItemsList from './ActionItemsList.svelte';

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
    topics?: TopicSection[];
    action_items: ActionItem[];
    topic?: string;
    participants?: string[];
  }

  interface TopicSection {
    title: string;
    summary_html: string;
  }

  let {
    summaries,
    favouriteIds = new Set<string>(),
    onActionItemUpdate,
    onMarkRead,
    onToggleFavourite,
  }: {
    summaries: Summary[];
    favouriteIds?: Set<string>;
    onActionItemUpdate: () => void;
    onMarkRead: (channelId: string) => void;
    onToggleFavourite?: (channelId: string, channelName: string, teamName: string) => void;
  } = $props();

  async function markRead(channelId: string) {
    try {
      await fetch(`/api/v1/channels/${encodeURIComponent(channelId)}/read`, { method: 'POST' });
      onMarkRead(channelId);
    } catch {
      // Non-fatal: watermark will catch up on next run
    }
  }
</script>

<div class="space-y-8">
  {#each summaries as s}
    <article      data-tour="channel-card"      class="bg-gray-800 rounded-lg border border-gray-700 p-5"
      aria-labelledby="channel-{s.channel_name}"
    >
      <div class="flex items-start gap-3 mb-3 flex-wrap">
        <!-- Channel name, linked if we have a URL -->
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-1.5">
            {#if s.channel_url}
              <a
                href={s.channel_url}
                target="_blank"
                rel="noopener noreferrer"
                class="text-green-400 font-bold hover:underline text-base"
                id="channel-{s.channel_name}"
              >
                #{s.channel_name}
              </a>
            {:else}
              <span class="text-green-400 font-bold text-base" id="channel-{s.channel_name}">
                #{s.channel_name}
              </span>
            {/if}
            <button
              onclick={() => onToggleFavourite?.(s.channel_id, s.channel_name, s.team_name)}
              class="text-xl leading-none transition-colors {favouriteIds.has(s.channel_id) ? 'text-yellow-400 hover:text-yellow-200' : 'text-gray-600 hover:text-yellow-400'}"
              title={favouriteIds.has(s.channel_id) ? 'Remove from favourites' : 'Add to favourites'}
              aria-label={favouriteIds.has(s.channel_id) ? `Remove #${s.channel_name} from favourites` : `Add #${s.channel_name} to favourites`}
            >★</button>
          </div>

          <!-- DM / group topic subtitle -->
          {#if s.topic}
            <p class="text-xs text-gray-400 mt-0.5 italic">{s.topic}</p>
          {/if}

          <!-- DM / group participants -->
          {#if s.participants && s.participants.length > 0}
            <p class="text-xs text-gray-500 mt-0.5">
              with {s.participants.join(', ')}
            </p>
          {/if}
        </div>

        <div class="flex items-center gap-2 shrink-0 flex-wrap">
          <span class="text-xs text-gray-400">
            {#if s.unread_count > 0}
              <span class="text-yellow-400">{s.unread_count} unread</span>
            {/if}
            {#if s.mention_count > 0}
              {#if s.unread_count > 0}&nbsp;·&nbsp;{/if}
              <span class="text-red-400 font-semibold">{s.mention_count} mentions</span>
            {/if}
          </span>

          <button
            data-tour="mark-read"
            onclick={() => markRead(s.channel_id)}
            class="text-xs text-gray-500 hover:text-gray-300 border border-gray-600 hover:border-gray-400 rounded px-2 py-0.5 transition-colors"
            aria-label="Mark #{s.channel_name} as read"
            id="mark-read-{s.channel_name}"
          >
            Mark as read
          </button>
        </div>
      </div>

      <!-- Topic sections or fallback prose -->
      <div data-tour="summary-html" class="space-y-3">
        {#if s.topics && s.topics.length > 0}
          {#each s.topics as topic, i}
            <div class={i > 0 ? 'border-t border-gray-700/50 pt-3' : ''}>
              <h4 class="text-sm font-semibold text-gray-300 mb-1">{topic.title}</h4>
              <div class="prose prose-invert prose-sm max-w-none text-gray-200">
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

      <!-- Action items for this channel -->
      {#if s.action_items?.length > 0}
        <div data-tour="action-items">
          <ActionItemsList
            channelId={s.channel_id}
            items={s.action_items}
            onupdate={onActionItemUpdate}
          />
        </div>
      {/if}
    </article>
  {/each}
</div>

