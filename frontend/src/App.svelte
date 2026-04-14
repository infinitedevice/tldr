<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: AGPL-3.0-or-later

  /**
   * @component App
   * Persistent application shell.
   * - Left icon-only NavRail (Skeleton) for page navigation
   * - Thin AppBar (Skeleton) with app title + avatar button
   * - Settings slide-over drawer opened by the avatar
   * - ConfigWizard modal when daemon is unconfigured
   * - Routes between SummarisePage and InsightsPage via `currentPage` state
   */

  import { onMount, onDestroy } from "svelte";
  import { Navigation, AppBar, Avatar } from "@skeletonlabs/skeleton-svelte";
  import ConfigModal from "./lib/ConfigModal.svelte";
  import PriorityUsersForm from "./lib/PriorityUsersForm.svelte";
  import SummarisePage from "./pages/SummarisePage.svelte";
  import InsightsPage from "./pages/InsightsPage.svelte";
  import { startTour, startTourIfNew } from "./lib/ProductTour";

  type Page = "summarise" | "insights";

  let currentPage: Page = $state("summarise");
  let showWizard = $state(false);
  let configAllowCancel = $state(false);
  let showUserMenu = $state(false);
  let showPriorityModal = $state(false);
  let showRoleModal = $state(false);

  // Health state for sidebar chips
  let health = $state({
    mm_ok: false,
    llm_ok: false,
    store_ok: false,
    rag_ok: false,
    rag_configured: false,
    rag_error: null as string | null,
  });
  let healthInterval: ReturnType<typeof setInterval> | null = null;

  // User info
  let userName = $state("");
  let priorityUsers: string[] = $state([]);
  let userRole = $state("");

  onMount(async () => {
    // Initial health check + start polling
    async function refreshHealth() {
      try {
        const h = await fetch("/api/v1/health").then((r) => r.json());
        health = {
          mm_ok: h.mm_status === "ok",
          llm_ok: h.llm_ok ?? false,
          store_ok: h.store_ok ?? false,
          rag_ok: h.rag_ok ?? false,
          rag_configured: h.rag_configured ?? false,
          rag_error: h.rag_error ?? null,
        };
        return h;
      } catch {
        return null;
      }
    }

    const h = await refreshHealth();
    if (!h?.configured) {
      configAllowCancel = false;
      showWizard = true;
      return;
    }

    healthInterval = setInterval(refreshHealth, 30_000);

    // Load user info + prefs in parallel
    await Promise.allSettled([
      fetch("/api/v1/me")
        .then((r) => r.json())
        .then((d) => {
          userName = d.display_name || d.username || "";
        }),
      fetch("/api/v1/config")
        .then((r) => r.json())
        .then((cfg) => {
          priorityUsers = cfg.priority_users ?? [];
          userRole = cfg.user_role ?? "";
        }),
    ]);

    startTourIfNew();
  });

  onDestroy(() => {
    if (healthInterval) clearInterval(healthInterval);
  });

  async function savePriorityUsers(users: string[]) {
    const resp = await fetch("/api/v1/config");
    const cfg = await resp.json();
    cfg.priority_users = users;
    const save = await fetch("/api/v1/config", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(cfg),
    });
    const result = await save.json();
    if (!result.ok) throw new Error(result.error ?? "Failed to save");
    priorityUsers = users;
  }

  async function saveUserRole(role: string) {
    const resp = await fetch("/api/v1/config");
    const cfg = await resp.json();
    cfg.user_role = role;
    const save = await fetch("/api/v1/config", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(cfg),
    });
    const result = await save.json();
    if (!result.ok) throw new Error(result.error ?? "Failed to save");
    userRole = role;
  }

  function onWizardComplete() {
    showWizard = false;
  }

  function handleNavChange(id: string) {
    currentPage = id as Page;
    showUserMenu = false;
  }
</script>

{#if showWizard}
  <ConfigModal
    oncomplete={onWizardComplete}
    allowCancel={configAllowCancel}
    onskip={configAllowCancel ? undefined : onWizardComplete}
  />
{/if}

<div
  class="flex h-screen overflow-hidden bg-gray-950 text-gray-100 pt-2"
  data-theme="cerberus"
>
  <!-- Left NavRail -->
  <Navigation
    value={currentPage}
    onValueChange={handleNavChange}
    background="bg-gray-900"
    width="w-24"
    padding="py-3"
    classes="border-r border-gray-800"
    tilesFlexDirection="flex-col"
    tilesJustify="justify-start"
    tilesGap="gap-1"
    tilesItems="items-center"
    tilesClasses="flex-1 pt-2"
  >
    {#snippet tiles()}
      <!-- Summarise -->
      <Navigation.Tile
        id="summarise"
        label="Summarise"
        title="Summarise unread channels"
        width="w-full"
        padding="px-1 py-2"
        gap="gap-1"
        rounded="rounded-lg"
        hover="hover:bg-gray-700/60"
        active="bg-gray-700/80"
        labelBase="text-[10px] leading-tight text-center"
      >
        <!-- List icon -->
        <svg
          viewBox="0 0 24 24"
          class="w-5 h-5 fill-none stroke-current mx-auto"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <line x1="8" y1="6" x2="21" y2="6" />
          <line x1="8" y1="12" x2="21" y2="12" />
          <line x1="8" y1="18" x2="13" y2="18" />
          <circle cx="3" cy="6" r="1" fill="currentColor" />
          <circle cx="3" cy="12" r="1" fill="currentColor" />
          <circle cx="3" cy="18" r="1" fill="currentColor" />
        </svg>
      </Navigation.Tile>

      <!-- Insights -->
      <Navigation.Tile
        id="insights"
        label="Insights"
        title="LLM-powered channel insights"
        width="w-full"
        padding="px-1 py-2"
        gap="gap-1"
        rounded="rounded-lg"
        hover="hover:bg-gray-700/60"
        active="bg-gray-700/80"
        labelBase="text-[10px] leading-tight text-center"
      >
        <!-- Lightbulb icon -->
        <svg
          viewBox="0 0 24 24"
          class="w-5 h-5 fill-none stroke-current mx-auto"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path
            d="M9 18h6M10 22h4M12 2a7 7 0 0 1 7 7c0 2.5-1.3 4.7-3 6l-1 3H9l-1-3C6.3 13.7 5 11.5 5 9a7 7 0 0 1 7-7z"
          />
        </svg>
      </Navigation.Tile>
    {/snippet}

    {#snippet footer()}
      <!-- llms.txt link -->
      <a
        href="/llms.txt"
        target="_blank"
        rel="noopener noreferrer"
        title="API documentation (llms.txt)"
        class="flex flex-col items-center gap-1 w-full px-1 py-2 rounded-lg text-gray-500 hover:text-gray-300 hover:bg-gray-700/40 transition-colors"
      >
        <svg
          viewBox="0 0 24 24"
          class="w-4 h-4 fill-none stroke-current"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path
            d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"
          />
          <polyline points="14 2 14 8 20 8" />
          <line x1="16" y1="13" x2="8" y2="13" />
          <line x1="16" y1="17" x2="8" y2="17" />
          <polyline points="10 9 9 9 8 9" />
        </svg>
        <span class="text-[9px] leading-tight">API</span>
      </a>

      <!-- Health chips: MM / LLM / DB / RAG in a 2x2 grid -->
      <div class="grid grid-cols-2 gap-1 w-full px-2 mt-1 mb-1">
        {#each [{ label: "MM", ok: health.mm_ok, tip: health.mm_ok ? "Mattermost: ok" : "Mattermost: unavailable" }, { label: "LLM", ok: health.llm_ok, tip: health.llm_ok ? "LLM: ok" : "LLM: unavailable" }, { label: "DB", ok: health.store_ok, tip: health.store_ok ? "Database: ok" : "Database: unavailable" }, { label: "RAG", ok: health.rag_ok, off: !health.rag_configured, tip: health.rag_ok ? "RAG: ok" : health.rag_error ? `RAG: ${health.rag_error}` : "RAG: disabled" }] as chip}
          <div
            class={`flex items-center justify-center gap-1 rounded-md px-1.5 py-1 text-[10px] font-medium leading-none cursor-default select-none
              ${
                chip.ok
                  ? "bg-green-900/50 text-green-300 border border-green-800/60"
                  : chip.off
                    ? "bg-gray-800/50 text-gray-600 border border-gray-700/60"
                    : "bg-red-900/50 text-red-400 border border-red-800/60"
              }`}
            title={chip.tip}
          >
            <span
              class={`w-1.5 h-1.5 rounded-full shrink-0 ${chip.ok ? "bg-green-400" : chip.off ? "bg-gray-600" : "bg-red-500"}`}
            ></span>
            {chip.label}
          </div>
        {/each}
      </div>
    {/snippet}
  </Navigation>

  <!-- Right column: AppBar + content -->
  <div class="flex-1 flex flex-col min-h-0 overflow-hidden">
    <!-- AppBar -->
    <AppBar
      background="bg-gray-900"
      border="border-b border-gray-800"
      padding="px-4 py-0"
      toolbarGridCols="grid-cols-[auto_1fr_auto]"
      classes="h-12 shrink-0"
    >
      {#snippet lead()}
        <span class="text-xl font-black text-cyan-400 mr-4">tldr</span>
      {/snippet}

      <!-- center: page title -->
      <span
        class="text-xs font-semibold uppercase tracking-widest text-gray-400"
      >
        {currentPage === "summarise" ? "Summarise" : "Insights"}
      </span>

      {#snippet trail()}
        <div class="relative flex items-center">
          <!-- Avatar button → opens user menu -->
          <button
            data-tour="settings-btn"
            onclick={() => {
              showUserMenu = !showUserMenu;
            }}
            aria-label="Open user menu"
            aria-expanded={showUserMenu}
            aria-haspopup="menu"
            class="rounded-full hover:ring-2 hover:ring-cyan-500 transition"
          >
            <Avatar
              src="/api/v1/me/avatar"
              name={userName || "User"}
              size="w-8 h-8"
              rounded="rounded-full"
              background="bg-gray-600"
              font="text-xs font-bold"
            />
          </button>

          {#if showUserMenu}
            <!-- Transparent backdrop closes menu on outside click -->
            <button
              onclick={() => (showUserMenu = false)}
              class="fixed inset-0 z-40 cursor-default"
              aria-hidden="true"
              tabindex="-1"
            ></button>
            <!-- Dropdown menu -->
            <div
              role="menu"
              class="absolute right-0 top-full mt-2 w-52 bg-gray-800 border border-gray-700 rounded-xl shadow-2xl z-50 py-1 overflow-hidden"
            >
              <button
                role="menuitem"
                onclick={() => {
                  showUserMenu = false;
                  showPriorityModal = true;
                }}
                class="w-full text-left px-4 py-2.5 text-sm text-gray-200 hover:bg-gray-700 flex items-center gap-3 transition-colors"
              >
                <svg
                  viewBox="0 0 24 24"
                  class="w-4 h-4 shrink-0 fill-none stroke-current"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                  <circle cx="9" cy="7" r="4" />
                  <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                  <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                </svg>
                Priority Users
              </button>
              <button
                role="menuitem"
                onclick={() => {
                  showUserMenu = false;
                  showRoleModal = true;
                }}
                class="w-full text-left px-4 py-2.5 text-sm text-gray-200 hover:bg-gray-700 flex items-center gap-3 transition-colors"
              >
                <svg
                  viewBox="0 0 24 24"
                  class="w-4 h-4 shrink-0 fill-none stroke-current"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
                  <circle cx="12" cy="7" r="4" />
                </svg>
                Your Role
              </button>
              <button
                role="menuitem"
                onclick={() => {
                  showUserMenu = false;
                  configAllowCancel = true;
                  showWizard = true;
                }}
                class="w-full text-left px-4 py-2.5 text-sm text-gray-200 hover:bg-gray-700 flex items-center gap-3 transition-colors"
              >
                <svg
                  viewBox="0 0 24 24"
                  class="w-4 h-4 shrink-0 fill-none stroke-current"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <circle cx="12" cy="12" r="3" />
                  <path
                    d="M19.07 4.93a10 10 0 0 1 0 14.14M4.93 4.93a10 10 0 0 0 0 14.14"
                  />
                </svg>
                Configuration
              </button>
              <hr class="my-1 border-gray-700" />
              <button
                role="menuitem"
                onclick={() => {
                  showUserMenu = false;
                  startTour();
                }}
                class="w-full text-left px-4 py-2.5 text-sm text-gray-200 hover:bg-gray-700 flex items-center gap-3 transition-colors"
              >
                <svg
                  viewBox="0 0 24 24"
                  class="w-4 h-4 shrink-0 fill-none stroke-current"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <circle cx="12" cy="12" r="10" />
                  <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
                  <line
                    x1="12"
                    y1="17"
                    x2="12.01"
                    y2="17"
                    stroke-width="3"
                    stroke-linecap="round"
                  />
                </svg>
                Help / Tour
              </button>
            </div>
          {/if}
        </div>
      {/snippet}
    </AppBar>

    <!-- Page content — both pages are always mounted to preserve state -->
    <main class="flex-1 overflow-y-auto bg-gray-950">
      <div class:hidden={currentPage !== "summarise"}>
        <SummarisePage />
      </div>
      <div class:hidden={currentPage !== "insights"}>
        <InsightsPage {userRole} />
      </div>
    </main>
  </div>
</div>

<!-- Priority Users modal -->
{#if showPriorityModal}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center p-4"
    style="backdrop-filter: blur(4px); background: rgba(0,0,0,0.6);"
    role="dialog"
    aria-modal="true"
    aria-label="Priority Users"
  >
    <div
      class="bg-gray-900 border border-gray-700 rounded-xl shadow-2xl w-full max-w-md p-6"
    >
      <div class="flex items-center justify-between mb-4">
        <h2 class="text-sm font-semibold text-gray-200 uppercase tracking-wide">
          Priority Users
        </h2>
        <button
          onclick={() => (showPriorityModal = false)}
          class="text-gray-400 hover:text-white text-lg leading-none"
          aria-label="Close">✕</button
        >
      </div>
      <PriorityUsersForm
        bind:users={priorityUsers}
        onsave={async (u) => {
          await savePriorityUsers(u);
          showPriorityModal = false;
        }}
      />
    </div>
  </div>
{/if}

<!-- Your Role modal -->
{#if showRoleModal}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center p-4"
    style="backdrop-filter: blur(4px); background: rgba(0,0,0,0.6);"
    role="dialog"
    aria-modal="true"
    aria-label="Your Role"
  >
    <div
      class="bg-gray-900 border border-gray-700 rounded-xl shadow-2xl w-full max-w-md p-6"
    >
      <div class="flex items-center justify-between mb-4">
        <h2 class="text-sm font-semibold text-gray-200 uppercase tracking-wide">
          Your Role
        </h2>
        <button
          onclick={() => (showRoleModal = false)}
          class="text-gray-400 hover:text-white text-lg leading-none"
          aria-label="Close">✕</button
        >
      </div>
      <p class="text-xs text-gray-400 mb-3">
        Describes your role for the Insights page — the LLM uses this to tailor
        cross-channel synthesis.
      </p>
      <textarea
        value={userRole}
        oninput={(e) => (userRole = (e.target as HTMLTextAreaElement).value)}
        rows="6"
        placeholder="e.g. I'm a software engineering consultant focused on customer issues and project delivery…"
        class="w-full bg-gray-800 border border-gray-600 rounded px-3 py-2 text-sm text-white placeholder-gray-500 focus:outline-none focus:border-cyan-500 resize-y"
      ></textarea>
      <div class="flex justify-end gap-2 mt-3">
        <button
          onclick={() => (showRoleModal = false)}
          class="px-4 py-1.5 bg-gray-700 hover:bg-gray-600 rounded text-sm text-gray-200 transition-colors"
          >Cancel</button
        >
        <button
          onclick={async () => {
            await saveUserRole(userRole);
            showRoleModal = false;
          }}
          class="px-4 py-1.5 bg-cyan-700 hover:bg-cyan-600 rounded text-sm text-white transition-colors"
          >Save</button
        >
      </div>
    </div>
  </div>
{/if}
