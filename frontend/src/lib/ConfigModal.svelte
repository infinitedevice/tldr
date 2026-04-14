<script lang="ts">
    // SPDX-FileCopyrightText: 2026 Martin Donnelly
    // SPDX-FileCopyrightText: 2026 Collabora Ltd.
    // SPDX-License-Identifier: AGPL-3.0-or-later

    /**
     * @component ConfigModal
     * Single-form configuration modal. Pre-populates from the daemon's current
     * config on mount. Replaces the multi-step ConfigWizard.
     *
     * Props:
     *  - oncomplete()    — called after a successful save
     *  - allowCancel     — if true, show Cancel button and allow ESC to close
     *  - onskip()        — called when the user clicks "Skip for now"
     */

    import { onMount } from "svelte";

    let {
        oncomplete,
        allowCancel = false,
        onskip,
    }: {
        oncomplete: () => void;
        allowCancel?: boolean;
        onskip?: () => void;
    } = $props();

    let dialogEl: HTMLDivElement | undefined = $state();

    // Form fields
    let mmUrl = $state("");
    let mmToken = $state("");
    let llmUrl = $state("");
    let llmModel = $state("");
    let llmToken = $state("");

    // Eye-toggle visibility
    let showMmToken = $state(false);
    let showLlmToken = $state(false);

    let saving = $state(false);
    let loading = $state(true);
    let saveError = $state("");

    // Focus-trap + ESC handler
    $effect(() => {
        if (!dialogEl) return;
        const el = dialogEl;

        // Auto-focus first input
        const first = el.querySelector<HTMLElement>(
            'input, button, select, textarea, [tabindex]:not([tabindex="-1"])',
        );
        first?.focus();

        function onKeyDown(e: KeyboardEvent) {
            if (e.key === "Escape" && allowCancel) {
                oncomplete();
                return;
            }
            if (e.key !== "Tab") return;
            const focusable = Array.from(
                el.querySelectorAll<HTMLElement>(
                    'button:not([disabled]), input, select, textarea, [tabindex]:not([tabindex="-1"])',
                ),
            );
            if (focusable.length === 0) return;
            const f = focusable[0];
            const l = focusable[focusable.length - 1];
            if (e.shiftKey) {
                if (document.activeElement === f) {
                    e.preventDefault();
                    l.focus();
                }
            } else {
                if (document.activeElement === l) {
                    e.preventDefault();
                    f.focus();
                }
            }
        }
        document.addEventListener("keydown", onKeyDown);
        return () => document.removeEventListener("keydown", onKeyDown);
    });

    onMount(async () => {
        try {
            const resp = await fetch("/api/v1/config");
            if (resp.ok) {
                const cfg = await resp.json();
                mmUrl = cfg.mattermost?.server_url ?? "";
                // Tokens are not returned by the API for security — leave blank so user
                // can enter them; if blank on save, we preserve the existing value.
                mmToken = "";
                llmUrl = cfg.llm?.base_url ?? llmUrl;
                llmModel = cfg.llm?.model ?? llmModel;
                llmToken = "";
            }
        } finally {
            loading = false;
        }
    });

    const formValid = $derived(
        mmUrl.trim().length > 0 &&
            llmUrl.trim().length > 0 &&
            llmModel.trim().length > 0,
    );

    async function save() {
        saving = true;
        saveError = "";
        try {
            // Load existing config first so we don't clobber fields we can't see
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            let existing: Record<string, any> = {};
            const r = await fetch("/api/v1/config");
            if (r.ok) existing = await r.json();

            const config = {
                ...existing,
                mattermost: {
                    ...(existing.mattermost ?? {}),
                    server_url: mmUrl.trim(),
                    // Only overwrite token if user entered something
                    ...(mmToken.trim() ? { token: mmToken.trim() } : {}),
                },
                llm: {
                    ...(existing.llm ?? {}),
                    base_url: llmUrl.trim(),
                    model: llmModel.trim(),
                    ...(llmToken.trim()
                        ? { bearer_token: llmToken.trim() }
                        : {}),
                },
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
                saveError = data.error || "Failed to save configuration";
            }
        } catch (e: unknown) {
            saveError = e instanceof Error ? e.message : "Network error";
        } finally {
            saving = false;
        }
    }
</script>

<!-- Modal backdrop -->
<div
    bind:this={dialogEl}
    class="fixed inset-0 z-50 flex items-center justify-center p-4"
    style="backdrop-filter: blur(4px); background: rgba(0,0,0,0.7);"
    role="dialog"
    aria-modal="true"
    aria-label="Configuration"
>
    <div
        class="bg-gray-900 border border-gray-700 rounded-xl shadow-2xl w-full max-w-lg"
    >
        <!-- Header -->
        <div
            class="flex items-center justify-between px-6 pt-6 pb-4 border-b border-gray-800"
        >
            <h2 class="text-lg font-semibold text-white">Configuration</h2>
            {#if allowCancel}
                <button
                    onclick={oncomplete}
                    class="text-gray-400 hover:text-white text-xl leading-none transition-colors"
                    aria-label="Close">✕</button
                >
            {/if}
        </div>

        {#if loading}
            <div class="px-6 py-10 text-center text-gray-500 text-sm">
                Loading…
            </div>
        {:else}
            <div class="px-6 py-5 space-y-5">
                <!-- Mattermost section -->
                <fieldset>
                    <legend
                        class="text-xs uppercase tracking-widest text-cyan-400 font-semibold mb-3"
                        >Mattermost</legend
                    >

                    <label class="block mb-3">
                        <span class="text-sm text-gray-300 block mb-1"
                            >Server URL</span
                        >
                        <input
                            type="url"
                            placeholder="https://mattermost.example.com"
                            bind:value={mmUrl}
                            class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
                            aria-required="true"
                        />
                    </label>

                    <label class="block">
                        <span class="text-sm text-gray-300 block mb-1">
                            Access token
                            <span class="text-gray-500 font-normal"
                                >(leave blank to keep existing)</span
                            >
                        </span>
                        <div class="relative">
                            <input
                                type={showMmToken ? "text" : "password"}
                                placeholder="your-mattermost-token"
                                bind:value={mmToken}
                                class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 pr-10 text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
                            />
                            <button
                                type="button"
                                onclick={() => (showMmToken = !showMmToken)}
                                class="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300 transition-colors"
                                aria-label={showMmToken
                                    ? "Hide token"
                                    : "Show token"}
                            >
                                {#if showMmToken}
                                    <!-- eye-off -->
                                    <svg
                                        viewBox="0 0 24 24"
                                        class="w-4 h-4 fill-none stroke-current"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    >
                                        <path
                                            d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"
                                        />
                                        <path
                                            d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"
                                        />
                                        <line x1="1" y1="1" x2="23" y2="23" />
                                    </svg>
                                {:else}
                                    <!-- eye -->
                                    <svg
                                        viewBox="0 0 24 24"
                                        class="w-4 h-4 fill-none stroke-current"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    >
                                        <path
                                            d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"
                                        />
                                        <circle cx="12" cy="12" r="3" />
                                    </svg>
                                {/if}
                            </button>
                        </div>
                        <p class="text-xs text-gray-500 mt-1">
                            Generate in Mattermost → Profile → Security →
                            Personal Access Tokens.
                        </p>
                    </label>
                </fieldset>

                <!-- LLM section -->
                <fieldset>
                    <legend
                        class="text-xs uppercase tracking-widest text-cyan-400 font-semibold mb-3"
                        >Language Model</legend
                    >

                    <label class="block mb-3">
                        <span class="text-sm text-gray-300 block mb-1"
                            >API base URL</span
                        >
                        <input
                            type="url"
                            bind:value={llmUrl}
                            class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white focus:outline-none focus:border-cyan-500"
                            aria-required="true"
                        />
                    </label>

                    <label class="block mb-3">
                        <span class="text-sm text-gray-300 block mb-1"
                            >Model name</span
                        >
                        <input
                            type="text"
                            bind:value={llmModel}
                            class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-white focus:outline-none focus:border-cyan-500"
                            aria-required="true"
                        />
                    </label>

                    <label class="block">
                        <span class="text-sm text-gray-300 block mb-1">
                            API key
                            <span class="text-gray-500 font-normal"
                                >(optional — leave blank to keep existing)</span
                            >
                        </span>
                        <div class="relative">
                            <input
                                type={showLlmToken ? "text" : "password"}
                                placeholder="leave blank if not required"
                                bind:value={llmToken}
                                class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 pr-10 text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500"
                            />
                            <button
                                type="button"
                                onclick={() => (showLlmToken = !showLlmToken)}
                                class="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300 transition-colors"
                                aria-label={showLlmToken
                                    ? "Hide API key"
                                    : "Show API key"}
                            >
                                {#if showLlmToken}
                                    <svg
                                        viewBox="0 0 24 24"
                                        class="w-4 h-4 fill-none stroke-current"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    >
                                        <path
                                            d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"
                                        />
                                        <path
                                            d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"
                                        />
                                        <line x1="1" y1="1" x2="23" y2="23" />
                                    </svg>
                                {:else}
                                    <svg
                                        viewBox="0 0 24 24"
                                        class="w-4 h-4 fill-none stroke-current"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    >
                                        <path
                                            d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"
                                        />
                                        <circle cx="12" cy="12" r="3" />
                                    </svg>
                                {/if}
                            </button>
                        </div>
                    </label>
                </fieldset>

                {#if saveError}
                    <div
                        class="bg-red-900/50 border border-red-500 text-red-200 rounded p-3 text-sm"
                        role="alert"
                    >
                        {saveError}
                    </div>
                {/if}
            </div>

            <!-- Footer -->
            <div
                class="flex items-center justify-between px-6 pb-6 pt-2 border-t border-gray-800"
            >
                <div class="flex gap-2">
                    {#if onskip}
                        <button
                            onclick={onskip}
                            class="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors"
                        >
                            Skip for now
                        </button>
                    {/if}
                    {#if allowCancel}
                        <button
                            onclick={oncomplete}
                            class="px-4 py-2 text-sm bg-gray-700 hover:bg-gray-600 rounded font-medium transition-colors"
                        >
                            Cancel
                        </button>
                    {/if}
                </div>
                <button
                    onclick={save}
                    disabled={saving || !formValid}
                    class="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 rounded font-medium text-sm transition-colors"
                >
                    {saving ? "Saving…" : "Save"}
                </button>
            </div>
        {/if}
    </div>
</div>
