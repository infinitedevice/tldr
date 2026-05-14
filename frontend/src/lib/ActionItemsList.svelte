<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @component ActionItemsList
   * Displays pending action items for a channel and provides
   * Claim / Ignore / Done buttons that PATCH `/api/v1/action-items/{id}`.
   */

  interface ActionItem {
    id: string;
    channel_id: string;
    text: string;
    created_at: number;
    resolved: boolean;
    ignored: boolean;
    claimed: boolean;
  }

  let {
    channelId,
    items,
    onupdate,
  }: {
    channelId: string;
    items: ActionItem[];
    onupdate: () => void;
  } = $props();

  const pending = $derived(items.filter((i) => !i.resolved && !i.ignored));

  async function patchItem(id: string, action: "ignore" | "resolve" | "claim" | "unclaim") {
    await fetch(`/api/v1/action-items/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action }),
    });
    onupdate();
  }
</script>

{#if pending.length > 0}
  <div
    class="mt-3 p-3 bg-yellow-900/30 border border-yellow-700/50 rounded-lg"
    role="note"
    aria-label="Action items for this channel"
  >
    <h3
      class="text-xs font-semibold text-yellow-400 uppercase tracking-wide mb-2"
    >
      Action items
    </h3>
    <ul class="space-y-1.5" aria-live="polite">
      {#each pending as item (item.id)}
        <li class="flex items-start gap-2 text-sm text-yellow-200">
          <span class="mt-0.5 shrink-0" aria-hidden="true">→</span>
          <span class="flex-1">
            {item.text}
            {#if item.claimed}
              <span class="ml-1.5 inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-semibold bg-blue-900/60 text-blue-300 border border-blue-700/50">
                claimed
              </span>
            {/if}
          </span>
          {#if item.claimed}
            <button
              onclick={() => patchItem(item.id, "unclaim")}
              class="shrink-0 text-xs text-blue-400 hover:text-blue-200 transition-colors"
              aria-label="Unclaim this action item: {item.text}"
            >
              Unclaim
            </button>
          {:else}
            <button
              onclick={() => patchItem(item.id, "claim")}
              class="shrink-0 text-xs text-blue-500 hover:text-blue-300 transition-colors"
              aria-label="Claim this action item: {item.text}"
            >
              Claim
            </button>
          {/if}
          <button
            onclick={() => patchItem(item.id, "ignore")}
            class="shrink-0 text-xs text-gray-500 hover:text-gray-300 transition-colors"
            aria-label="Ignore this action item: {item.text}"
          >
            Ignore
          </button>
          <button
            onclick={() => patchItem(item.id, "resolve")}
            class="shrink-0 text-xs text-green-600 hover:text-green-400 transition-colors"
            aria-label="Mark as resolved: {item.text}"
          >
            Done
          </button>
        </li>
      {/each}
    </ul>
  </div>
{/if}
