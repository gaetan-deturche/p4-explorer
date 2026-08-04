<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { MergeData, MergeRegion } from "$lib/p4";

  // Opened by the Rust `open_merge_window` command; the job itself is fetched by
  // id (regions don't fit in a query string).
  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  type Pick = "theirs" | "ours" | "both" | "base" | "custom";

  let data = $state<MergeData | null>(null);
  let error = $state("");
  let saving = $state(false);
  /** Per-conflict resolution, keyed by region index. Unset = still to settle. */
  let picks = $state<Record<number, Pick>>({});
  let custom = $state<Record<number, string>>({});
  let editing = $state<number | null>(null);
  let showBase = $state<Record<number, boolean>>({});
  let current = $state(0); // which conflict the prev/next buttons are on

  const regions = $derived(data?.regions ?? []);
  const conflicts = $derived(
    regions.map((r, i) => (r.kind === "conflict" ? i : -1)).filter((i) => i >= 0),
  );
  const unsettled = $derived(conflicts.filter((i) => !picks[i]));
  const resultLines = $derived(regions.flatMap((r, i) => linesFor(r, i)));

  /** The lines the middle (result) pane contributes for a region. */
  function linesFor(r: MergeRegion, i: number): string[] {
    if (r.kind !== "conflict") return r.lines;
    const p = picks[i];
    if (!p) return [];
    if (p === "custom") return (custom[i] ?? "").split("\n");
    if (p === "theirs") return r.theirs;
    if (p === "ours") return r.ours;
    if (p === "base") return r.base;
    return [...r.theirs, ...r.ours]; // "both": depot first, then the workspace
  }

  /** What a side pane shows: its own text, or the base where it didn't change. */
  function side(r: MergeRegion, which: "theirs" | "ours"): string[] {
    if (r.kind === "same") return r.lines;
    if (r.kind === "conflict") return r[which];
    if (r.kind === "both") return r.lines;
    return r.kind === which ? r.lines : r.base;
  }
  /** A side pane is the origin of this region's change. */
  function isSource(r: MergeRegion, which: "theirs" | "ours"): boolean {
    return r.kind === which || r.kind === "both";
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
      await invoke<string>("merge_save", { id, text: resultLines.join("\n") + "\n" });
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
      // Nothing is auto-settled: conflicts must be chosen deliberately. Open on
      // the first one so the window lands on the work to be done.
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
        {data.conflicts} conflict{data.conflicts === 1 ? "" : "s"}{data.conflicts
          ? ` · ${unsettled.length} still to settle`
          : ""} · {resultLines.length} lines
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
    <!-- One grid for headers AND content, so a scrollbar can never push the
         column titles out of line with their pane. -->
    <div class="scroll">
      <div class="grid">
        <div class="head">{data.theirsLabel}</div>
        <div class="head mid">result — merged file<span class="dim"> (base: {data.baseLabel})</span></div>
        <div class="head">{data.yoursLabel}</div>

        {#each regions as r, i (i)}
          {@const conflict = r.kind === "conflict"}
          <div class="cell {r.kind}" class:src={isSource(r, "theirs")}>
            {#if conflict}
              <button class="take" title="Take the depot side" onclick={() => set(i, "theirs")}>
                take ▶
              </button>
            {/if}
            {#each side(r, "theirs") as line}
              <div class="line mono">{line || " "}</div>
            {/each}
          </div>

          <div class="cell mid {r.kind}" data-region={i}>
            {#if conflict}
              <div class="picks">
                <span class="cnum">conflict {conflicts.indexOf(i) + 1}</span>
                <button class:on={picks[i] === "theirs"} onclick={() => set(i, "theirs")}>
                  depot
                </button>
                <button class:on={picks[i] === "ours"} onclick={() => set(i, "ours")}>
                  workspace
                </button>
                <button class:on={picks[i] === "both"} onclick={() => set(i, "both")}>both</button>
                <button class:on={picks[i] === "base"} onclick={() => set(i, "base")}>base</button>
                <button class:on={picks[i] === "custom"} onclick={() => set(i, "custom")}>
                  edit…
                </button>
                <button
                  class="peek"
                  class:on={showBase[i]}
                  title="Show the common ancestor for this conflict"
                  onclick={() => (showBase = { ...showBase, [i]: !showBase[i] })}
                >
                  base?
                </button>
              </div>
              {#if showBase[i] && r.kind === "conflict"}
                <div class="baseblock">
                  {#each r.base as line}
                    <div class="line mono dim">{line || " "}</div>
                  {/each}
                </div>
              {/if}
            {/if}

            {#if conflict && editing === i}
              <textarea
                class="mono"
                rows={Math.min(24, (custom[i] ?? "").split("\n").length + 1)}
                value={custom[i] ?? ""}
                oninput={(e) => (custom = { ...custom, [i]: e.currentTarget.value })}
              ></textarea>
            {:else if conflict && !picks[i]}
              <div class="line pending">— take a side, or edit —</div>
            {:else}
              {#each linesFor(r, i) as line}
                <div class="line mono">{line || " "}</div>
              {/each}
            {/if}
          </div>

          <div class="cell {r.kind}" class:src={isSource(r, "ours")}>
            {#if conflict}
              <button class="take left" title="Take the workspace side" onclick={() => set(i, "ours")}>
                ◀ take
              </button>
            {/if}
            {#each side(r, "ours") as line}
              <div class="line mono">{line || " "}</div>
            {/each}
          </div>
        {/each}
      </div>
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
  .scroll {
    flex: 1;
    overflow: auto;
    min-height: 0;
  }
  .grid {
    display: grid;
    /* A third of the viewport each, growing for long lines. One scrollbar on the
       container then pans all three panes together, so rows stay lined up. */
    grid-template-columns: repeat(3, minmax(33.333%, max-content));
    align-items: stretch;
  }
  .head {
    position: sticky;
    top: 0;
    z-index: 2;
    padding: 5px 8px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim, #999);
    background: var(--bg-alt, #1f1f1f);
    border-bottom: 1px solid var(--border, #333);
    border-right: 1px solid var(--border, #333);
    white-space: nowrap;
  }
  .head.mid {
    color: var(--text, #ddd);
  }
  .cell {
    position: relative;
    border-right: 1px solid var(--border, #333);
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
    min-width: 0;
  }
  /* The middle pane is the result: keep it visually the subject. */
  .cell.mid {
    background: rgba(255, 255, 255, 0.02);
  }
  .cell.theirs.src {
    background: rgba(106, 154, 215, 0.1);
  }
  .cell.ours.src {
    background: rgba(108, 195, 108, 0.1);
  }
  .cell.both.src {
    background: rgba(180, 180, 180, 0.07);
  }
  .cell.conflict {
    background: rgba(215, 106, 106, 0.1);
    box-shadow: inset 0 0 0 1px rgba(215, 106, 106, 0.35);
  }
  .cell.mid.conflict {
    background: rgba(215, 106, 106, 0.06);
  }
  .line {
    padding: 0 8px;
    white-space: pre;
    line-height: 1.45;
  }
  .line.pending {
    color: var(--danger, #d76a6a);
    font-style: italic;
  }
  .picks {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
    padding: 3px 6px;
    background: rgba(255, 255, 255, 0.05);
  }
  .cnum {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim, #999);
  }
  .baseblock {
    border-left: 2px solid var(--border, #333);
    margin: 2px 0 2px 6px;
  }
  /* Inline "take this side" affordance on a conflict's side panes. */
  .take {
    position: absolute;
    top: 2px;
    right: 4px;
    z-index: 1;
    opacity: 0.75;
    font-size: 10px;
    padding: 0 5px;
  }
  .take.left {
    right: auto;
    left: 4px;
  }
  .take:hover {
    opacity: 1;
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
  .peek {
    margin-left: auto;
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
