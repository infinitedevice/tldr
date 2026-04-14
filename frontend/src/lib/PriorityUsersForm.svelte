<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @component PriorityUsersForm
   * Chip-list UI for managing priority users.  Calls `onsave(users)` when the
   * user clicks Save; the parent is responsible for persisting via the config API.
   */

  let {
    users = $bindable([]),
    onsave,
  }: {
    users: string[];
    onsave: (users: string[]) => Promise<void>;
  } = $props();

  let input = $state("");
  let saving = $state(false);
  let error = $state("");

  function add() {
    const username = input.trim().replace(/^@/, "");
    if (!username) return;
    if (!users.includes(username)) {
      users = [...users, username];
    }
    input = "";
  }

  function remove(username: string) {
    users = users.filter((u) => u !== username);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  }

  async function save() {
    saving = true;
    error = "";
    try {
      await onsave(users);
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : "Failed to save";
    } finally {
      saving = false;
    }
  }
</script>

<div class="space-y-3">
  <p class="text-xs text-gray-400">
    Messages from these users will be highlighted in summaries. Enter Mattermost
    usernames (without @).
  </p>

  <div class="flex gap-2">
    <input
      type="text"
      placeholder="username"
      bind:value={input}
      onkeydown={handleKeyDown}
      class="flex-1 bg-gray-800 border border-gray-600 rounded px-3 py-1.5 text-sm text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
      aria-label="Priority username to add"
    />
    <button
      onclick={add}
      class="px-3 py-1.5 bg-gray-700 hover:bg-gray-600 rounded text-sm text-white transition-colors"
    >
      Add
    </button>
  </div>

  {#if users.length > 0}
    <div class="flex flex-wrap gap-2" aria-label="Priority users">
      {#each users as username}
        <span
          class="flex items-center gap-1 bg-cyan-900/50 text-cyan-300 border border-cyan-700 rounded-full px-2.5 py-0.5 text-xs"
        >
          @{username}
          <button
            onclick={() => remove(username)}
            class="text-cyan-500 hover:text-white transition-colors leading-none"
            aria-label="Remove @{username}">✕</button
          >
        </span>
      {/each}
    </div>
  {:else}
    <p class="text-xs text-gray-500 italic">No priority users set.</p>
  {/if}

  {#if error}
    <p class="text-xs text-red-400">{error}</p>
  {/if}

  <button
    onclick={save}
    disabled={saving}
    class="px-4 py-1.5 bg-cyan-600 hover:bg-cyan-500 disabled:opacity-50 rounded text-sm font-medium text-white transition-colors"
  >
    {saving ? "Saving…" : "Save priority users"}
  </button>
</div>
