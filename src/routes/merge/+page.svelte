<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { MergeData, MergeRegion } from "$lib/p4";

  // Opened by the Rust `open_merge_window` command; the job itself is fetched by
  // id (regions don't fit in a query string).
  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  type Pick = "ours" | "theirs" | "both" | "base" | "custom";

  let data = $state<MergeData | null>(null);
  let error = $state("");
  let saving = $state(false);
  /** Per-conflict resolution, keyed by region index. Unset = still to settle. */
  let picks = $state<Record<number, Pick>>({});
  let custom = $state<Record<number, string>>({});
  let editing = $state<number | null>(null);
  let current = $state(0); // which conflict the prev/next buttons are on

  const regions = $derived(data?.regions ?? []);
  const conflicts = $derived(
    regions.map((r, i) => (r.kind === "conflict" ? i : -1)).filter((i) => i >= 0),
  );
  const unsettled = $derived(conflicts.filter((i) => !picks[i]));

  /** The lines a region contributes given the current picks. */
  function linesFor(r: MergeRegion, i: number): string[] {
    if (r.kind !== "conflict") return r.lines;
    const p = picks[i];
    if (!p) return [];
    if (p === "custom") return (custom[i] ?? "").split("\n");
    if (p === "ours") return r.ours;
    if (p === "theirs") return r.theirs;
    if (p === "base") return r.base;
    return [...r.theirs, ...r.ours]; // "both": depot first, then the workspace
  }
  const result = $derived(regions.flatMap((r, i) => linesFor(r, i)));

  function label(r: MergeRegion): string {
    if (r.kind === "same") return "";
    if (r.kind === "ours") return "workspace only";
    if (r.kind === "theirs") return "depot only";
    if (r.kind === "both") return "same on both";
    return "conflict";
  }
  /** What each pane shows for a region (base / theirs / yours columns). */
  function pane(r: MergeRegion, which: "base" | "theirs" | "ours"): string[] {
    if (r.kind === "same") return r.lines;
    if (r.kind === "conflict") return r[which];
    if (which === "base") return r.base;
    // A one-sided change: the other side still holds the base text.
    if (r.kind === "ours") return which === "ours" ? r.lines : r.base;
    return which === "theirs" ? r.lines : r.base;
  }

  function set(i: number, p: Pick) {
    picks = { ...picks, [i]: p };
    if (p === "custom" && custom[i] === undefined) {
      const r = regions[i];
      if (r.kind === "conflict") custom = { ...custom, [i]: [...r.theirs, ...r.ours].join("\n") };
    }
    editing = p === "custom" ? i : null;
  }

  function goTo(n: number) {
    if (!conflicts.length) return;
    current = ((n % conflicts.length) + conflicts.length) % conflicts.length;
    document
      .querySelector(`[data-region="${conflicts[current]}"]`)
      ?.scrollIntoView({ block: "center" });
  }

  async function save() {
    if (!data || unsettled.length) return;
    saving = true;
    try {
      const text = result.join("\n") + "\n";
      await invoke<string>("merge_save", { id, text });
      await getCurrentWindow().close();
    } catch (e) {
      error = String(e);
      saving = false;
    }
  }

  async function cancel() {
    try {
      await invoke<void>("merge_cancel", { id });
    } catch {
      /* closing anyway */
    }
    await getCurrentWindow().close();
  }

  onMount(async () => {
    try {
      data = await invoke<MergeData>("merge_data", { id });
      // Auto-settle nothing: conflicts must be chosen deliberately. Jump to the
      // first one so the window opens on the work to be done.
      if (data.conflicts > 0) setTimeout(() => goTo(0), 0);
    } catch (e) {
      error = String(e);
    }
  });
</script>

<div class="wrap">
  <div class="bar">
    {#if data}
      <span class="name mono">{data.name}</span>
      <span class="dim">
        {data.conflicts} conflict{data.conflicts === 1 ? "" : "s"}
        {#if data.conflicts}· {unsettled.length} still to settle{/if}
      </span>
      <span class="grow"></span>
      {#if conflicts.length > 1}
        <button onclick={() => goTo(current - 1)} title="Previous conflict">▲</button>
        <span class="dim">{current + 1}/{conflicts.length}</span>
        <button onclick={() => goTo(current + 1)} title="Next conflict">▼</button>
      {/if}
      <button onclick={cancel} disabled={saving}>Cancel</button>
      <button
        class="primary"
        disabled={saving || unsettled.length > 0}
        title={unsettled.length
          ? `${unsettled.length} conflict(s) still to settle`
          : data.kind === "resolve"
            ? "Write the merged file and mark the resolve done"
            : "Write the merged region back into the file"}
        onclick={save}
      >
        {saving ? "Saving…" : data.kind === "resolve" ? "Save & resolve" : "Save"}
      </button>
    {/if}
  </div>

  {#if error}
    <div class="err mono">{error}</div>
  {:else if !data}
    <div class="dim pad">Loading…</div>
  {:else}
    <div class="heads">
      <div class="head">{data.baseLabel}</div>
      <div class="head">{data.theirsLabel}</div>
      <div class="head">{data.yoursLabel}</div>
    </div>
    <div class="panes">
      {#each regions as r, i (i)}
        <div class="row {r.kind}" data-region={i}>
          {#each ["base", "theirs", "ours"] as const as which}
            <div class="cell">
              {#each pane(r, which) as line}
                <div class="line mono">{line || " "}</div>
              {/each}
            </div>
          {/each}
          {#if r.kind !== "same"}
            <div class="tag {r.kind}">{label(r)}</div>
          {/if}
        </div>
      {/each}
    </div>

    <div class="resbar">
      <span>Result</span>
      <span class="dim">{result.length} lines</span>
    </div>
    <div class="res">
      {#each regions as r, i (i)}
        {#if r.kind === "conflict"}
          <div class="conf" class:settled={!!picks[i]} data-region-res={i}>
            <div class="confbar">
              <span class="ctitle">Conflict {conflicts.indexOf(i) + 1}</span>
              <button class:on={picks[i] === "theirs"} onclick={() => set(i, "theirs")}>
                Take depot
              </button>
              <button class:on={picks[i] === "ours"} onclick={() => set(i, "ours")}>
                Take workspace
              </button>
              <button class:on={picks[i] === "both"} onclick={() => set(i, "both")}>Both</button>
              <button class:on={picks[i] === "base"} onclick={() => set(i, "base")}>Base</button>
              <button class:on={picks[i] === "custom"} onclick={() => set(i, "custom")}>
                Edit…
              </button>
            </div>
            {#if editing === i}
              <textarea
                class="mono"
                rows={Math.min(20, (custom[i] ?? "").split("\n").length + 1)}
                value={custom[i] ?? ""}
                oninput={(e) => (custom = { ...custom, [i]: e.currentTarget.value })}
              ></textarea>
            {:else if picks[i]}
              {#each linesFor(r, i) as line}
                <div class="line mono">{line || " "}</div>
              {/each}
            {:else}
              <div class="line dim">— pick a side —</div>
            {/if}
          </div>
        {:else}
          {#each linesFor(r, i) as line}
            <div class="line mono" class:ours={r.kind === "ours"} class:theirs={r.kind === "theirs"}>
              {line || " "}
            </div>
          {/each}
        {/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  :global(body) {
    margin: 0;
    background: var(--bg, #1b1b1b);
    color: var(--text, #ddd);
  }
  .wrap {
    display: flex;
    flex-direction: column;
    height: 100vh;
    font-size: 12px;
  }
  .mono {
    font-family: var(--mono, ui-monospace, Consolas, monospace);
  }
  .dim {
    color: var(--text-dim, #999);
  }
  .pad {
    padding: 12px;
  }
  .grow {
    flex: 1;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border, #333);
    background: var(--bg-panel, #232323);
    flex: none;
  }
  .name {
    font-weight: 600;
  }
  .err {
    padding: 10px;
    color: var(--warn, #d9a33a);
    white-space: pre-wrap;
  }
  .heads,
  .row {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
  }
  .heads {
    flex: none;
    border-bottom: 1px solid var(--border, #333);
    background: var(--bg-alt, #1f1f1f);
  }
  .head {
    padding: 4px 8px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim, #999);
    border-right: 1px solid var(--border, #333);
  }
  .panes {
    flex: 1 1 55%;
    overflow: auto;
    min-height: 0;
  }
  .row {
    position: relative;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
  }
  .row.conflict {
    background: rgba(215, 106, 106, 0.09);
    outline: 1px solid rgba(215, 106, 106, 0.35);
  }
  .row.ours {
    background: rgba(108, 195, 108, 0.07);
  }
  .row.theirs {
    background: rgba(106, 154, 215, 0.07);
  }
  .row.both {
    background: rgba(180, 180, 180, 0.05);
  }
  .cell {
    border-right: 1px solid var(--border, #333);
    min-width: 0;
    overflow-x: auto;
  }
  .line {
    padding: 0 8px;
    white-space: pre;
    line-height: 1.45;
  }
  .line.ours {
    background: rgba(108, 195, 108, 0.09);
  }
  .line.theirs {
    background: rgba(106, 154, 215, 0.09);
  }
  .tag {
    position: absolute;
    top: 0;
    right: 0;
    font-size: 9px;
    padding: 0 5px;
    border-bottom-left-radius: 4px;
    background: var(--bg-panel, #232323);
    color: var(--text-dim, #999);
  }
  .tag.conflict {
    color: var(--danger, #d76a6a);
  }
  .resbar {
    flex: none;
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 4px 10px;
    border-top: 1px solid var(--border, #333);
    border-bottom: 1px solid var(--border, #333);
    background: var(--bg-alt, #1f1f1f);
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim, #999);
  }
  .res {
    flex: 1 1 45%;
    overflow: auto;
    min-height: 0;
  }
  .conf {
    border: 1px solid rgba(215, 106, 106, 0.45);
    border-radius: 4px;
    margin: 4px 6px;
  }
  .conf.settled {
    border-color: rgba(108, 195, 108, 0.4);
  }
  .confbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    background: rgba(255, 255, 255, 0.04);
    flex-wrap: wrap;
  }
  .ctitle {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim, #999);
    margin-right: 4px;
  }
  button {
    background: var(--bg-alt, #1f1f1f);
    color: inherit;
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 11px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button.on {
    border-color: var(--accent, #d98d3a);
    color: var(--accent, #d98d3a);
  }
  .primary {
    border-color: var(--accent, #d98d3a);
    color: var(--accent, #d98d3a);
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    background: var(--bg, #1b1b1b);
    color: inherit;
    border: 0;
    border-top: 1px solid var(--border, #333);
    padding: 4px 8px;
    font-size: 12px;
    resize: vertical;
  }
</style>
