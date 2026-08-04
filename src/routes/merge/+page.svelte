<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import type { MergeData, MergeRegion } from "$lib/p4";

  // Opened by the Rust `open_merge_window` command; the job itself is fetched by
  // id (regions don't fit in a query string).
  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  type Pick = "theirs" | "ours" | "both" | "base" | "custom";
  type Side = "theirs" | "ours";

  let data = $state<MergeData | null>(null);
  let error = $state("");
  let saving = $state(false);
  /** Per-conflict resolution, keyed by region index. Unset = still to settle. */
  let picks = $state<Record<number, Pick>>({});
  let custom = $state<Record<number, string>>({});
  let editing = $state<number | null>(null);
  let showBase = $state<Record<number, boolean>>({});
  let current = $state(0); // which conflict the prev/next buttons are on
  /** line text → colored runs, from tokenizing all three sides once. */
  let tokens = $state<Map<string, TokenRun[]>>(new Map());

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
  function side(r: MergeRegion, which: Side): string[] {
    if (r.kind === "same" || r.kind === "both") return r.lines;
    if (r.kind === "conflict") return r[which];
    return r.kind === which ? r.lines : r.base;
  }

  // --- add / remove, the same meaning in every pane -------------------------
  // "add" = text the merge keeps, "del" = base text it drops, "!" = contested.
  // Which SIDE a change came from is shown by the arrows, not by the colour.
  function sideKind(r: MergeRegion, which: Side): string {
    if (r.kind === "same") return "";
    if (r.kind === "conflict") return "vs";
    if (r.kind === "both") return "add";
    return r.kind === which ? "add" : "del";
  }
  function resultKind(r: MergeRegion, i: number): string {
    if (r.kind === "same") return "";
    if (r.kind !== "conflict") return "add";
    const p = picks[i];
    if (!p) return "";
    return p === "base" ? "keep" : "add";
  }
  const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };

  /** Which side(s) feed the result here — drawn as arrows in the link columns. */
  function flows(r: MergeRegion, i: number): { left: boolean; right: boolean; open: boolean } {
    if (r.kind === "same") return { left: false, right: false, open: false };
    if (r.kind === "both") return { left: true, right: true, open: false };
    if (r.kind !== "conflict") {
      return { left: r.kind === "theirs", right: r.kind === "ours", open: false };
    }
    const p = picks[i];
    if (!p) return { left: false, right: false, open: true }; // undecided
    return {
      left: p === "theirs" || p === "both",
      right: p === "ours" || p === "both",
      open: false,
    };
  }

  // First line number of each region, per pane: the panes are whole files, so
  // each one's numbering runs continuously across regions.
  const starts = $derived.by(() => {
    let t = 1,
      o = 1,
      m = 1;
    return regions.map((r, i) => {
      const at = { t, o, m };
      t += side(r, "theirs").length;
      o += side(r, "ours").length;
      m += linesFor(r, i).length;
      return at;
    });
  });

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

  /** Colour every line once, from all three sides — identical text tokenizes the
   *  same, so one map serves the side panes AND the assembled result. */
  async function highlight(d: MergeData) {
    const lang = langForFile(d.name);
    if (!lang) return;
    const dark = !window.matchMedia?.("(prefers-color-scheme: light)").matches;
    const sides: string[][] = [[], [], []];
    for (const r of d.regions) {
      sides[0].push(...(r.kind === "conflict" ? r.theirs : r.lines));
      sides[1].push(...(r.kind === "conflict" ? r.ours : r.lines));
      if (r.kind !== "same") sides[2].push(...r.base);
    }
    const map = new Map<string, TokenRun[]>();
    for (const lines of sides) {
      const runs = await tokenizeLines(lines.join("\n"), lang, dark);
      if (!runs) continue;
      lines.forEach((l, i) => {
        if (runs[i] && !map.has(l)) map.set(l, runs[i]);
      });
    }
    tokens = map;
  }

  onMount(async () => {
    try {
      data = await invoke<MergeData>("merge_data", { id });
      // Nothing is auto-settled: conflicts must be chosen deliberately. Open on
      // the first one so the window lands on the work to be done.
      if (data.conflicts > 0) setTimeout(() => goTo(0), 0);
      void highlight(data);
    } catch (e) {
      error = String(e);
    }
  });
</script>

<!-- `from` = this pane's first line number; 0 for text that is in no file (the
     base peek), which keeps an empty gutter so code stays aligned. -->
{#snippet code(lines: string[], from: number, kind: string)}
  {#each lines as line, k}
    <div class="line k-{kind}"><span class="mk">{MARK[kind] ?? ""}</span><span class="ln"
        >{from ? from + k : ""}</span
      ><span class="src"
        >{#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
{/snippet}

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
      <span class="legend dim">
        <span class="chip add">+ kept</span><span class="chip del">− dropped</span><span
          class="chip vs">! conflict</span
        >
        <span class="lgd">▶ ◀ origin</span>
      </span>
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
    <!-- Headers live in the same grid as the content, so a scrollbar can never
         push a column out of line with its title. The two narrow link columns
         carry the arrows that say which side a region came from. -->
    <div class="scroll">
      <div class="grid mono">
        <div class="head">{data.theirsLabel}</div>
        <div class="head link"></div>
        <div class="head mid">
          result — merged file<span class="dim"> (base: {data.baseLabel})</span>
        </div>
        <div class="head link"></div>
        <div class="head">{data.yoursLabel}</div>

        {#each regions as r, i (i)}
          {@const conflict = r.kind === "conflict"}
          {@const flow = flows(r, i)}
          <div class="cell">
            {#if conflict}<div class="chead side"></div>{/if}
            {@render code(side(r, "theirs"), starts[i].t, sideKind(r, "theirs"))}
          </div>

          <!-- origin arrow: depot → result -->
          <div class="cell link" class:on={flow.left}>
            {#if conflict}<div class="chead side"></div>{/if}
            {#if flow.left}
              <div class="arrow" title="This region comes from the depot side">▶</div>
            {:else if flow.open}
              <div class="arrow open" title="Undecided conflict">?</div>
            {/if}
          </div>

          <div class="cell mid">
            {#if conflict}
              <div class="chead" data-region={i}>
                <span class="cnum">conflict {conflicts.indexOf(i) + 1}</span>
                <button class:on={picks[i] === "theirs"} onclick={() => set(i, "theirs")}>
                  ◀ depot
                </button>
                <button class:on={picks[i] === "ours"} onclick={() => set(i, "ours")}>
                  workspace ▶
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
                  {showBase[i] ? "hide base" : "base"}
                </button>
              </div>
            {/if}
            {#if conflict && showBase[i] && r.kind === "conflict"}
              <div class="baseblock">{@render code(r.base, 0, "keep")}</div>
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
              {@render code(linesFor(r, i), starts[i].m, resultKind(r, i))}
            {/if}
          </div>

          <!-- origin arrow: workspace → result -->
          <div class="cell link" class:on={flow.right}>
            {#if conflict}<div class="chead side"></div>{/if}
            {#if flow.right}
              <div class="arrow" title="This region comes from the workspace side">◀</div>
            {:else if flow.open}
              <div class="arrow open" title="Undecided conflict">?</div>
            {/if}
          </div>

          <div class="cell">
            {#if conflict}<div class="chead side"></div>{/if}
            {@render code(side(r, "ours"), starts[i].o, sideKind(r, "ours"))}
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
  .legend {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 10px;
  }
  .chip {
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid currentColor;
  }
  .chip.add {
    color: #7cc47c;
  }
  .chip.del {
    color: #d9873a;
  }
  .chip.vs {
    color: #e0555a;
  }
  .lgd {
    margin-left: 4px;
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
    /* pane | link | result | link | pane */
    grid-template-columns: minmax(0, 1fr) 1.4rem minmax(0, 1fr) 1.4rem minmax(0, 1fr);
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
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .head.mid {
    color: var(--text, #ddd);
  }
  .head.link {
    padding: 0;
  }
  .cell {
    border-right: 1px solid var(--border, #333);
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
    min-width: 0;
  }
  .cell.mid {
    background: rgba(255, 255, 255, 0.02);
  }
  /* Link columns: the visual connection between a side and the result. */
  .cell.link {
    background: var(--bg-alt, #1f1f1f);
    text-align: center;
  }
  .cell.link.on {
    background: rgba(124, 196, 124, 0.12);
  }
  .arrow {
    color: #7cc47c;
    line-height: 1.45;
  }
  .arrow.open {
    color: #e0555a;
    font-weight: 600;
  }
  .line {
    display: flex;
    align-items: flex-start;
    line-height: 1.45;
    border-left: 3px solid transparent;
  }
  /* Wrap long lines inside their own pane instead of bleeding into the next. */
  .src {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding-right: 6px;
    min-width: 0;
  }
  .mk {
    flex: none;
    width: 1em;
    text-align: center;
    user-select: none;
  }
  .ln {
    flex: none;
    width: 3.2em;
    padding-right: 8px;
    text-align: right;
    color: var(--text-dim, #999);
    opacity: 0.55;
    user-select: none;
  }
  /* Colour says add / remove, exactly as in a diff — never which side. */
  .k-add {
    background: rgba(108, 195, 108, 0.15);
    border-left-color: #5faf5f;
  }
  .k-add .mk {
    color: #7cc47c;
  }
  .k-del {
    background: rgba(217, 135, 58, 0.14);
    border-left-color: #d9873a;
    opacity: 0.8;
  }
  .k-del .mk {
    color: #d9873a;
  }
  .k-vs {
    background: rgba(224, 85, 90, 0.2);
    border-left-color: #e0555a;
  }
  .k-vs .mk {
    color: #e0555a;
  }
  .k-keep {
    background: rgba(180, 180, 180, 0.08);
    border-left-color: #6d6d6d;
  }
  .k-keep .mk {
    color: var(--text-dim, #999);
  }
  .line.pending {
    color: #e0555a;
    font-style: italic;
    padding-left: 8px;
  }
  .chead {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: nowrap;
    min-height: 24px;
    padding: 2px 6px;
    box-sizing: border-box;
    background: rgba(224, 85, 90, 0.2);
    border-top: 1px solid rgba(224, 85, 90, 0.55);
  }
  .chead.side {
    background: rgba(224, 85, 90, 0.11);
    padding: 2px 0;
  }
  .cnum {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim, #999);
    white-space: nowrap;
  }
  .baseblock {
    opacity: 0.85;
  }
  button {
    background: var(--bg-alt, #1f1f1f);
    color: inherit;
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 11px;
    cursor: pointer;
    white-space: nowrap;
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
