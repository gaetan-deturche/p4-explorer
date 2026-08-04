<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import { createMergeEditor, type MergeEditor, type RegionSpec } from "$lib/mergeedit";
  import type { MergeData, MergeRegion } from "$lib/p4";

  // Opened by the Rust `open_merge_window` command; the job itself is fetched by
  // id (regions don't fit in a query string).
  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  type Side = "theirs" | "ours";

  let data = $state<MergeData | null>(null);
  let error = $state("");
  let saving = $state(false);
  /** Region → its text in the result. Set means settled; the toolbar buttons only
   *  seed it, and the editor reports every further keystroke back into it. */
  let edited = $state<Record<number, string>>({});
  /** Region → where its text came from, for the origin arrows. */
  let origin = $state<Record<number, string>>({});
  let current = $state(0); // which conflict the prev/next buttons are on
  /** line text → colored runs; the side panes are rendered, not editors. */
  let tokens = $state<Map<string, TokenRun[]>>(new Map());

  let host: HTMLDivElement | undefined = $state();
  let editor: MergeEditor | null = null;

  const regions = $derived(data?.regions ?? []);
  const conflicts = $derived(
    regions.map((r, i) => (r.kind === "conflict" ? i : -1)).filter((i) => i >= 0),
  );
  const unsettled = $derived(conflicts.filter((i) => edited[i] === undefined));

  /** The result text for a region: what was edited, else the auto-merge. An
   *  unsettled conflict contributes nothing but still occupies one line. */
  function resultText(r: MergeRegion, i: number): string {
    if (edited[i] !== undefined) return edited[i];
    return r.kind === "conflict" ? "" : r.lines.join("\n");
  }

  /** What a side pane shows: its own text, or the base where it didn't change. */
  function side(r: MergeRegion, which: Side): string[] {
    if (r.kind === "same" || r.kind === "both") return r.lines;
    if (r.kind === "conflict") return r[which];
    return r.kind === which ? r.lines : r.base;
  }

  // --- add / drop, the same meaning in every pane ---------------------------
  // "add" = text the merge keeps, "del" = base text it drops, "!" = contested.
  // Which SIDE a change came from is shown by the arrows, not by the colour.
  function sideKind(r: MergeRegion, which: Side): string {
    if (r.kind === "same") return "";
    if (r.kind === "conflict") return "vs";
    if (r.kind === "both") return "add";
    return r.kind === which ? "add" : "del";
  }
  function resultKind(r: MergeRegion, i: number): string {
    if (r.kind === "same" && edited[i] === undefined) return "";
    if (r.kind === "conflict" && edited[i] === undefined) return "vs";
    return origin[i] === "base" ? "keep" : "add";
  }
  const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };

  /** Which side(s) feed the result here — drawn as arrows in the link columns. */
  function flows(r: MergeRegion, i: number): { left: boolean; right: boolean; open: boolean } {
    const o = origin[i];
    if (o) {
      return {
        left: o === "theirs" || o === "both",
        right: o === "ours" || o === "both",
        open: false,
      };
    }
    if (r.kind === "conflict") return { left: false, right: false, open: true };
    if (r.kind === "same") return { left: false, right: false, open: false };
    if (r.kind === "both") return { left: true, right: true, open: false };
    return { left: r.kind === "theirs", right: r.kind === "ours", open: false };
  }

  // --- alignment ------------------------------------------------------------
  // No soft wrap anywhere, so every line is exactly one row: a region can be made
  // to occupy the same number of rows in all three panes by padding the shorter
  // ones. Purely arithmetic — no measuring, nothing to drift.
  const rowPlan = $derived.by(() =>
    regions.map((r, i) => {
      const res = Math.max(1, resultText(r, i).split("\n").length);
      const t = side(r, "theirs").length;
      const o = side(r, "ours").length;
      const rows = Math.max(res, t, o);
      return { rows, res, theirs: t, ours: o };
    }),
  );
  /** Side pane line numbers run continuously through their own file. */
  const starts = $derived.by(() => {
    let t = 1,
      o = 1;
    return regions.map((r) => {
      const at = { t, o };
      t += side(r, "theirs").length;
      o += side(r, "ours").length;
      return at;
    });
  });

  const specs = $derived<RegionSpec[]>(
    regions.map((r, i) => ({
      region: i,
      kind: resultKind(r, i),
      conflict: r.kind === "conflict",
      text: resultText(r, i),
    })),
  );

  /** Copy a side's text into the result; it stays editable afterwards. */
  function take(i: number, what: "theirs" | "ours" | "both" | "base") {
    const r = regions[i];
    if (!r || r.kind !== "conflict") return;
    const text =
      what === "both"
        ? [...r.theirs, ...r.ours].join("\n")
        : what === "base"
          ? r.base.join("\n")
          : r[what].join("\n");
    edited = { ...edited, [i]: text };
    origin = { ...origin, [i]: what };
    editor?.setRegions(nextSpecs({ ...edited, [i]: text }, { ...origin, [i]: what }));
  }
  /** Back to an undecided conflict / the merged text. */
  function reset(i: number) {
    const nextE = { ...edited };
    const nextO = { ...origin };
    delete nextE[i];
    delete nextO[i];
    edited = nextE;
    origin = nextO;
    editor?.setRegions(nextSpecs(nextE, nextO));
  }
  /** Specs for a given state — used when we rewrite the document ourselves. */
  function nextSpecs(e: Record<string, string>, o: Record<string, string>): RegionSpec[] {
    return regions.map((r, i) => {
      const text = e[i] !== undefined ? e[i] : r.kind === "conflict" ? "" : r.lines.join("\n");
      const kind =
        r.kind === "same" && e[i] === undefined
          ? ""
          : r.kind === "conflict" && e[i] === undefined
            ? "vs"
            : o[i] === "base"
              ? "keep"
              : "add";
      return { region: i, kind, conflict: r.kind === "conflict", text };
    });
  }

  function goTo(n: number) {
    if (!conflicts.length) return;
    current = ((n % conflicts.length) + conflicts.length) % conflicts.length;
    document
      .querySelector(`[data-region="${conflicts[current]}"]`)
      ?.scrollIntoView({ block: "center" });
  }

  async function save() {
    if (!data || unsettled.length || !editor) return;
    saving = true;
    try {
      const text = editor.view.state.doc.toString();
      await invoke<string>("merge_save", { id, text: text.endsWith("\n") ? text : text + "\n" });
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

  /** Colour the side panes. The editor highlights itself, with the same palette. */
  async function recolor(d: MergeData) {
    const lang = langForFile(d.name);
    if (!lang) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const batches: string[][] = [[], [], []];
    for (const r of d.regions) {
      batches[0].push(...(r.kind === "conflict" ? r.theirs : r.lines));
      batches[1].push(...(r.kind === "conflict" ? r.ours : r.lines));
      if (r.kind !== "same") batches[2].push(...r.base);
    }
    const map = new Map(tokens);
    for (const lines of batches) {
      if (!lines.some((l) => !map.has(l))) continue;
      try {
        const runs = await tokenizeLines(lines.join("\n"), lang, dark);
        if (!runs) continue;
        lines.forEach((l, i) => {
          if (runs[i] && !map.has(l)) map.set(l, runs[i]);
        });
      } catch {
        /* one batch failing must not cost the others their colour */
      }
    }
    tokens = map;
  }

  onMount(async () => {
    try {
      data = await invoke<MergeData>("merge_data", { id });
      void recolor(data);
    } catch (e) {
      error = String(e);
    }
  });

  // Mount the editor once the host element and the data are both there.
  $effect(() => {
    if (!host || !data || editor) return;
    editor = createMergeEditor(host, {
      regions: specs,
      onEdit: (region, text) => {
        edited = { ...edited, [region]: text };
        if (origin[region] !== "manual") origin = { ...origin, [region]: "manual" };
      },
      onTake: take,
      onReset: reset,
      conflictNumber: (region) => conflicts.indexOf(region) + 1,
      settled: (region) => edited[region] !== undefined,
    });
    if (conflicts.length) setTimeout(() => goTo(0), 0);
  });

  // Keep the editor's blank-row padding in step with the side panes.
  $effect(() => {
    if (!editor) return;
    const rows = new Map<number, number>();
    rowPlan.forEach((p, i) => rows.set(i, p.rows - p.res));
    editor.setSpacers(rows);
  });
  // Toolbar labels depend on host state (settled, conflict numbering).
  $effect(() => {
    void unsettled.length;
    editor?.touch();
  });

  $effect(() => () => editor?.destroy());
</script>

<!-- Read-only pane content: mark + line number + coloured code, then filler rows
     so the region occupies the same height as in the other panes. -->
{#snippet pane(lines: string[], from: number, kind: string, filler: number)}
  {#each lines as line, k}
    <div class="line k-{kind}"><span class="mk">{MARK[kind] ?? ""}</span><span class="ln"
        >{from + k}</span
      ><span class="src"
        >{#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
  {#each Array.from({ length: filler }) as _, k (k)}
    <div class="line filler">&nbsp;</div>
  {/each}
{/snippet}

<div class="wrap">
  <div class="bar">
    {#if data}
      <span class="name mono">{data.name}</span>
      <span class="dim">
        {data.conflicts} conflict{data.conflicts === 1 ? "" : "s"}{data.conflicts
          ? ` · ${unsettled.length} still to settle`
          : ""}
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
    <div class="scroll">
      <div class="grid mono">
        <div class="head">{data.theirsLabel}</div>
        <div class="head link"></div>
        <div class="head mid">
          result — merged file, editable<span class="dim"> (base: {data.baseLabel})</span>
        </div>
        <div class="head link"></div>
        <div class="head">{data.yoursLabel}</div>

        <!-- The result is ONE editor spanning every region row; the side panes
             are per-region cells padded to the same heights. -->
        <div class="editorcell" bind:this={host} style="grid-row:2 / span {regions.length}"></div>

        {#each regions as r, i (i)}
          {@const conflict = r.kind === "conflict"}
          {@const flow = flows(r, i)}
          {@const plan = rowPlan[i]}
          <div class="cell theirs" class:conflict
            style="grid-row:{i + 2}; height:calc({plan.rows} * var(--lh) + {conflict ? 24 : 0}px)"
          >
            {#if conflict}<div class="chead side"></div>{/if}
            {@render pane(
              side(r, "theirs"),
              starts[i].t,
              sideKind(r, "theirs"),
              plan.rows - plan.theirs,
            )}
          </div>

          <div class="cell link l" class:conflict class:on={flow.left}
            style="grid-row:{i + 2}; height:calc({plan.rows} * var(--lh) + {conflict ? 24 : 0}px)"
          >
            {#if conflict}<div class="chead side"></div>{/if}
            {#if flow.left}
              <div class="arrow" title="This region's text came from the depot side">▶</div>
            {:else if flow.open}
              <div class="arrow open" title="Undecided conflict" data-region={i}>?</div>
            {/if}
          </div>

          <div class="cell link r" class:conflict class:on={flow.right}
            style="grid-row:{i + 2}; height:calc({plan.rows} * var(--lh) + {conflict ? 24 : 0}px)"
          >
            {#if conflict}<div class="chead side"></div>{/if}
            {#if flow.right}
              <div class="arrow" title="This region's text came from the workspace side">◀</div>
            {:else if flow.open}
              <div class="arrow open" title="Undecided conflict">?</div>
            {/if}
          </div>

          <div class="cell ours" class:conflict
            style="grid-row:{i + 2}; height:calc({plan.rows} * var(--lh) + {conflict ? 24 : 0}px)"
          >
            {#if conflict}<div class="chead side"></div>{/if}
            {@render pane(side(r, "ours"), starts[i].o, sideKind(r, "ours"), plan.rows - plan.ours)}
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
    /* One row height everywhere — 12px * 1.45. The alignment maths and the
       editor's spacer widgets both measure in this unit, so it must be absolute. */
    --lh: 17.4px;
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
  /* Explicit columns: the editor spans every region row in column 3. */
  .cell.theirs {
    grid-column: 1;
  }
  .cell.link.l {
    grid-column: 2;
  }
  .cell.link.r {
    grid-column: 4;
  }
  .editorcell {
    grid-column: 3;
    background: rgba(255, 255, 255, 0.02);
    border-right: 1px solid var(--border, #333);
    overflow: hidden;
    min-width: 0;
  }
  .cell.ours {
    grid-column: 5;
  }
  .cell {
    border-right: 1px solid var(--border, #333);
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
    min-width: 0;
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: none; /* a visible bar would add height and skew the rows */
  }
  .cell::-webkit-scrollbar {
    display: none;
  }
  .cell.conflict {
    background: rgba(224, 85, 90, 0.12);
    border-top: 1px solid rgba(224, 85, 90, 0.55);
    border-bottom: 1px solid rgba(224, 85, 90, 0.55);
  }
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
    height: var(--lh);
  }
  .line.filler {
    border-left-color: transparent;
  }
  /* No wrapping: alignment depends on one line being exactly one row. */
  .src {
    white-space: pre;
    min-width: 0;
    padding-right: 6px;
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
  /* Colour says add / drop, exactly as in a diff — never which side. */
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
  .chead {
    display: flex;
    align-items: center;
    height: 24px;
    padding: 2px 6px;
    box-sizing: border-box;
    background: rgba(224, 85, 90, 0.1);
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
  .primary {
    border-color: var(--accent, #d98d3a);
    color: var(--accent, #d98d3a);
  }
</style>
