<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import MergeResult from "$lib/components/MergeResult.svelte";
  import OverviewRuler, { type Mark } from "$lib/components/OverviewRuler.svelte";
  import {
    clampCaret,
    deleteBackward,
    deleteForward,
    docText,
    emptyHistory,
    insertLineBreak,
    insertText,
    moveLeft,
    moveLineEnd,
    moveLineStart,
    moveRight,
    moveVertical,
    deleteRange,
    deleteWord,
    insertOverRange,
    lineRange,
    moveByLines,
    wordLeft,
    wordRange,
    wordRight,
    push,
    redo,
    sameCaret,
    selectAll,
    selectedText,
    setRegionLines,
    undo,
    type Caret,
    type DocState,
    type History,
    type MergeAction,
  } from "$lib/mergedoc";
  import { setClipboard } from "$lib/p4";
  import type { MergeData, MergeRegion } from "$lib/p4";

  // Opened by the Rust `open_merge_window` command; the job itself is fetched by
  // id (regions don't fit in a query string).
  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  type Side = "theirs" | "ours";

  const LH = 17.4; // one row: 12px * 1.45, the unit every pane measures in
  const TOOLBAR = 24;

  let data = $state<MergeData | null>(null);
  let error = $state("");
  let saving = $state(false);
  /** The result document: regions that own their lines. */
  let ds = $state<DocState | null>(null);
  let hist = $state<History>(emptyHistory());
  /** The fixed end of the selection; null when there is none. */
  let anchor = $state<Caret | null>(null);
  /** Region → where its text came from; also marks a conflict as settled. */
  let origin = $state<Record<number, string>>({});
  let current = $state(0); // which conflict the prev/next buttons are on
  let tokens = $state<Map<string, TokenRun[]>>(new Map());
  let typing = false; // coalesce consecutive typing into one undo step

  const regions = $derived(data?.regions ?? []);
  const conflicts = $derived(
    regions.map((r, i) => (r.kind === "conflict" ? i : -1)).filter((i) => i >= 0),
  );
  const unsettled = $derived(conflicts.filter((i) => origin[i] === undefined));

  /** What a side pane shows: its own text, or the base where it didn't change. */
  function side(r: MergeRegion, which: Side): string[] {
    if (r.kind === "same" || r.kind === "both") return r.lines;
    if (r.kind === "conflict") return r[which];
    return r.kind === which ? r.lines : r.base;
  }

  // --- add / drop, the same meaning in every pane ---------------------------
  function sideKind(r: MergeRegion, which: Side): string {
    if (r.kind === "same") return "";
    if (r.kind === "conflict") return "vs";
    if (r.kind === "both") return "add";
    return r.kind === which ? "add" : "del";
  }
  const kinds = $derived(
    regions.map((r, i) => {
      // A conflict stays red once a side is taken: the region is still a conflict
      // until the merge is saved, and green would read as "settled and done".
      // Progress is shown by the counter and the marker strip instead.
      if (r.kind === "conflict") return "vs";
      if (r.kind === "same" && origin[i] === undefined) return "";
      return origin[i] === "base" ? "keep" : "add";
    }),
  );
  const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };

  /** Which side(s) feed a region — the arrows in the link columns. */
  function flows(r: MergeRegion, i: number): { left: boolean; right: boolean; open: boolean } {
    const o = origin[i];
    if (o) {
      return { left: o === "theirs" || o === "both", right: o === "ours" || o === "both", open: false };
    }
    if (r.kind === "conflict") return { left: false, right: false, open: true };
    if (r.kind === "same") return { left: false, right: false, open: false };
    if (r.kind === "both") return { left: true, right: true, open: false };
    return { left: r.kind === "theirs", right: r.kind === "ours", open: false };
  }

  // --- alignment ------------------------------------------------------------
  // One row per line in every pane, so a region simply occupies as many rows as
  // its tallest side. Pure arithmetic, shared by all three panes: nothing to
  // measure, so nothing can drift.
  const rows = $derived(
    regions.map((r, i) =>
      Math.max(ds?.doc.regions[i]?.lines.length ?? 0, side(r, "theirs").length, side(r, "ours").length),
    ),
  );
  const tops = $derived.by(() => {
    let y = 0;
    return regions.map((r, i) => {
      const at = y;
      y += rows[i] * LH + (r.kind === "conflict" ? TOOLBAR : 0);
      return at;
    });
  });
  const total = $derived(
    regions.reduce((sum, r, i) => sum + rows[i] * LH + (r.kind === "conflict" ? TOOLBAR : 0), 0),
  );
  /** First line number of each region, per pane (each pane is its own file). */
  const starts = $derived.by(() => {
    let t = 1,
      o = 1,
      m = 1;
    return regions.map((r, i) => {
      const at = { t, o, m };
      t += side(r, "theirs").length;
      o += side(r, "ours").length;
      m += ds?.doc.regions[i]?.lines.length ?? 0;
      return at;
    });
  });

  // --- editing --------------------------------------------------------------
  /** Apply an intent from the result pane to the model, recording undo. */
  /** A live selection, or null. */
  function selection(): { from: Caret; to: Caret } | null {
    if (!ds || !anchor || sameCaret(anchor, ds.caret)) return null;
    return { from: anchor, to: ds.caret };
  }
  /** Every edit clears the selection: it has been consumed or replaced. */
  function apply(a: MergeAction) {
    if (!ds) return;
    const before = ds;
    const sel = selection();
    switch (a.t) {
      case "insert":
        hist = push(hist, before, typing && !sel);
        typing = true;
        ds = sel
          ? insertOverRange(before, sel.from, sel.to, a.text)
          : insertText(before, a.text);
        anchor = null;
        touched(before.caret.region);
        break;
      case "enter":
        hist = push(hist, before, false);
        typing = false;
        ds = sel ? insertOverRange(before, sel.from, sel.to, "\n") : insertLineBreak(before);
        anchor = null;
        touched(before.caret.region);
        break;
      case "backspace":
        hist = push(hist, before, false);
        typing = false;
        ds = sel ? deleteRange(before, sel.from, sel.to) : deleteBackward(before);
        anchor = null;
        touched(before.caret.region);
        break;
      case "delete":
        hist = push(hist, before, false);
        typing = false;
        ds = sel ? deleteRange(before, sel.from, sel.to) : deleteForward(before);
        anchor = null;
        touched(before.caret.region);
        break;
      case "move": {
        typing = false;
        const c = before.caret;
        const next =
          a.dir === "left"
            ? moveLeft(before.doc, c)
            : a.dir === "right"
              ? moveRight(before.doc, c)
              : a.dir === "up"
                ? moveVertical(before.doc, c, -1)
                : a.dir === "down"
                  ? moveVertical(before.doc, c, 1)
                  : a.dir === "home"
                    ? moveLineStart(before.doc, c)
                    : a.dir === "end"
                      ? moveLineEnd(before.doc, c)
                      : a.dir === "wordLeft"
                        ? wordLeft(before.doc, c)
                        : wordRight(before.doc, c);
        if (a.extend) anchor = anchor ?? before.caret;
        else anchor = null;
        ds = { doc: before.doc, caret: next };
        break;
      }
      case "caret":
        typing = false;
        if (a.extend) anchor = anchor ?? before.caret;
        else anchor = null;
        ds = { doc: before.doc, caret: clampCaret(before.doc, a.caret) };
        break;
      case "moveLines": {
        typing = false;
        if (a.extend) anchor = anchor ?? before.caret;
        else anchor = null;
        ds = { doc: before.doc, caret: moveByLines(before.doc, before.caret, a.delta) };
        break;
      }
      case "selectWord": {
        typing = false;
        const w = wordRange(before.doc, a.caret);
        anchor = w.from;
        ds = { doc: before.doc, caret: w.to };
        break;
      }
      case "selectLine": {
        typing = false;
        const l = lineRange(before.doc, a.caret);
        anchor = l.from;
        ds = { doc: before.doc, caret: l.to };
        break;
      }
      case "deleteWord": {
        hist = push(hist, before, false);
        typing = false;
        ds = sel ? deleteRange(before, sel.from, sel.to) : deleteWord(before, a.forward);
        anchor = null;
        touched(before.caret.region);
        break;
      }
      case "selectAll": {
        typing = false;
        const all = selectAll(before.doc);
        if (all) {
          anchor = all.anchor;
          ds = { doc: before.doc, caret: all.head };
        }
        break;
      }
      case "copy": {
        typing = false;
        if (!sel) break;
        const text = selectedText(before.doc, sel.from, sel.to);
        void setClipboard(text).catch((e) => (error = String(e)));
        if (a.cut) {
          hist = push(hist, before, false);
          ds = deleteRange(before, sel.from, sel.to);
          anchor = null;
          touched(before.caret.region);
        }
        break;
      }
      case "undo": {
        typing = false;
        const u = undo(hist, before);
        if (u) {
          ds = u.state;
          hist = u.history;
          anchor = null;
        }
        break;
      }
      case "save":
        void save();
        break;
      case "redo": {
        typing = false;
        const r = redo(hist, before);
        if (r) {
          ds = r.state;
          hist = r.history;
          anchor = null;
        }
        break;
      }
    }
  }
  /** Any edit inside a region settles it and marks it hand-edited. */
  function touched(region: number) {
    if (origin[region] !== "manual") origin = { ...origin, [region]: "manual" };
  }

  /** Copy a side's text into a conflict; it stays editable afterwards. */
  function take(i: number, what: "theirs" | "ours" | "both" | "base") {
    if (!ds) return;
    const r = regions[i];
    if (!r || r.kind !== "conflict") return;
    const lines =
      what === "both" ? [...r.theirs, ...r.ours] : what === "base" ? r.base : r[what];
    hist = push(hist, ds, false);
    typing = false;
    ds = setRegionLines(ds, i, lines);
    anchor = null;
    origin = { ...origin, [i]: what };
  }
  /** Back to an undecided conflict. */
  function reset(i: number) {
    if (!ds) return;
    hist = push(hist, ds, false);
    typing = false;
    ds = setRegionLines(ds, i, []);
    anchor = null;
    const next = { ...origin };
    delete next[i];
    origin = next;
  }

  let scrollEl: HTMLDivElement | undefined = $state();
  /** The native scrollbar's width, so the ruler sits beside it rather than under. */
  const barWidth = $derived(scrollEl ? scrollEl.offsetWidth - scrollEl.clientWidth : 0);
  /** A tick for every region that is not identical on both sides — conflicts red
   *  until settled, and the auto-merged changes in the panes' own colours, so the
   *  strip is a map of the whole merge rather than only of its conflicts. */
  const marks = $derived<Mark[]>(
    regions
      .map((r, i) => ({ r, i }))
      .filter(({ r }) => r.kind !== "same")
      .map(({ r, i }) => {
        const pct = total ? tops[i] / total : 0;
        if (r.kind === "conflict") {
          return {
            pct,
            kind: origin[i] === undefined ? ("conflict" as const) : ("done" as const),
            title:
              `conflict ${conflicts.indexOf(i) + 1}` +
              (origin[i] === undefined ? " — still to settle" : ` — ${origin[i]}`),
            index: i,
          };
        }
        const lines = ds?.doc.regions[i]?.lines ?? [];
        const base = "base" in r ? r.base : [];
        // Kept where the result has lines, dropped where the base had them and the
        // result does not, both when one replaced the other.
        const kind = !lines.length ? ("del" as const) : !base.length ? ("add" as const) : ("mod" as const);
        const what = kind === "add" ? "added" : kind === "del" ? "removed" : "changed";
        const from = r.kind === "theirs" ? "depot" : r.kind === "ours" ? "workspace" : "both sides";
        return { pct, kind, title: `${what} — from ${from}`, index: i };
      }),
  );
  /** Alt+Up / Alt+Down step through the changes from anywhere in the window. */
  function onWindowKey(e: KeyboardEvent) {
    if (!e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    goTo(current + (e.key === "ArrowDown" ? 1 : -1));
  }
  /** Scroll a region into view; conflicts also move the prev/next counter. */
  function jumpTo(i: number) {
    const at = conflicts.indexOf(i);
    if (at >= 0) {
      goTo(at);
      return;
    }
    if (scrollEl) scrollEl.scrollTop = Math.max(0, tops[i] - LH * 3);
  }
  function seek(fraction: number) {
    if (!scrollEl) return;
    scrollEl.scrollTop = fraction * (scrollEl.scrollHeight - scrollEl.clientHeight);
  }

  function goTo(n: number) {
    if (!conflicts.length) return;
    current = ((n % conflicts.length) + conflicts.length) % conflicts.length;
    document
      .querySelector(`[data-region="${conflicts[current]}"]`)
      ?.scrollIntoView({ block: "center" });
  }

  /** Conflicts settled by editing that ended up with no text at all: legitimate
   *  ("drop this code") but the most likely shape of an accident, so it is said
   *  out loud in the header rather than only counted as settled. */
  const emptied = $derived(
    conflicts.filter((i) => origin[i] !== undefined && !(ds?.doc.regions[i]?.lines.length ?? 0)),
  );

  /** Re-checked at save time, not only when the button is drawn: whatever the UI
   *  state, these must never reach the file. */
  function saveProblem(text: string): string | null {
    if (unsettled.length) {
      const which = unsettled.map((i) => conflicts.indexOf(i) + 1).join(", ");
      return `Conflict ${which} still to settle — nothing was written.`;
    }
    const at = text.split("\n").findIndex((l) => /^(<{7}|={7}|>{7})/.test(l));
    if (at >= 0) {
      return `Conflict markers on line ${at + 1} — remove them before saving.`;
    }
    return null;
  }

  async function save() {
    if (!data || !ds || unsettled.length) return;
    const problem = saveProblem(docText(ds.doc));
    if (problem) {
      error = problem;
      return;
    }
    saving = true;
    try {
      const text = docText(ds.doc);
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

  /** Colour every distinct line once; all three panes share the map. */
  async function recolor(d: MergeData) {
    const lang = langForFile(d.name);
    if (!lang) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const batches: string[][] = [[], [], [], []];
    for (const r of d.regions) {
      batches[0].push(...(r.kind === "conflict" ? r.theirs : r.lines));
      batches[1].push(...(r.kind === "conflict" ? r.ours : r.lines));
      if (r.kind !== "same") batches[2].push(...r.base);
    }
    batches[3] = ds ? ds.doc.regions.flatMap((r) => r.lines) : [];
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
      // The result starts as the auto-merge; conflicts start empty and unsettled.
      ds = {
        doc: {
          regions: data.regions.map((r, i) => ({
            region: i,
            kind: r.kind === "conflict" ? "vs" : r.kind === "same" ? "" : "add",
            conflict: r.kind === "conflict",
            lines: r.kind === "conflict" ? [] : r.lines.slice(),
          })),
        },
        caret: { region: 0, line: 0, col: 0 },
      };
      void recolor(data);
      if (data.conflicts > 0) setTimeout(() => goTo(0), 0);
    } catch (e) {
      error = String(e);
    }
  });
</script>

<!-- Read-only pane content: mark, line number, coloured code. -->
{#snippet pane(lines: string[], from: number, kind: string)}
  {#each lines as line, k}
    <div class="line k-{kind}"><span class="mk">{MARK[kind] ?? ""}</span><span class="ln"
        >{from + k}</span
      ><span class="src"
        >{#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
{/snippet}

<!-- The buttons over a conflict, rendered inside the result pane's own strip. -->
{#snippet toolbar(region: number)}
  <span class="cnum">conflict {conflicts.indexOf(region) + 1}</span>
  <button class:on={origin[region] === "theirs"} onclick={() => take(region, "theirs")}>
    ◀ depot
  </button>
  <button class:on={origin[region] === "ours"} onclick={() => take(region, "ours")}>
    workspace ▶
  </button>
  <button class:on={origin[region] === "both"} onclick={() => take(region, "both")}>both</button>
  <button class:on={origin[region] === "base"} onclick={() => take(region, "base")}>base</button>
  {#if origin[region] !== undefined}
    <button onclick={() => reset(region)} title="Back to an undecided conflict">reset</button>
  {/if}
{/snippet}

<svelte:window onkeydown={onWindowKey} />

<div class="wrap">
  <div class="bar">
    {#if data}
      <span class="name mono">{data.name}</span>
      <span class="dim">
        {data.conflicts} conflict{data.conflicts === 1 ? "" : "s"}{data.conflicts
          ? ` · ${unsettled.length} still to settle`
          : ""}{emptied.length
          ? ` · ${emptied.length} resolve${emptied.length === 1 ? "s" : ""} to nothing`
          : ""}
      </span>
      <span class="grow"></span>
      <span class="legend dim">
        <span class="chip add">+ kept</span><span class="chip del">− dropped</span><span
          class="chip vs">! conflict</span
        >
      </span>
      <!-- Navigation is conflict-only: the auto-merged changes need no visit. -->
      {#if conflicts.length}
        <button onclick={() => goTo(current - 1)} title="Previous conflict (alt+up)">▲</button>
        <span class="dim">{current + 1}/{conflicts.length}</span>
        <button onclick={() => goTo(current + 1)} title="Next conflict (alt+down)">▼</button>
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
  {:else if !data || !ds}
    <div class="dim pad">Loading…</div>
  {:else}
    <div class="viewport">
      <div class="scroll" bind:this={scrollEl}>
      <div class="grid mono">
        <div class="head">{data.theirsLabel}</div>
        <div class="head link"></div>
        <div class="head mid">
          result — merged file, editable<span class="dim"> (base: {data.baseLabel})</span>
        </div>
        <div class="head link"></div>
        <div class="head">{data.yoursLabel}</div>

        <!-- Every pane places its regions at the same y, from the same row counts. -->
        <div class="col" style="height:{total}px">
          {#each regions as r, i (i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              style="top:{tops[i]}px; height:{rows[i] * LH + (r.kind === 'conflict' ? TOOLBAR : 0)}px"
            >
              {#if r.kind === "conflict"}<div class="strip"></div>{/if}
              {@render pane(side(r, "theirs"), starts[i].t, sideKind(r, "theirs"))}
            </div>
          {/each}
        </div>

        <div class="col link" style="height:{total}px">
          {#each regions as r, i (i)}
            {@const flow = flows(r, i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              class:on={flow.left}
              style="top:{tops[i]}px; height:{rows[i] * LH + (r.kind === 'conflict' ? TOOLBAR : 0)}px"
            >
              {#if r.kind === "conflict"}<div class="strip"></div>{/if}
              {#if flow.left}
                <div class="arrow" title="This region's text came from the depot side">▶</div>
              {:else if flow.open}
                <div class="arrow open" title="Undecided conflict" data-region={i}>?</div>
              {/if}
            </div>
          {/each}
        </div>

        <div class="resultcol">
          <MergeResult
            docState={ds}
            {anchor}
            {rows}
            starts={starts.map((s) => s.m)}
            {kinds}
            {tokens}
            lineHeight={LH}
            toolbarHeight={TOOLBAR}
            {toolbar}
            onAction={apply}
          />
        </div>

        <div class="col link" style="height:{total}px">
          {#each regions as r, i (i)}
            {@const flow = flows(r, i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              class:on={flow.right}
              style="top:{tops[i]}px; height:{rows[i] * LH + (r.kind === 'conflict' ? TOOLBAR : 0)}px"
            >
              {#if r.kind === "conflict"}<div class="strip"></div>{/if}
              {#if flow.right}
                <div class="arrow" title="This region's text came from the workspace side">◀</div>
              {:else if flow.open}
                <div class="arrow open" title="Undecided conflict">?</div>
              {/if}
            </div>
          {/each}
        </div>

        <div class="col" style="height:{total}px">
          {#each regions as r, i (i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              style="top:{tops[i]}px; height:{rows[i] * LH + (r.kind === 'conflict' ? TOOLBAR : 0)}px"
            >
              {#if r.kind === "conflict"}<div class="strip"></div>{/if}
              {@render pane(side(r, "ours"), starts[i].o, sideKind(r, "ours"))}
            </div>
          {/each}
        </div>
        </div>
      </div>
      {#if marks.length}
        <OverviewRuler {marks} offsetRight={barWidth} onPick={jumpTo} onSeek={seek} />
      {/if}
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
  .err {
    padding: 10px;
    color: var(--warn, #d9a33a);
    white-space: pre-wrap;
  }
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .scroll {
    flex: 1;
    overflow: auto;
    min-height: 0;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 1.4rem minmax(0, 1fr) 1.4rem minmax(0, 1fr);
    align-items: start;
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
  .col {
    position: relative;
    border-right: 1px solid var(--border, #333);
    overflow: hidden;
    min-width: 0;
  }
  .col.link {
    background: var(--bg-alt, #1f1f1f);
    text-align: center;
  }
  .resultcol {
    background: rgba(255, 255, 255, 0.02);
    border-right: 1px solid var(--border, #333);
    overflow: hidden;
    min-width: 0;
  }
  .rgn {
    position: absolute;
    left: 0;
    right: 0;
    overflow: hidden;
  }
  .rgn.conflict {
    background: rgba(224, 85, 90, 0.12);
    box-shadow:
      inset 0 1px 0 rgba(224, 85, 90, 0.55),
      inset 0 -1px 0 rgba(224, 85, 90, 0.55);
  }
  .rgn.on {
    background: rgba(124, 196, 124, 0.12);
  }
  /* Reserves the height of the result pane's conflict toolbar. */
  .strip {
    height: 24px;
    background: rgba(224, 85, 90, 0.1);
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
    height: 17.4px;
    border-left: 3px solid transparent;
    box-sizing: border-box;
  }
  .src {
    white-space: pre;
    tab-size: 4; /* must match TAB_WIDTH */
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
  .cnum {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim, #999);
    white-space: nowrap;
  }
  button {
    background: var(--bg-alt, #1f1f1f);
    color: inherit;
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    padding: 0 7px;
    height: 18px;
    line-height: 16px;
    font-size: 11px;
    cursor: pointer;
    white-space: nowrap;
  }
  .bar button {
    height: auto;
    padding: 2px 8px;
    line-height: normal;
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
</style>
