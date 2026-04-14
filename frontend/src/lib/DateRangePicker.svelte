<script lang="ts">
  // SPDX-FileCopyrightText: 2026 Martin Donnelly
  // SPDX-FileCopyrightText: 2026 Collabora Ltd.
  // SPDX-License-Identifier: MIT OR Apache-2.0

  /**
   * @component DateRangePicker
   * Compact date range selector with epoch-ms bindable values and preset shortcuts.
   */

  interface Props {
    fromMs: number;
    toMs: number;
    /** Optional lower bound (epoch ms). Presets and date inputs earlier than this are disabled. */
    minMs?: number | null;
  }

  let {
    fromMs = $bindable(),
    toMs = $bindable(),
    minMs = null,
  }: Props = $props();

  function msToDateInput(ms: number): string {
    return new Date(ms).toISOString().slice(0, 10);
  }

  function dateInputToMs(val: string, eod = false): number {
    const d = new Date(val + "T00:00:00");
    if (eod) d.setHours(23, 59, 59, 999);
    return d.getTime();
  }

  function presetFromMs(label: string): number {
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    switch (label) {
      case "Today":
        return today.getTime();
      case "Yesterday": {
        const y = new Date(today);
        y.setDate(today.getDate() - 1);
        return y.getTime();
      }
      case "Last 7 days": {
        const f = new Date(today);
        f.setDate(today.getDate() - 6);
        return f.getTime();
      }
      case "Last week": {
        const dow = today.getDay() || 7;
        const m = new Date(today);
        m.setDate(today.getDate() - dow - 6);
        return m.getTime();
      }
      case "Last month":
        return new Date(today.getFullYear(), today.getMonth() - 1, 1).getTime();
      case "Last quarter": {
        const q = Math.floor(today.getMonth() / 3);
        return new Date(today.getFullYear(), q * 3 - 3, 1).getTime();
      }
      case "Last year":
        return new Date(today.getFullYear() - 1, 0, 1).getTime();
      default:
        return today.getTime();
    }
  }

  function presetDisabled(label: string): boolean {
    if (!minMs) return false;
    return presetFromMs(label) < minMs;
  }

  function applyPreset(label: string) {
    const today = new Date();
    today.setHours(0, 0, 0, 0);

    switch (label) {
      case "Today": {
        const end = new Date(today);
        end.setHours(23, 59, 59, 999);
        fromMs = today.getTime();
        toMs = end.getTime();
        break;
      }
      case "Yesterday": {
        const y = new Date(today);
        y.setDate(today.getDate() - 1);
        const end = new Date(y);
        end.setHours(23, 59, 59, 999);
        fromMs = y.getTime();
        toMs = end.getTime();
        break;
      }
      case "Last 7 days": {
        const from = new Date(today);
        from.setDate(today.getDate() - 6);
        const end = new Date();
        end.setHours(23, 59, 59, 999);
        fromMs = from.getTime();
        toMs = end.getTime();
        break;
      }
      case "Last week": {
        // Calendar week Mon–Sun
        const dow = today.getDay() || 7;
        const mon = new Date(today);
        mon.setDate(today.getDate() - dow - 6);
        const sun = new Date(mon);
        sun.setDate(mon.getDate() + 6);
        sun.setHours(23, 59, 59, 999);
        fromMs = mon.getTime();
        toMs = sun.getTime();
        break;
      }
      case "Last month": {
        const from = new Date(today.getFullYear(), today.getMonth() - 1, 1);
        const end = new Date(
          today.getFullYear(),
          today.getMonth(),
          0,
          23,
          59,
          59,
          999,
        );
        fromMs = from.getTime();
        toMs = end.getTime();
        break;
      }
      case "Last quarter": {
        const q = Math.floor(today.getMonth() / 3);
        const from = new Date(today.getFullYear(), q * 3 - 3, 1);
        const end = new Date(today.getFullYear(), q * 3, 0, 23, 59, 59, 999);
        fromMs = from.getTime();
        toMs = end.getTime();
        break;
      }
      case "Last year": {
        const from = new Date(today.getFullYear() - 1, 0, 1);
        const end = new Date(today.getFullYear() - 1, 11, 31, 23, 59, 59, 999);
        fromMs = from.getTime();
        toMs = end.getTime();
        break;
      }
    }
  }

  const PRESETS = [
    "Today",
    "Yesterday",
    "Last 7 days",
    "Last week",
    "Last month",
    "Last quarter",
    "Last year",
  ];
</script>

<div class="flex flex-col gap-2">
  <!-- Preset shortcuts -->
  <div class="flex flex-wrap gap-1.5">
    {#each PRESETS as label}
      <button
        onclick={() => applyPreset(label)}
        disabled={presetDisabled(label)}
        class="px-2.5 py-1 text-xs bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-gray-500 rounded-lg text-gray-300 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        >{label}</button
      >
    {/each}
  </div>

  <!-- Manual date inputs -->
  <div class="flex items-center gap-2 flex-wrap">
    <input
      type="date"
      value={msToDateInput(fromMs)}
      min={minMs ? msToDateInput(minMs) : undefined}
      onchange={(e) => {
        fromMs = dateInputToMs((e.target as HTMLInputElement).value);
      }}
      class="bg-gray-800 border border-gray-600 rounded px-3 py-1.5 text-sm text-white focus:border-cyan-500 focus:outline-none"
    />
    <span class="text-gray-500 text-sm">→</span>
    <input
      type="date"
      value={msToDateInput(toMs)}
      onchange={(e) => {
        toMs = dateInputToMs((e.target as HTMLInputElement).value, true);
      }}
      class="bg-gray-800 border border-gray-600 rounded px-3 py-1.5 text-sm text-white focus:border-cyan-500 focus:outline-none"
    />
  </div>
</div>
