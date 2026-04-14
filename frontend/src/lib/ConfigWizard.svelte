<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @component ConfigWizard
   * Multi-step setup wizard shown automatically when the daemon reports
   * `configured: false`.  Collects Mattermost server URL, API token, and
   * LLM settings, then writes them to `/api/v1/config`.
   */

  let { oncomplete }: { oncomplete: () => void } = $props();
  let dialogEl: HTMLDivElement | undefined = $state();

  // Trap focus within the dialog
  $effect(() => {
    if (!dialogEl) return;
    const first = dialogEl.querySelector<HTMLElement>(
      'button, input, select, textarea, [tabindex]:not([tabindex="-1"])',
    );
    first?.focus();

    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Tab") return;
      const focusable = Array.from(
        dialogEl!.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input, select, textarea, [tabindex]:not([tabindex="-1"])',
        ),
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey) {
        if (document.activeElement === first) {
          e.preventDefault();
          last.focus();
        }
      } else {
        if (document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  });

  let step = $state(1);

  // Step 1: Mattermost
  let mmUrl = $state("");
  let mmToken = $state("");

  // Step 2: LLM
  let llmUrl = $state("");
  let llmModel = $state("");
  let llmToken = $state("");

  // Step 3: Review / save
  let saving = $state(false);
  let saveError = $state("");

  async function save() {
    saving = true;
    saveError = "";
    try {
      const config = {
        mattermost: { server_url: mmUrl.trim(), token: mmToken.trim() },
        llm: {
          base_url: llmUrl.trim(),
          model: llmModel.trim(),
          bearer_token: llmToken.trim() || null,
        },
        paths: {},
        server: {},
      };
      const resp = await fetch("/api/v1/config", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(config),
      });
      const data = await resp.json();
      if (data.ok) {
        oncomplete();
      } else {
        saveError = data.error || "Failed to save config";
      }
    } catch (e: unknown) {
      saveError = e instanceof Error ? e.message : "Network error";
    } finally {
      saving = false;
    }
  }

  function next() {
    step++;
  }
  function back() {
    step--;
  }

  const step1Valid = $derived(
    mmUrl.trim().length > 0 && mmToken.trim().length > 0,
  );
  const step2Valid = $derived(
    llmUrl.trim().length > 0 && llmModel.trim().length > 0,
  );
</script>

<!-- Modal backdrop -->
<div
  bind:this={dialogEl}
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/70"
  role="dialog"
  aria-modal="true"
  aria-label="Setup wizard"
>
  <div
    class="bg-gray-900 border border-gray-700 rounded-xl shadow-2xl w-full max-w-lg mx-4 p-8"
  >
    <!-- Progress indicators -->
    <div class="flex gap-2 mb-6" aria-label="Wizard progress">
      {#each [1, 2, 3] as s}
        <div
          class="h-1.5 flex-1 rounded-full {s <= step
            ? 'bg-cyan-500'
            : 'bg-gray-700'}"
          aria-current={s === step ? "step" : undefined}
        ></div>
      {/each}
    </div>

    {#if step === 1}
      <h2 class="text-xl font-semibold text-white mb-1">
        Mattermost connection
      </h2>
      <p class="text-sm text-gray-400 mb-6">
        Enter your Mattermost server URL and access token.
      </p>

      <label class="block mb-4">
        <span class="text-sm text-gray-300 block mb-1">Server URL</span>
        <input
          type="url"
          placeholder="https://mattermost.example.com"
          bind:value={mmUrl}
          class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
          aria-required="true"
        />
      </label>

      <label class="block mb-6">
        <span class="text-sm text-gray-300 block mb-1">Access token</span>
        <input
          type="password"
          placeholder="your-mattermost-token"
          bind:value={mmToken}
          class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
          aria-required="true"
        />
        <p class="text-xs text-gray-500 mt-1">
          Generate in Mattermost → Profile → Security → Personal Access Tokens.
        </p>
      </label>

      <div class="flex justify-end">
        <button
          onclick={next}
          disabled={!step1Valid}
          class="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 rounded font-medium text-sm transition-colors"
        >
          Next →
        </button>
      </div>
    {:else if step === 2}
      <h2 class="text-xl font-semibold text-white mb-1">LLM configuration</h2>
      <p class="text-sm text-gray-400 mb-6">
        Configure the language model used to generate summaries.
      </p>

      <label class="block mb-4">
        <span class="text-sm text-gray-300 block mb-1">API base URL</span>
        <input
          type="url"
          bind:value={llmUrl}
          class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white focus:outline-none focus:border-cyan-500"
          aria-required="true"
        />
      </label>

      <label class="block mb-4">
        <span class="text-sm text-gray-300 block mb-1">Model name</span>
        <input
          type="text"
          bind:value={llmModel}
          class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white focus:outline-none focus:border-cyan-500"
          aria-required="true"
        />
      </label>

      <label class="block mb-6">
        <span class="text-sm text-gray-300 block mb-1"
          >API key <span class="text-gray-500">(optional)</span></span
        >
        <input
          type="password"
          placeholder="leave blank if not required"
          bind:value={llmToken}
          class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
        />
      </label>

      <div class="flex justify-between">
        <button
          onclick={back}
          class="px-5 py-2 bg-gray-700 hover:bg-gray-600 rounded font-medium text-sm transition-colors"
        >
          ← Back
        </button>
        <button
          onclick={next}
          disabled={!step2Valid}
          class="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 rounded font-medium text-sm transition-colors"
        >
          Next →
        </button>
      </div>
    {:else}
      <h2 class="text-xl font-semibold text-white mb-1">Ready to save</h2>
      <p class="text-sm text-gray-400 mb-6">
        Review your configuration, then click Save.
      </p>

      <dl class="text-sm space-y-2 mb-6">
        <div class="flex gap-2">
          <dt class="text-gray-400 w-28 shrink-0">MM Server</dt>
          <dd class="text-white break-all">{mmUrl}</dd>
        </div>
        <div class="flex gap-2">
          <dt class="text-gray-400 w-28 shrink-0">MM Token</dt>
          <dd class="text-white">{"•".repeat(Math.min(mmToken.length, 12))}</dd>
        </div>
        <div class="flex gap-2">
          <dt class="text-gray-400 w-28 shrink-0">LLM URL</dt>
          <dd class="text-white break-all">{llmUrl}</dd>
        </div>
        <div class="flex gap-2">
          <dt class="text-gray-400 w-28 shrink-0">LLM Model</dt>
          <dd class="text-white">{llmModel}</dd>
        </div>
      </dl>

      {#if saveError}
        <div
          class="bg-red-900/50 border border-red-500 text-red-200 rounded p-3 mb-4 text-sm"
          role="alert"
        >
          {saveError}
        </div>
      {/if}

      <div class="flex justify-between">
        <button
          onclick={back}
          class="px-5 py-2 bg-gray-700 hover:bg-gray-600 rounded font-medium text-sm transition-colors"
        >
          ← Back
        </button>
        <button
          onclick={save}
          disabled={saving}
          class="px-5 py-2 bg-green-600 hover:bg-green-500 disabled:opacity-40 rounded font-medium text-sm transition-colors"
        >
          {saving ? "Saving…" : "Save configuration"}
        </button>
      </div>
    {/if}
  </div>
</div>
