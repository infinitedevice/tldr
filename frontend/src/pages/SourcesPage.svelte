<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @page SourcesPage
   * GUI for enabling/disabling Mattermost teams/channels and email mailboxes,
   * and adding per-source custom instructions for the LLM summariser.
   */

  import { onMount } from "svelte";

  interface ChannelInfo {
    id: string;
    name: string;
    display_name: string;
    team_name: string;
    enabled: boolean;
    instructions: string | null;
  }

  interface TeamInfo {
    id: string;
    name: string;
    display_name: string;
    enabled: boolean;
    instructions: string | null;
    channels: ChannelInfo[];
  }

  interface MailboxInfo {
    name: string;
    enabled: boolean;
    instructions: string | null;
  }

  interface SourcesData {
    mattermost: {
      teams: TeamInfo[];
    };
    email: {
      mailboxes: MailboxInfo[];
    } | null;
  }

  // Working copies — mutated by toggles/textareas
  let data: SourcesData | null = $state(null);
  let loading = $state(true);
  let loadError: string | null = $state(null);
  let saving = $state(false);
  let saveResult: { ok: boolean; message: string } | null = $state(null);

  // Track which team accordions are expanded
  let expandedTeams: Set<string> = $state(new Set());

  // All IMAP mailboxes from the server
  let imapMailboxes: string[] = $state([]);
  let imapLoading = $state(false);

  onMount(async () => {
    await loadSources();
    await loadImapMailboxes();
  });

  async function loadSources() {
    loading = true;
    loadError = null;
    try {
      const res = await fetch("/api/v1/config/sources");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      data = await res.json();
      // Teams start collapsed; user expands as needed
    } catch (e) {
      loadError = `Failed to load sources: ${e}`;
    } finally {
      loading = false;
    }
  }

  async function loadImapMailboxes() {
    imapLoading = true;
    try {
      const res = await fetch("/api/v1/email/mailboxes");
      if (!res.ok) return;
      const json = await res.json();
      imapMailboxes = json.mailboxes ?? [];
      // Merge IMAP mailboxes into data.email.mailboxes — add any that aren't yet present
      if (data?.email && imapMailboxes.length > 0) {
        const existing = new Set(data.email.mailboxes.map((m: MailboxInfo) => m.name));
        for (const name of imapMailboxes) {
          if (!existing.has(name)) {
            data.email.mailboxes.push({ name, enabled: true, instructions: null });
          }
        }
      }
    } catch {
      // Non-fatal — email section falls back to whatever was in data
    } finally {
      imapLoading = false;
    }
  }

  function toggleTeam(teamId: string) {
    if (expandedTeams.has(teamId)) {
      expandedTeams.delete(teamId);
    } else {
      expandedTeams.add(teamId);
    }
    expandedTeams = new Set(expandedTeams); // trigger reactivity
  }

  async function save() {
    if (!data) return;
    saving = true;
    saveResult = null;

    const body = {
      mattermost: {
        teams: data.mattermost.teams.map((t) => ({
          name: t.name,
          enabled: t.enabled,
          instructions: t.instructions || null,
        })),
        channels: data.mattermost.teams.flatMap((t) =>
          t.channels.map((ch) => ({
            name: ch.name,
            team: t.name,
            enabled: ch.enabled,
            instructions: ch.instructions || null,
          }))
        ),
      },
      email: data.email
        ? {
            mailboxes: data.email.mailboxes.map((m) => ({
              name: m.name,
              enabled: m.enabled,
              instructions: m.instructions || null,
            })),
          }
        : null,
    };

    try {
      const res = await fetch("/api/v1/config/sources", {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(err.error || `HTTP ${res.status}`);
      }
      saveResult = { ok: true, message: "Configuration saved." };
    } catch (e) {
      saveResult = { ok: false, message: `Save failed: ${e}` };
    } finally {
      saving = false;
      // Auto-clear success message after 3 s
      if (saveResult?.ok) {
        setTimeout(() => {
          saveResult = null;
        }, 3000);
      }
    }
  }
</script>

<div class="sources-page" data-tour="sources-page">
  <div class="page-header">
    <h2>Sources</h2>
    <p class="subtitle">Enable or disable data sources and add custom LLM instructions per channel or mailbox.</p>
  </div>

  {#if loading}
    <div class="loading-state">
      <div class="spinner"></div>
      <span>Loading sources…</span>
    </div>
  {:else if loadError}
    <div class="error-banner">
      <span>{loadError}</span>
      <button onclick={loadSources}>Retry</button>
    </div>
  {:else if data}
    <div class="sources-content">
      <!-- Mattermost Section -->
      <section class="source-section" data-tour="sources-mattermost">
        <div class="section-header">
          <svg class="section-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
          </svg>
          <h3>Mattermost</h3>
        </div>

        {#if data.mattermost.teams.length === 0}
          <p class="empty-hint">No teams found. Make sure the daemon is connected to Mattermost.</p>
        {:else}
          {#each data.mattermost.teams as team (team.id)}
            <div class="team-card" class:team-disabled={!team.enabled}>
              <div class="team-header">
                <button
                  class="team-toggle-btn"
                  onclick={() => toggleTeam(team.id)}
                  aria-expanded={expandedTeams.has(team.id)}
                >
                  <svg class="chevron" class:expanded={expandedTeams.has(team.id)} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="6 9 12 15 18 9"/>
                  </svg>
                  <span class="team-name">{team.display_name || team.name}</span>
                  <span class="channel-count">{team.channels.length} channels</span>
                </button>
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    bind:checked={team.enabled}
                    class="toggle-input"
                  />
                  <span class="toggle-track"></span>
                </label>
              </div>

              {#if expandedTeams.has(team.id)}
                <div class="team-body">
                  <div class="instructions-row">
                    <label class="instructions-label" for="team-instr-{team.id}">
                      Team instructions (applied to all channels in this team):
                    </label>
                    <textarea
                      id="team-instr-{team.id}"
                      class="instructions-input"
                      rows="2"
                      placeholder="e.g. This is the Apertis project team. Focus on technical blockers."
                      bind:value={team.instructions}
                      disabled={!team.enabled}
                    ></textarea>
                  </div>

                  <div class="channels-list">
                    {#each team.channels as channel (channel.id)}
                      <div class="channel-row" class:channel-disabled={!channel.enabled || !team.enabled}>
                        <div class="channel-row-top">
                          <span class="channel-hash">#</span>
                          <span class="channel-name">{channel.display_name || channel.name}</span>
                          <label class="toggle-label small">
                            <input
                              type="checkbox"
                              bind:checked={channel.enabled}
                              disabled={!team.enabled}
                              class="toggle-input"
                            />
                            <span class="toggle-track"></span>
                          </label>
                        </div>
                        <div class="channel-instructions">
                          <textarea
                            class="instructions-input small"
                            rows="1"
                            placeholder="Custom instructions for this channel…"
                            bind:value={channel.instructions}
                            disabled={!channel.enabled || !team.enabled}
                          ></textarea>
                        </div>
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
            </div>
          {/each}
        {/if}
      </section>

      <!-- Email Section -->
      {#if data.email}
        <section class="source-section" data-tour="sources-email">
          <div class="section-header">
            <svg class="section-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M4 4h16c1.1 0 2 .9 2 2v12c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V6c0-1.1.9-2 2-2z"/>
              <polyline points="22,6 12,13 2,6"/>
            </svg>
            <h3>Email</h3>
          </div>

          {#if imapLoading && data.email.mailboxes.length === 0}
            <div class="loading-state">
              <div class="spinner"></div>
              <span>Connecting to IMAP…</span>
            </div>
          {:else if data.email.mailboxes.length === 0}
            <p class="empty-hint">No mailboxes found. Check your IMAP connection.</p>
          {:else}
            <div class="mailboxes-list">
              {#each data.email.mailboxes as mailbox (mailbox.name)}
                <div class="mailbox-card" class:mailbox-disabled={!mailbox.enabled}>
                  <div class="mailbox-header">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="mailbox-icon">
                      <path d="M22 17H2a3 3 0 0 0 3-3V9a9 7 0 0 1 14 0v5a3 3 0 0 0 3 3zm-8.27 4a2 2 0 0 1-3.46 0"/>
                    </svg>
                    <span class="mailbox-name">{mailbox.name}</span>
                    <label class="toggle-label small">
                      <input
                        type="checkbox"
                        bind:checked={mailbox.enabled}
                        class="toggle-input"
                      />
                      <span class="toggle-track"></span>
                    </label>
                  </div>
                  <div class="mailbox-instructions">
                    <textarea
                      class="instructions-input small"
                      rows="1"
                      placeholder="Custom instructions for this mailbox…"
                      bind:value={mailbox.instructions}
                      disabled={!mailbox.enabled}
                    ></textarea>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {/if}
    </div>

    <div class="save-bar">
      {#if saveResult}
        <span class="save-message" class:save-ok={saveResult.ok} class:save-error={!saveResult.ok}>
          {saveResult.message}
        </span>
      {/if}
      <button class="save-btn" onclick={save} disabled={saving}>
        {#if saving}Saving…{:else}Save changes{/if}
      </button>
    </div>
  {/if}
</div>

<style>
  .sources-page {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .page-header {
    padding: 1.25rem 1.5rem 0.75rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .page-header h2 {
    margin: 0 0 0.25rem;
    font-size: 1.1rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .subtitle {
    margin: 0;
    font-size: 0.82rem;
    color: var(--text-muted);
  }

  .loading-state {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 2rem 1.5rem;
    color: var(--text-muted);
  }

  .spinner {
    width: 18px;
    height: 18px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 1rem 1.5rem;
    background: var(--error-bg, #3a1a1a);
    color: var(--error-fg, #f87171);
    font-size: 0.85rem;
  }

  .error-banner button {
    padding: 0.25rem 0.75rem;
    border: 1px solid currentColor;
    border-radius: 4px;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.8rem;
  }

  .sources-content {
    flex: 1;
    overflow-y: auto;
    padding: 1rem 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .source-section {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .section-icon {
    width: 18px;
    height: 18px;
    color: var(--accent);
    flex-shrink: 0;
  }

  .section-header h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .empty-hint {
    font-size: 0.82rem;
    color: var(--text-muted);
    padding: 0.5rem 0;
    margin: 0;
  }

  /* Team card */
  .team-card {
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }

  .team-card.team-disabled {
    opacity: 0.6;
  }

  .team-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.6rem 0.75rem;
    background: var(--surface-2, var(--bg-secondary));
  }

  .team-toggle-btn {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-primary);
    font-size: 0.88rem;
    font-weight: 500;
    flex: 1;
    text-align: left;
    padding: 0;
  }

  .chevron {
    width: 14px;
    height: 14px;
    color: var(--text-muted);
    transition: transform 0.15s ease;
    flex-shrink: 0;
  }

  .chevron.expanded {
    transform: rotate(180deg);
  }

  .channel-count {
    font-size: 0.75rem;
    color: var(--text-muted);
    font-weight: 400;
  }

  .team-body {
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    border-top: 1px solid var(--border);
  }

  /* Toggle switch */
  .toggle-label {
    display: flex;
    align-items: center;
    cursor: pointer;
    flex-shrink: 0;
  }

  .toggle-input {
    position: absolute;
    opacity: 0;
    width: 0;
    height: 0;
  }

  .toggle-track {
    display: inline-block;
    width: 34px;
    height: 18px;
    background: var(--border);
    border-radius: 9px;
    position: relative;
    transition: background 0.15s ease;
  }

  .toggle-track::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    background: white;
    border-radius: 50%;
    transition: transform 0.15s ease;
  }

  .toggle-input:checked + .toggle-track {
    background: var(--accent);
  }

  .toggle-input:checked + .toggle-track::after {
    transform: translateX(16px);
  }

  .toggle-label.small .toggle-track {
    width: 28px;
    height: 15px;
  }

  .toggle-label.small .toggle-track::after {
    width: 11px;
    height: 11px;
  }

  .toggle-label.small .toggle-input:checked + .toggle-track::after {
    transform: translateX(13px);
  }

  /* Instructions */
  .instructions-row {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .instructions-label {
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  .instructions-input {
    width: 100%;
    box-sizing: border-box;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 0.82rem;
    font-family: inherit;
    resize: vertical;
    line-height: 1.4;
  }

  .instructions-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .instructions-input:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .instructions-input.small {
    font-size: 0.79rem;
    rows: 1;
  }

  /* Channel rows */
  .channels-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .channel-row {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg-primary);
  }

  .channel-row.channel-disabled {
    opacity: 0.55;
  }

  .channel-row-top {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .channel-hash {
    color: var(--text-muted);
    font-size: 0.82rem;
    flex-shrink: 0;
  }

  .channel-name {
    flex: 1;
    font-size: 0.85rem;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Mailboxes */
  .mailboxes-list {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .mailbox-card {
    border: 1px solid var(--border);
    border-radius: 7px;
    overflow: hidden;
    padding: 0.6rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    background: var(--bg-primary);
  }

  .mailbox-card.mailbox-disabled {
    opacity: 0.6;
  }

  .mailbox-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .mailbox-icon {
    width: 15px;
    height: 15px;
    color: var(--accent);
    flex-shrink: 0;
  }

  .mailbox-name {
    flex: 1;
    font-size: 0.88rem;
    font-weight: 500;
    color: var(--text-primary);
  }

  /* Save bar */
  .save-bar {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 1rem;
    padding: 0.75rem 1.5rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
    background: var(--bg-primary);
  }

  .save-message {
    font-size: 0.82rem;
  }

  .save-ok {
    color: var(--success-fg, #4ade80);
  }

  .save-error {
    color: var(--error-fg, #f87171);
  }

  .save-btn {
    padding: 0.45rem 1.1rem;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 0.88rem;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .save-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .save-btn:not(:disabled):hover {
    opacity: 0.88;
  }
</style>
