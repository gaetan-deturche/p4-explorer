<script lang="ts">
  //! The diff window: the same layout, palette and editing as the resolve window,
  //! with one reference side instead of two.
  //!
  //! A diff is expressed as the merge model: each run of same / removed / added
  //! lines becomes a region, the right side owns its lines, and a run present on
  //! only one side leaves a void on the other. Alignment is then the same single
  //! arithmetic pass both windows use, and the right side is editable exactly like
  //! a merge result — which is what makes "fix it while you are looking at it"
  //! possible on the workspace file.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { diffLines, lineEndings, lineKey, type DiffRow } from "$lib/linediff";
  import { endingLabel, visualize } from "$lib/invisibles";
  import { cacheGet, cacheSet } from "$lib/store.svelte";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import MergeResult from "$lib/components/MergeResult.svelte";
  import OverviewRuler, { type Mark } from "$lib/components/OverviewRuler.svelte";
  import {
    visualColumn,
    addCursorAtNextMatch,
    addCursorVertical,
    applyBackspace,
    applyCaret,
    applyCollapse,
    applyDelete,
    applyDeleteWord,
    applyEnter,
    applyInsert,
    applyMove,
    applyMoveLines,
    applyRegionLines,
    applySelectAll,
    applySelectLine,
    applySelectWord,
    copyText,
    hasSelection,
    primaryCaret,
    singleCursor,
    docText,
    emptyHistory,
    push,
    redo,
    undo,
    type Caret,
    type DocState,
    type History,
    type MergeAction,
  } from "$lib/mergedoc";
  import { setClipboard, writeLocalFile } from "$lib/p4";

  // Opened by the Rust `open_diff_window` command with both sides materialized.
  const params = new URLSearchParams(window.location.search);
  const leftPath = params.get("left") ?? "";
  const rightPath = params.get("right") ?? "";
  const leftLabel = params.get("ll") ?? "left";
  const rightLabel = params.get("rl") ?? "right";
  const title = params.get("title") ?? "";
  /** Only a workspace file can be edited; a printed revision cannot. */
  const editable = params.get("edit") === "1";
  /** Non-empty when the file still owes a resolve, i.e. this diff does NOT show
   *  the depot change that is about to land. Loud on purpose: acting on the diff
   *  in that state is how an incoming change gets deleted by hand. */
  const note = params.get("note") ?? "";

  const LH = 17.4; // one row: 12px * 1.45
  const TOOLBAR = 0; // the diff has no per-region toolbar

  /** One hunk: a run of unchanged rows, or a run of changed ones. */
  interface Block {
    kind: "same" | "change";
    left: string[];
    right: string[];
    leftFrom: number; // 1-based line number in the left file
  }

  let leftText = "";
  let blocks = $state<Block[]>([]);
  let ds = $state<DocState | null>(null);
  let hist = $state<History>(emptyHistory());

  // --- movable center split --------------------------------------------------
  // Fraction of the width given to the LEFT pane. Persisted once dragged; an
  // ADDED file (no previous version, left side empty) defaults to a sliver so
  // the real content isn't halved for the sake of a blank column.
  const SPLIT_MIN = 0.15;
  const SPLIT_MAX = 0.85;
  // Deliberately NOT persisted: every diff is a different file with a different
  // shape, so last window's split is not a useful default for this one — and one
  // shared key across windows is what let an added-file window collapse the
  // panes of every later diff. Drag applies to the window you are in.
  let split = $state(0.5);
  // An ADDED file has no previous version: one pane, no gutter. The hidden
  // columns stay in the DOM at zero width so the change-navigation anchors
  // (data-change) keep their positions.
  let single = $state(false);
  let gridEl = $state<HTMLDivElement>();
  /** An added file (empty left side) renders as a single pane; anything else
   *  opens evenly split. */
  function initSplit(leftEmpty: boolean) {
    single = leftEmpty;
  }
  /** The longest line in either side, in visual columns — tabs expanded exactly
   *  as the panes render them. This drives --content-w, and with it how far there
   *  is to pan. */
  const maxCols = $derived.by(() => {
    let n = 0;
    for (const b of blocks) {
      for (const l of b.left) n = Math.max(n, visualColumn(l, l.length));
      for (const l of b.right) n = Math.max(n, visualColumn(l, l.length));
    }
    return n;
  });

  // What there is to pan is a LAYOUT fact, so it is observed rather than guessed
  // at: the first attempt measured on a state change, which happens while the
  // grid is still behind the loading branch — it saw nothing to pan and never
  // looked again. A ResizeObserver catches the grid appearing, the split moving
  // and the window changing; `measurePan` is also called on the way into a pan,
  // so a stale number can never block one.
  $effect(() => {
    const ps = paneEls.filter((x): x is HTMLElement => !!x);
    if (!ps.length) return;
    const ro = new ResizeObserver(() => measurePan());
    for (const p of ps) ro.observe(p);
    measurePan();
    return () => ro.disconnect();
  });
  // Content can grow wider without any box being resized (a re-diff swaps the
  // lines inside panes that keep their width), so the blocks are watched too.
  $effect(() => {
    void blocks;
    void ds;
    void maxCols;
    requestAnimationFrame(measurePan);
  });

  const gridCols = $derived(
    single
      ? "0 0 minmax(0, 1fr)"
      : `minmax(0, ${split}fr) 1.6rem minmax(0, ${1 - split}fr)`,
  );
  // --- horizontal panning ----------------------------------------------------
  // A scrollbar under each pane, and the panes move together: side by side, the
  // panes show the same columns of two versions of a file, so panning one and not
  // the other would break the comparison. One bar stretched across both panels
  // was the first attempt and read as if the two were a single surface.
  //
  // The bars are ours rather than the panes' own, because a pane is as tall as the
  // file: the scrollbar it would grow sits at the bottom of the DOCUMENT, hundreds
  // of rows below the window. And the wheel is handled here, since a WebView2 does
  // not reliably turn shift+wheel into horizontal scroll.
  let paneEls = $state<(HTMLElement | undefined)[]>([]);
  let barEls = $state<(HTMLDivElement | undefined)[]>([]);
  let barWs = $state<number[]>([]);
  let panWs = $state<number[]>([]); // content width per pane
  let panViews = $state<number[]>([]); // visible width per pane
  /** The shared range: the panes move as one, so it is the widest need. */
  const panRange = $derived(
    Math.max(0, ...panWs.map((w, i) => w - (panViews[i] ?? 0)), 0),
  );
  const canPan = $derived(panRange > 1);
  let panning = false;

  function measurePan() {
    const w = paneEls.map((p) => p?.scrollWidth ?? 0);
    const v = paneEls.map((p) => p?.clientWidth ?? 0);
    // Only write when something actually moved. Assigning unconditionally fed the
    // ResizeObserver from its own results — measure, show the bar, layout changes,
    // observe, measure — which ran thousands of times for one open file.
    const same = (a: number[], b: number[]) =>
      a.length === b.length && a.every((n, i) => n === b[i]);
    if (same(w, panWs) && same(v, panViews)) return;
    panWs = w;
    panViews = v;
  }
  /** Spacer width giving bar `i` the shared scroll range.
   *
   *  Sizing it to the CONTENT width was the bug behind "no scrollbar at all": a
   *  bar is not as wide as the content it stands for, and a spacer narrower than
   *  its own bar leaves nothing to scroll. What must match is the range. */
  function spacerW(i: number): number {
    return (barWs[i] ?? 0) + panRange;
  }
  function setPan(x: number) {
    const to = Math.max(0, Math.min(x, panRange));
    panning = true;
    for (const p of paneEls) if (p && p.scrollLeft !== to) p.scrollLeft = to;
    for (const b of barEls) if (b && b.scrollLeft !== to) b.scrollLeft = to;
    requestAnimationFrame(() => (panning = false));
  }
  function onBarScroll(i: number) {
    if (panning) return;
    const bar = barEls[i];
    if (bar) setPan(bar.scrollLeft);
  }
  /** A pane scrolled itself (a trackpad gesture, a focus jump): follow it. */
  function onPaneScroll(i: number) {
    if (panning) return;
    const pane = paneEls[i];
    if (pane) setPan(pane.scrollLeft);
  }
  /** Shift+wheel, or a horizontal wheel/swipe. */
  function onPanWheel(e: WheelEvent) {
    const dx = e.deltaX !== 0 ? e.deltaX : e.shiftKey ? e.deltaY : 0;
    if (!dx) return;
    measurePan(); // a stale measurement must not refuse a pan
    if (!canPan) return;
    e.preventDefault(); // or the vertical scroller eats a shifted wheel
    setPan((paneEls[0]?.scrollLeft ?? 0) + dx);
  }

  function splitDown(e: PointerEvent) {
    // Only from the gutter background — the revert buttons stay clickable.
    if ((e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    const move = (ev: PointerEvent) => {
      if (!gridEl) return;
      const r = gridEl.getBoundingClientRect();
      split = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, (ev.clientX - r.left) / r.width));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  // --- view options ----------------------------------------------------------
  // Persisted through the store (authoritative: SQLite, with localStorage only as
  // an accelerator), because a preference that forgets itself between windows is
  // worse than no preference — the editor choice already taught us that.
  let ignoreWs = $state(false);
  let invisibles = $state(false);
  /** Line endings of the two sides, for the header. The diff strips CR before
   *  comparing lines, so a file that differs ONLY in line endings shows no diff
   *  at all — this is where that becomes visible instead of mysterious. */
  let endings = $state({ left: "", right: "" });

  async function loadOptions() {
    ignoreWs = (await cacheGet("nav", "diff-ignore-ws")) === "1";
    invisibles = (await cacheGet("nav", "diff-invisibles")) === "1";
  }
  function setIgnoreWs(v: boolean) {
    ignoreWs = v;
    cacheSet("nav", "diff-ignore-ws", v ? "1" : "0");
    // The comparison changed, so the blocks have to be recomputed from the text
    // that is on screen now (edits included).
    if (ds) rebuild(docText(ds.doc), absoluteLine());
  }
  function setInvisibles(v: boolean) {
    invisibles = v;
    cacheSet("nav", "diff-invisibles", v ? "1" : "0");
  }

  let dirty = $state(false);
  let saving = $state(false);
  let error = $state("");
  let loading = $state(true);
  let tokens = $state<Map<string, TokenRun[]>>(new Map());
  let current = $state(0);
  let typing = false;

  /** Do the two sides of block `i` currently agree? Under the same rule the diff
   *  itself used, or a block would count as changed while its rows showed as
   *  unchanged. */
  function agrees(i: number): boolean {
    const b = blocks[i];
    const right = ds?.doc.regions[i]?.lines ?? [];
    const opts = { ignoreWhitespace: ignoreWs };
    return (
      !!b &&
      right.length === b.left.length &&
      right.every((l, k) => lineKey(l, opts) === lineKey(b.left[k], opts))
    );
  }
  const settled = $derived(blocks.map((_, i) => agrees(i)));
  const changes = $derived(blocks.map((_, i) => (settled[i] ? -1 : i)).filter((i) => i >= 0));

  /** Group the aligned rows into hunks. Adjacent changed rows belong to ONE hunk
   *  whatever their individual types: a change where the sides have different line
   *  counts comes back from the diff as mod rows followed by add or del rows, and
   *  counting those separately reported one visible change as several. */
  function toBlocks(rows: DiffRow[]): Block[] {
    const out: Block[] = [];
    for (const row of rows) {
      const kind: Block["kind"] = row.type === "same" ? "same" : "change";
      const last = out[out.length - 1];
      if (last && last.kind === kind) {
        if (row.l) last.left.push(row.l.text);
        if (row.r) last.right.push(row.r.text);
        continue;
      }
      out.push({
        kind,
        left: row.l ? [row.l.text] : [],
        right: row.r ? [row.r.text] : [],
        leftFrom: row.l?.no ?? (last ? last.leftFrom + last.left.length : 1),
      });
    }
    return out;
  }

  /** Colour by add / drop, the same scale as the resolve window. */
  function leftKind(i: number): string {
    return settled[i] ? "" : "del";
  }
  const kinds = $derived(blocks.map((_, i) => (settled[i] ? "" : "add")));

  // Alignment: a block takes as many rows as its taller side.
  const rows = $derived(
    blocks.map((b, i) => Math.max(b.left.length, ds?.doc.regions[i]?.lines.length ?? 0)),
  );
  const tops = $derived.by(() => {
    let y = 0;
    return blocks.map((_, i) => {
      const at = y;
      y += rows[i] * LH;
      return at;
    });
  });
  const total = $derived(rows.reduce((sum, r) => sum + r * LH, 0));
  /** First line number of each block, per side (each side is its own file). */
  const starts = $derived.by(() => {
    let r = 1;
    return blocks.map((b, i) => {
      const at = { l: b.leftFrom, r };
      r += ds?.doc.regions[i]?.lines.length ?? 0;
      return at;
    });
  });

  /** Re-diff `rightText` against the left side and rebuild the regions. `keep` is
   *  an absolute line index in the right file, so the caret survives the new block
   *  structure. */
  function rebuild(rightText: string, caret: Caret | number, col = 0) {
initSplit(leftText.trim() === "");
    blocks = toBlocks(diffLines(leftText, rightText, { ignoreWhitespace: ignoreWs }));
    endings = { left: endingLabel(lineEndings(leftText)), right: endingLabel(lineEndings(rightText)) };
    const regions = blocks.map((b, i) => ({
      region: i,
      kind: b.kind === "same" ? "" : "add",
      conflict: false,
      lines: b.right.slice(),
    }));
    let target: Caret = { region: 0, line: 0, col: 0 };
    if (typeof caret === "number") {
      let n = caret;
      const put = (r: (typeof regions)[number], line: number) => ({
        region: r.region,
        line,
        col: Math.min(col, r.lines[line]?.length ?? 0),
      });
      for (const r of regions) {
        if (n < r.lines.length) {
          target = put(r, n);
          break;
        }
        n -= r.lines.length;
        target = put(r, Math.max(0, r.lines.length - 1));
      }
    } else {
      target = caret;
    }
    ds = singleCursor({ regions }, target);
  }

  /** Re-diff immediately after an edit.
   *
   *  Every marker, colour and the change count derive from `agrees(i)` — a
   *  whole-BLOCK comparison — and `blocks` only changes here. Since everything
   *  after the last real change is ONE "same" block, any edit inside it makes
   *  that whole block unequal, so the rest of the file paints as removed/added
   *  until the blocks are recomputed. Deferring that by even a few hundred
   *  milliseconds is visible as a flash of a whole-file diff, so it is
   *  SYNCHRONOUS: measured (Claude/bench-diff-reflow.ts) at 0.1ms for 1k lines,
   *  2.4ms for 20k and 4.8ms for 50k — inside one frame at every realistic size.
   *
   *  Skipped only while a selection is open, since re-blocking rebuilds the
   *  regions its endpoints point into; the next edit (or its collapse) reflows. */
  function reflow() {
    if (!ds || !editable || hasSelection(ds)) return;
    rebuild(docText(ds.doc), absoluteLine());
  }

  /** The caret's line counted from the top of the right file. */
  function absoluteLine(): number {
    if (!ds) return 0;
    const caret = primaryCaret(ds);
    let n = 0;
    for (const r of ds.doc.regions) {
      if (r.region === caret.region) return n + caret.line;
      n += r.lines.length;
    }
    return n;
  }

  let scrollEl: HTMLDivElement | undefined = $state();
  /** The native scrollbar's width, so the ruler sits beside it rather than under. */
  const barWidth = $derived(scrollEl ? scrollEl.offsetWidth - scrollEl.clientWidth : 0);
  /** One tick per block that still differs. */
  const marks = $derived<Mark[]>(
    blocks
      .map((b, i) => ({ b, i }))
      .filter(({ i }) => !settled[i])
      .map(({ b, i }) => {
        const right = ds?.doc.regions[i]?.lines ?? [];
        // Same reading as the panes: the local side gained it, lost it, or both.
        const kind = !b.left.length
          ? ("add" as const)
          : !right.length
            ? ("del" as const)
            : ("mod" as const);
        const what = kind === "add" ? "added" : kind === "del" ? "removed" : "changed";
        return {
          pct: total ? tops[i] / total : 0,
          kind,
          title: `${what} at line ${starts[i].r}`,
          index: i,
        };
      }),
  );
  /** Alt+Up / Alt+Down step through the changes from anywhere in the window. */
  function onWindowKey(e: KeyboardEvent) {
    // Whoever handled it first wins: the editor claims alt+shift+arrows for its
    // extra carets, and preventDefault does not stop the event bubbling to here.
    if (e.defaultPrevented) return;
    if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    goTo(current + (e.key === "ArrowDown" ? 1 : -1));
  }
  function jumpTo(i: number) {
    const at = changes.indexOf(i);
    if (at >= 0) goTo(at);
  }
  function seek(fraction: number) {
    if (!scrollEl) return;
    scrollEl.scrollTop = fraction * (scrollEl.scrollHeight - scrollEl.clientHeight);
  }

  function goTo(n: number) {
    if (!changes.length) return;
    current = ((n % changes.length) + changes.length) % changes.length;
    document.querySelector(`[data-change="${changes[current]}"]`)?.scrollIntoView({ block: "center" });
  }

  /** Actions that only look: caret, selection, copy. A read-only pane keeps
   *  these — it is the same editor, it just refuses to change the document. */
  const READ_ONLY_OK = new Set([
    "move",
    "moveLines",
    "caret",
    "selectWord",
    "selectLine",
    "selectAll",
    "copy",
    "addCursor",
    "addCursorMatch",
    "collapse",
  ]);

  /** Apply an intent to the document. Identical handling to the resolve window,
   *  because it is the same model. */
  function apply(a: MergeAction) {
    if (!ds) return;
    if (!editable) {
      // Dropping EVERY action here made a click scroll to the top: the caret
      // never moved, and the pane's reveal() then scrolled to where it still
      // was — line one. Navigation, selection and copy stay; mutations don't.
      if (!READ_ONLY_OK.has(a.t)) return;
      if (a.t === "copy" && a.cut) a = { ...a, cut: false }; // cut writes; degrade to copy
    }
    const before = ds;
    const edit = (next: DocState, coalesce = false) => {
      hist = push(hist, before, coalesce);
      ds = next;
      dirty = true;
      reflow();
    };
    switch (a.t) {
      case "insert":
        // Coalescing stops at a selection: replacing text is one step of its own.
        edit(applyInsert(before, a.text), typing && !hasSelection(before));
        typing = true;
        break;
      case "enter":
        typing = false;
        edit(applyEnter(before));
        break;
      case "backspace":
        typing = false;
        edit(applyBackspace(before));
        break;
      case "delete":
        typing = false;
        edit(applyDelete(before));
        break;
      case "deleteWord":
        typing = false;
        edit(applyDeleteWord(before, a.forward));
        break;
      case "move":
        typing = false;
        ds = applyMove(before, a.dir, a.extend);
        break;
      case "moveLines":
        typing = false;
        ds = applyMoveLines(before, a.delta, a.extend);
        break;
      case "caret":
        typing = false;
        ds = applyCaret(before, a.caret, { extend: a.extend, add: a.add });
        break;
      case "addCursor":
        typing = false;
        ds = addCursorVertical(before, a.dir);
        break;
      case "addCursorMatch":
        typing = false;
        ds = addCursorAtNextMatch(before);
        break;
      case "collapse":
        typing = false;
        ds = applyCollapse(before);
        break;
      case "selectWord":
        ds = applySelectWord(before, a.caret, a.add);
        break;
      case "selectLine":
        ds = applySelectLine(before, a.caret, a.add);
        break;
      case "selectAll":
        ds = applySelectAll(before);
        break;
      case "copy": {
        const text = copyText(before);
        if (!text) break;
        void setClipboard(text).catch((e) => (error = String(e)));
        // Cut is copy plus "delete what was selected", which is exactly what
        // backspace does to a selection — at every cursor.
        if (a.cut) edit(applyBackspace(before));
        break;
      }
      case "undo": {
        const u = undo(hist, before);
        if (u) {
          ds = u.state;
          hist = u.history;
          dirty = true;
          reflow(); // the restored doc predates the current blocks
        }
        break;
      }
      case "save":
        void save();
        break;
      case "redo": {
        const r = redo(hist, before);
        if (r) {
          ds = r.state;
          hist = r.history;
          dirty = true;
          reflow(); // the restored doc predates the current blocks
        }
        break;
      }
    }
  }

  /** Discard the local change in one block: take the other side's lines. */
  function revertBlock(i: number) {
    if (!ds || !editable) return;
    const b = blocks[i];
    if (!b) return;
    hist = push(hist, ds, false);
    typing = false;
    ds = applyRegionLines(ds, i, b.left);
    dirty = true;
  }

  async function save() {
    if (!ds || !editable || saving) return;
    saving = true;
    try {
      const text = docText(ds.doc);
      const at = absoluteLine();
      await writeLocalFile(rightPath, text);
      dirty = false;
      // The file on disk is the new right side, so the diff is recomputed against
      // it: blocks that were edited into agreement stop being changes. The save is
      // a natural checkpoint, so history starts again from here.
      rebuild(text, at);
      hist = emptyHistory();
      if (changes.length) goTo(Math.min(current, changes.length - 1));
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function close() {
    await getCurrentWindow().close();
  }

  /** Basename of a path, for extension sniffing. */
  function base(p: string): string {
    return p.split(/[\\/]/).pop() ?? p;
  }
  async function recolor(left: string[], right: string[]) {
    // The window TITLE carries revisions ("foo.cpp#12 vs #14") and has no trailing
    // extension, so the language has to come from the paths.
    const lang = langForFile(base(rightPath)) ?? langForFile(base(leftPath));
    if (!lang) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const map = new Map(tokens);
    for (const lines of [left, right]) {
      if (!lines.some((l) => !map.has(l))) continue;
      try {
        const runs = await tokenizeLines(lines.join("\n"), lang, dark);
        if (!runs) continue;
        lines.forEach((l, i) => {
          if (runs[i] && !map.has(l)) map.set(l, runs[i]);
        });
      } catch {
        /* colour is optional */
      }
    }
    tokens = map;
  }

  onMount(async () => {
    // Before the first diff, so the very first blocks already honour the option.
    await loadOptions();
    try {
      const [l, r] = await Promise.all([
        invoke<string>("read_text_file", { path: leftPath }),
        invoke<string>("read_text_file", { path: rightPath }),
      ]);
      leftText = l;
      rebuild(r, { region: 0, line: 0, col: 0 });
      loading = false;
      void recolor(
        blocks.flatMap((b) => b.left),
        blocks.flatMap((b) => b.right),
      );
      if (changes.length) setTimeout(() => goTo(0), 0);
    } catch (e) {
      error = String(e);
      loading = false;
    }
  });
</script>

<!-- Read-only side: mark, line number, coloured code — as in the resolve window. -->
{#snippet pane(lines: string[], from: number, kind: string, fill = 0)}
  {#each lines as line, k}
    <div class="line k-{kind}"><span class="mk">{kind === "del" ? "-" : ""}</span><span class="ln"
        >{from + k}</span
      ><span class="src"
        >{#if invisibles}{#each visualize(tokens.get(line), line) as seg}<span
              style:color={seg.color} class:ghost={seg.ghost}>{seg.text}</span
            >{/each}{:else if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
  <!-- Rows this side does not HAVE: the block is taller because the other side
       has more lines. Hatched, because a blank row is indistinguishable from a
       real empty line — only the line numbers gave it away. Drawn as ONE element
       spanning the whole run, so the diagonals run continuously instead of
       restarting (and visibly stepping) at every row. -->
  {#if fill > 0}
    <div class="void" style="height:{fill * LH}px" aria-hidden="true"></div>
  {/if}
{/snippet}

{#snippet noToolbar(_region: number)}{/snippet}

<svelte:window onkeydown={onWindowKey} onresize={measurePan} />

<div class="wrap">
  <div class="bar">
    <span class="name mono">{title}</span>
    <span class="dim">
      {changes.length} change{changes.length === 1 ? "" : "s"}{editable
        ? dirty
          ? " · unsaved edits"
          : ""
        : " · read-only"}
    </span>
    <span class="grow"></span>
    <label class="opt" title="Ignore whitespace: lines that differ only in spaces, tabs or indentation count as unchanged. The text shown is always the real text.">
      <input type="checkbox" checked={ignoreWs} onchange={(e) => setIgnoreWs(e.currentTarget.checked)} />
      ignore spaces
    </label>
    <label class="opt" title="Show spaces as · and tabs as →, at their real width.">
      <input type="checkbox" checked={invisibles} onchange={(e) => setInvisibles(e.currentTarget.checked)} />
      show spaces
    </label>
    {#if invisibles && (endings.left || endings.right)}
      <!-- The one invisible difference the diff cannot show as a change: CR is
           stripped before lines are compared, so a file that differs only in line
           endings looks identical here. -->
      <span class="dim" title="Line endings of each side. The diff ignores them, so a difference here never shows as a changed line.">
        {endings.left && endings.right && endings.left !== endings.right
          ? `${endings.left} → ${endings.right}`
          : (endings.right || endings.left)}
      </span>
    {/if}
    <span class="legend dim">
      <span class="chip add">+ local</span><span class="chip del">− other side</span>
    </span>
    {#if changes.length > 1}
      <button onclick={() => goTo(current - 1)} title="Previous change (alt+up)">▲</button>
      <span class="dim">{current + 1}/{changes.length}</span>
      <button onclick={() => goTo(current + 1)} title="Next change (alt+down)">▼</button>
    {/if}
    <button onclick={close}>{dirty ? "Close without saving" : "Close"}</button>
    {#if editable}
      <button class="primary" disabled={!dirty || saving} onclick={save} title={rightPath}>
        {saving ? "Saving…" : "Save"}
      </button>
    {/if}
  </div>

  {#if note}
    <div class="warn">{note}</div>
  {/if}

  {#if error}
    <div class="err mono">{error}</div>
  {:else if loading || !ds}
    <div class="dim pad">Loading…</div>
  {:else}
    <div class="viewport" onwheel={onPanWheel}>
      <div class="scroll" class:hasbar={canPan} bind:this={scrollEl}>
      <div
        class="grid mono"
        class:single
        bind:this={gridEl}
        style="grid-template-columns: {gridCols}; --content-w: calc(4.2em + {maxCols}ch + 20px)"
      >
        <div class="head">{leftLabel}</div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="head gut splitgrip" title="Drag to resize the panes" onpointerdown={splitDown}>⋮</div>
        <div class="head mid">
          {rightLabel}{#if editable}<span class="dim"> — editable</span>{/if}
        </div>

        <div
          class="col"
          style="height:{total}px"
          bind:this={paneEls[0]}
          onscroll={() => onPaneScroll(0)}
        >
          {#each blocks as b, i (i)}
            <div
              class="rgn"
              class:change={b.kind !== "same"}
              data-change={b.kind === "same" ? undefined : i}
              style="top:{tops[i]}px; height:{rows[i] * LH}px"
            >
              {@render pane(b.left, starts[i].l, leftKind(i), rows[i] - b.left.length)}
            </div>
          {/each}
        </div>

        <!-- One button per remaining change: revert that block to the other side.
             The gutter background doubles as the split-drag handle. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="col gut splitgrip" style="height:{total}px" onpointerdown={splitDown}>
          {#each blocks as _b, i (i)}
            {#if !settled[i]}
              <div class="revwrap" style="top:{tops[i]}px">
                {#if editable}
                  <!-- Points the way the text moves: from the reference side into
                       the local file. -->
                  <button
                    class="rev"
                    title="Revert this block — take {leftLabel} into the local file"
                    onclick={() => revertBlock(i)}>▶</button
                  >
                {:else}
                  <span class="dim ro" title="Read-only diff">•</span>
                {/if}
              </div>
            {/if}
          {/each}
        </div>

        <div class="resultcol" bind:this={paneEls[1]} onscroll={() => onPaneScroll(1)}>
          <MergeResult
            docState={ds}
            {rows}
            starts={starts.map((s) => s.r)}
            {kinds}
            {tokens}
            showInvisibles={invisibles}
            lineHeight={LH}
            toolbarHeight={TOOLBAR}
            toolbar={noToolbar}
            onAction={apply}
          />
        </div>
        </div>
      </div>
      {#if canPan}
        <!-- On the grid's own column template, so each bar is exactly as wide as
             the pane above it. -->
      <!-- The row stops at the vertical scrollbar, so its columns match the panes'
           (which are fractions of the scroller's CLIENT width). The ruler is an
           overlay on top of the content rather than a column, so it is not
           subtracted here — the last bar just keeps clear of it. -->
        <div class="hbars" style="grid-template-columns: {gridCols}; right: {barWidth}px">
          <div
            class="hbar"
            bind:this={barEls[0]}
            bind:clientWidth={barWs[0]}
            onscroll={() => onBarScroll(0)}
          >
            <div class="hspace" style="width:{spacerW(0)}px"></div>
          </div>
          <div class="hgut"></div>
          <div
            class="hbar"
            style="margin-right: {marks.length ? 11 : 0}px"
            bind:this={barEls[1]}
            bind:clientWidth={barWs[1]}
            onscroll={() => onBarScroll(1)}
          >
            <div class="hspace" style="width:{spacerW(1)}px"></div>
          </div>
        </div>
      {/if}
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
  .opt {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: var(--dim, #8a8a8a);
    cursor: pointer;
    white-space: nowrap;
  }
  /* Whitespace marks: present, but never competing with the code. */
  .src :global(.ghost) {
    color: var(--dim, #8a8a8a);
    opacity: 0.55;
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
  .warn {
    flex: none;
    padding: 6px 10px;
    font-size: 12px;
    line-height: 1.4;
    color: #f0c674;
    background: color-mix(in srgb, #f0c674 12%, transparent);
    border-bottom: 1px solid color-mix(in srgb, #f0c674 40%, transparent);
  }
  .err {
    padding: 10px;
    color: var(--warn, #d9a33a);
    white-space: pre-wrap;
  }
  .hbars {
    position: absolute;
    left: 0;
    /* `right` comes from the markup: it depends on the scrollbar width and
       whether the ruler is there. */
    bottom: 0;
    display: grid;
    height: 12px;
    background: var(--bg-panel);
    border-top: 1px solid var(--border);
    z-index: 3;
  }
  .hbar {
    overflow-x: auto;
    overflow-y: hidden;
    min-width: 0;
  }
  /* The gutter column, so the two bars read as belonging to their own panes. */
  .hgut {
    border-left: 1px solid var(--border);
    border-right: 1px solid var(--border);
  }
  .hspace {
    height: 1px;
  }
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
  }
  /* Room for the pinned pan bar, so it does not sit on top of the last line. */
  .scroll.hasbar {
    padding-bottom: 12px;
  }
  .scroll {
    flex: 1;
    overflow: auto;
    min-height: 0;
  }
  .grid {
    display: grid;
    /* columns come from the inline style (movable split) */
    align-items: start;
  }
  .splitgrip {
    cursor: col-resize;
    user-select: none;
  }
  /* Single-pane (added file): the zero-width columns must not leave stray
     borders, and there is nothing to drag. */
  .grid.single .col,
  .grid.single .head {
    border-right: none;
  }
  .grid.single .head:not(.mid) {
    padding: 0;
  }
  .grid.single .splitgrip {
    cursor: default;
    pointer-events: none;
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
  .col {
    position: relative;
    border-right: 1px solid var(--border, #333);
    /* Sideways is the pane's own scroll; vertical belongs to the whole grid. */
    overflow-x: auto;
    overflow-y: hidden;
    min-width: 0;
    /* The pane is as tall as the file, so its own scrollbar would sit at the
       bottom of the document, out of reach. Hidden: shift+wheel and trackpad
       gestures do the panning. */
    scrollbar-width: none;
  }
  .col::-webkit-scrollbar {
    height: 0;
  }
  .col.gut {
    background: var(--bg-alt, #1f1f1f);
    text-align: center;
  }
  .head.gut {
    padding: 5px 0;
  }
  .revwrap {
    position: absolute;
    left: 0;
    right: 0;
    height: 17.4px;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .rev {
    padding: 0 3px;
    height: 14px;
    line-height: 12px;
    font-size: 10px;
    opacity: 0.65;
  }
  .rev:hover {
    opacity: 1;
    border-color: var(--accent, #d98d3a);
    color: var(--accent, #d98d3a);
  }
  .ro {
    font-size: 10px;
    opacity: 0.5;
  }
  .resultcol {
    background: rgba(255, 255, 255, 0.02);
    /* Same as .col: sideways here, vertical on the grid. */
    overflow-x: auto;
    overflow-y: hidden;
    min-width: 0;
    scrollbar-width: none;
  }
  .resultcol::-webkit-scrollbar {
    height: 0;
  }
  .rgn {
    position: absolute;
    left: 0;
    right: auto;
    /* Wide enough for the longest line, from --content-w (set on the grid): the
       panes are monospace, so that width is arithmetic rather than a measurement,
       and it does not depend on intrinsic sizing reaching through an absolutely
       positioned box. Never narrower than the pane, so backgrounds still span. */
    width: max(100%, var(--content-w, 100%));
    overflow: hidden;
  }
  .line {
    display: flex;
    width: max(100%, var(--content-w, 100%));
    align-items: flex-start;
    line-height: 1.45;
    height: 17.4px;
    border-left: 3px solid transparent;
    box-sizing: border-box;
  }
  .src {
    white-space: pre;
    tab-size: 4;
    min-width: 0;
    padding-right: 6px;
  }
  .mk {
    flex: none;
    width: 1em;
    text-align: center;
    user-select: none;
  }
  /* A row that does not exist on this side. Faint diagonal hatching reads as
     "nothing here" without competing with the add/drop colours. */
  .void {
    /* Ramped stops, not hard ones: a hard edge at -45deg lands between device
       pixels at fractional display scaling and the stripes come out jittery.
       Ramping lets them anti-alias, and the alpha is the only opacity knob. */
    background-image: repeating-linear-gradient(
      -45deg,
      transparent 0px,
      rgba(255, 255, 255, 0.16) 1px,
      rgba(255, 255, 255, 0.16) 2px,
      transparent 3px,
      transparent 9px
    );
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
  /* Same scale as the resolve window: green is kept, orange is dropped. */
  .k-del {
    background: rgba(217, 135, 58, 0.14);
    border-left-color: #d9873a;
  }
  .k-del .mk {
    color: #d9873a;
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
