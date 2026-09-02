<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { langForFile, openSyntax, type SyntaxSession, type TokenRun } from "$lib/syntax";
  import { rowWindow } from "$lib/rowwindow";
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
  import { colorParts } from "$lib/invisibles";
  import { p4, setClipboard } from "$lib/p4";
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
  /** Region → where its text came from; also marks a conflict as settled. */
  let origin = $state<Record<number, string>>({});
  let current = $state(0); // which conflict the prev/next buttons are on
  /** The workspace file this merge is built on has been written by something
   *  else. It is one of the three inputs, so the whole comparison is stale —
   *  and this window ends by writing its result over that file. */
  let changedOnDisk = $state(false);
  let reloading = $state(false);
  let stopWatching: (() => void) | null = null;
  /** A pane's colouring: runs per line, sparse — a line not yet asked for has
   *  no entry and renders uncoloured. */
  type Toks = (TokenRun[] | undefined)[];
  /** Colouring per pane, by ABSOLUTE line index into that pane's own text.
   *  Indexed, not keyed by line text: a grammar is a state machine, so the same
   *  text tokenizes differently in two places — and a text key also lets one
   *  pane's answer land on another pane's identical line. */
  let tokTheirs = $state<Toks>([]);
  let tokOurs = $state<Toks>([]);
  let tokResult = $state<Toks>([]);
  let typing = false; // coalesce consecutive typing into one undo step

  const regions = $derived(data?.regions ?? []);
  const conflicts = $derived(
    regions.map((r, i) => (r.kind === "conflict" ? i : -1)).filter((i) => i >= 0),
  );
  const unsettled = $derived(conflicts.filter((i) => origin[i] === undefined));
  /** Every region the merge touched: what the depot brought, what the workspace
   *  brought, what both did. */
  const changed = $derived(
    regions.map((r, i) => (r.kind === "same" ? -1 : i)).filter((i) => i >= 0),
  );
  /** What prev/next and alt+up/down step through. Conflicts when there are any —
   *  they need a decision — otherwise every change, because a merge with no
   *  conflicts still MOVED code into this file and the user has to be able to
   *  find it. Landing on the top of an untouched file and calling it done is how
   *  an incoming change gets mistaken for junk and deleted later. */
  const stops = $derived(conflicts.length ? conflicts : changed);
  const fromDepot = $derived(regions.filter((r) => r.kind === "theirs").length);
  const fromUs = $derived(regions.filter((r) => r.kind === "ours").length);
  const fromBoth = $derived(regions.filter((r) => r.kind === "both").length);

  /** What a side pane shows: its own text, or the base where it didn't change. */
  function side(r: MergeRegion, which: Side): string[] {
    if (r.kind === "same" || r.kind === "both") return r.lines;
    if (r.kind === "conflict") return r[which];
    return r.kind === which ? r.lines : r.base;
  }

  // --- add / drop, the same meaning in every pane ---------------------------
  function sideKind(r: MergeRegion, which: Side, i: number): string {
    if (r.kind === "same") return "";
    if (r.kind === "conflict") {
      // Once a side is taken the two sides are no longer rivals: one of them is
      // in the file and the other is not, which is what add/drop already say
      // everywhere else. `base` drops both; a hand-edit is neither, so it claims
      // neither marker.
      const o = origin[i];
      if (o === undefined) return "vs";
      if (o === "manual") return "";
      if (o === "both") return "add";
      return o === which ? "add" : "del";
    }
    if (r.kind === "both") return "add";
    return r.kind === which ? "add" : "del";
  }
  const kinds = $derived(
    regions.map((r, i) => {
      // An undecided conflict is red; one with a side taken is not. It reads as
      // whatever it now holds, like every other region — the toolbar keeps the
      // choice visible (and `reset` undoes it), so nothing is lost by letting a
      // settled block look settled.
      if (r.kind === "conflict" && origin[i] === undefined) return "vs";
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
  /** Apply an intent from the result pane to the model, recording undo.
   *
   *  Every branch is one call into the model: it owns the cursors, so an edit
   *  with three carets down is the same code as with one. */
  function apply(a: MergeAction) {
    if (!ds) return;
    const before = ds;
    /** Record undo, take the new state, and settle every region an edit reached
     *  — with several cursors that can be more than one. */
    const edit = (next: DocState, coalesce = false) => {
      hist = push(hist, before, coalesce);
      for (const c of before.cursors) touched(c.head.region);
      ds = next;
      scheduleRecolor(); // the edited lines have no tokens yet
    };
    switch (a.t) {
      case "insert":
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
        typing = false;
        ds = applySelectWord(before, a.caret, a.add);
        break;
      case "selectLine":
        typing = false;
        ds = applySelectLine(before, a.caret, a.add);
        break;
      case "selectAll":
        typing = false;
        ds = applySelectAll(before);
        break;
      case "copy": {
        typing = false;
        const text = copyText(before);
        if (!text) break;
        void setClipboard(text).catch((e) => (error = String(e)));
        if (a.cut) edit(applyBackspace(before));
        break;
      }
      case "undo": {
        typing = false;
        const u = undo(hist, before);
        if (u) {
          ds = u.state;
          hist = u.history;
          scheduleRecolor();
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
          scheduleRecolor();
        }
        break;
      }
    }
  }
  // --- horizontal panning ----------------------------------------------------
  // A scrollbar under each pane, and the panes move together: the panes show the
  // same columns of three versions of a file, so panning one and not
  // the other would break the comparison. One bar stretched across all the panels
  // was the first attempt and read as if they were a single surface.
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

  /** The longest line across all four panes, in visual columns. */
  const maxCols = $derived.by(() => {
    let n = 0;
    const widest = (lines: string[]) => {
      for (const l of lines) n = Math.max(n, visualColumn(l, l.length));
    };
    for (const r of regions) {
      if (r.kind === "conflict") {
        widest(r.theirs);
        widest(r.ours);
      } else {
        widest(r.lines);
      }
      if (r.kind !== "same") widest(r.base);
    }
    for (const r of ds?.doc.regions ?? []) widest(r.lines);
    return n;
  });

  // Observed, not guessed at — see the diff window for why a state-change
  // measurement is not enough (the grid does not exist yet when it fires).
  $effect(() => {
    const ps = paneEls.filter((x): x is HTMLElement => !!x);
    if (!ps.length) return;
    const ro = new ResizeObserver(() => measurePan());
    for (const p of ps) ro.observe(p);
    measurePan();
    return () => ro.disconnect();
  });
  $effect(() => {
    void regions;
    void ds;
    void maxCols;
    requestAnimationFrame(measurePan);
  });

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
    ds = applyRegionLines(ds, i, lines);
    origin = { ...origin, [i]: what };
    scheduleRecolor(); // taking a side rewrites the region — as an edit does
  }
  /** Take one side for the WHOLE file: p4's "accept theirs" / "accept yours".
   *
   *  Every region becomes that side's text, not just the conflicts — so the
   *  result is that file verbatim, and accepting the depot drops the local work
   *  the auto-merge had already folded in. That is what accepting a side means;
   *  settling only the conflicts is what the per-conflict buttons do. Regions
   *  identical on both sides are left alone: there is nothing to choose, and
   *  marking them would claim a decision that was never made.
   *
   *  One undo step, like any other edit. */
  function takeAll(which: "theirs" | "ours") {
    if (!ds) return;
    hist = push(hist, ds, false);
    typing = false;
    let next = ds;
    const org: Record<number, string> = { ...origin };
    regions.forEach((r, i) => {
      if (r.kind === "same") return;
      next = applyRegionLines(next, i, side(r, which));
      org[i] = which;
    });
    ds = next;
    origin = org;
    scheduleRecolor();
  }

  /** Back to an undecided conflict. */
  function reset(i: number) {
    if (!ds) return;
    hist = push(hist, ds, false);
    typing = false;
    ds = applyRegionLines(ds, i, []);
    const next = { ...origin };
    delete next[i];
    origin = next;
    scheduleRecolor();
  }

  // --- what is on screen -----------------------------------------------------
  // Only the visible rows are rendered, in all three panes: a merge of an 18k
  // line file otherwise builds several hundred thousand nodes, and every scroll
  // frame pays for them.
  const OVERSCAN = 24;
  let viewTop = $state(0);
  let viewH = $state(1200);
  function readView() {
    if (!scrollEl) return;
    viewTop = scrollEl.scrollTop;
    viewH = scrollEl.clientHeight;
  }
  /** The rows of a region worth rendering, and the padding that stands in for
   *  the rest of it. */
  function windowOf(top: number, lines: number) {
    return rowWindow(viewTop, viewH, top, lines, LH, OVERSCAN);
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
    // Whoever handled it first wins: the editor claims alt+shift+arrows for its
    // extra carets, and preventDefault does not stop the event bubbling to here.
    if (e.defaultPrevented) return;
    if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
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
    if (!stops.length) return;
    current = ((n % stops.length) + stops.length) % stops.length;
    document
      .querySelector(`[data-region="${stops[current]}"]`)
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

  // An edit moves the lines the colouring was asked about, so the result's
  // session no longer describes the document. Reopened after the edit rather
  // than per keystroke — and until it is, `docGen` keeps the stale answers off
  // the screen: no colour for a moment beats the wrong one.
  let recolorTimer: number | null = null;
  function scheduleRecolor() {
    docGen++; // the session in hand no longer describes the document
    if (recolorTimer !== null) clearTimeout(recolorTimer);
    recolorTimer = window.setTimeout(() => {
      recolorTimer = null;
      void openResult();
    }, 250);
  }

  // One colouring session per side. A grammar is a state machine, so a line is
  // coloured from the lines before it: each side is a session over its whole
  // text, even though only a window of it is ever asked for.
  let theirsSyntax: SyntaxSession | null = null;
  let oursSyntax: SyntaxSession | null = null;
  let resultSyntax: SyntaxSession | null = null;
  let painted = "";
  /** Bumped by every change to the result document, and recorded by the session
   *  opened over it. An answer from a session older than the document is thrown
   *  away rather than painted onto lines it does not describe. */
  let docGen = 0;
  let resultGen = -1;

  /** One side of the merge, as its own file: the lines each region contributes,
   *  in order — the same order the pane draws them in. */
  function sideText(which: "theirs" | "ours"): string {
    const out: string[] = [];
    for (const r of regions) out.push(...side(r, which));
    return out.join("\n");
  }
  function sideLines(which: "theirs" | "ours"): string[] {
    const out: string[] = [];
    for (const r of regions) out.push(...side(r, which));
    return out;
  }

  async function openSides() {
    const lang = langForFile(data?.name ?? "");
    if (!lang) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    theirsSyntax?.close();
    oursSyntax?.close();
    [theirsSyntax, oursSyntax] = await Promise.all([
      openSyntax(sideText("theirs"), lang, dark),
      openSyntax(sideText("ours"), lang, dark),
    ]);
    tokTheirs = [];
    tokOurs = [];
    painted = "";
    colourWindow();
  }
  async function openResult() {
    const lang = langForFile(data?.name ?? "");
    if (!lang || !ds) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const gen = docGen; // the document this session describes
    resultSyntax?.close();
    resultSyntax = await openSyntax(
      ds.doc.regions.flatMap((r) => r.lines).join("\n"),
      lang,
      dark,
    );
    resultGen = gen;
    tokResult = [];
    painted = "";
    colourWindow();
  }

  /** The visible line range on one side, as absolute 0-based indices into that
   *  side's text — which is what a session is addressed by. */
  function visibleRange(which: "theirs" | "ours" | "result"): [number, number] | null {
    let at = 0;
    let lo = Infinity;
    let hi = -Infinity;
    regions.forEach((r, i) => {
      const lines =
        which === "result"
          ? (ds?.doc.regions[i]?.lines.length ?? 0)
          : side(r, which).length;
      if (lines) {
        const w = windowOf(tops[i] + (r.kind === "conflict" ? TOOLBAR : 0), lines);
        if (w.last >= w.first) {
          lo = Math.min(lo, at + w.first);
          hi = Math.max(hi, at + w.last);
        }
      }
      at += lines;
    });
    return hi >= lo ? [lo, hi] : null;
  }

  /** Colour the rows about to be drawn, on all three sides. */
  function colourWindow() {
    const t0 = visibleRange("theirs");
    const o0 = visibleRange("ours");
    const r0 = visibleRange("result");
    const key = `${t0}|${o0}|${r0}`;
    if (key === painted) return;
    painted = key;
    const ask = (s: SyntaxSession | null, w: [number, number] | null) =>
      s && w ? s.window(w[0], w[1] - w[0] + 1) : Promise.resolve(null);
    const gen = docGen;
    void Promise.all([
      ask(theirsSyntax, t0),
      ask(oursSyntax, o0),
      ask(resultSyntax, resultGen === gen ? r0 : null),
    ]).then(([tr, or, rr]) => {
      const put = (into: Toks, runs: TokenRun[][] | null, w: [number, number] | null): Toks => {
        if (!runs || !w) return into;
        const next = into.slice();
        runs.forEach((run, k) => (next[w[0] + k] = run));
        return next;
      };
      tokTheirs = put(tokTheirs, tr, t0);
      tokOurs = put(tokOurs, or, o0);
      // The document may have moved on while the worker was answering.
      if (gen === docGen) tokResult = put(tokResult, rr, r0);
    });
  }
  $effect(() => {
    void viewTop;
    void viewH;
    void regions;
    colourWindow();
  });

  onMount(async () => {
    try {
      data = await invoke<MergeData>("merge_data", { id });
      // The result starts as the auto-merge; conflicts start empty and unsettled.
      ds = singleCursor(
        {
          regions: data.regions.map((r, i) => ({
            region: i,
            kind: r.kind === "conflict" ? "vs" : r.kind === "same" ? "" : "add",
            conflict: r.kind === "conflict",
            lines: r.kind === "conflict" ? [] : r.lines.slice(),
          })),
        },
        { region: 0, line: 0, col: 0 },
      );
      void openSides();
      void openResult();
      // Land on the first thing the merge touched, conflict or not: a clean
      // auto-merge still has to SHOW what it brought in.
      setTimeout(() => goTo(0), 0);
    } catch (e) {
      error = String(e);
    }
    if (!data?.target) return;
    const label = getCurrentWindow().label;
    void p4.watchFile(label, data.target).catch(() => {});
    // Unlike the diff window there is nothing to compare against: the merge is
    // built from three inputs and holds no copy of the file as it stood. Any
    // write to it is worth saying, including one that changed nothing — the
    // cost of a false alarm here is a button the user need not press.
    stopWatching = await listen("file-on-disk-changed", () => (changedOnDisk = true));
  });

  onDestroy(() => {
    stopWatching?.();
    void p4.unwatchFile(getCurrentWindow().label).catch(() => {});
  });

  /** Build the merge again from the file as it now stands.
   *
   *  Everything settled in this window goes: the file is one of the three
   *  inputs, so a change to it invalidates the comparison rather than one pane
   *  of it. That is why it is a button and not something that happens. */
  async function reloadMerge() {
    reloading = true;
    try {
      await p4.mergeReload(id);
      data = await invoke<MergeData>("merge_data", { id });
      ds = singleCursor(
        {
          regions: data.regions.map((r, i) => ({
            region: i,
            kind: r.kind === "conflict" ? "vs" : r.kind === "same" ? "" : "add",
            conflict: r.kind === "conflict",
            lines: r.kind === "conflict" ? [] : r.lines.slice(),
          })),
        },
        { region: 0, line: 0, col: 0 },
      );
      hist = emptyHistory();
      origin = {};
      current = 0;
      changedOnDisk = false;
      void openSides();
      void openResult();
      setTimeout(() => goTo(0), 0);
    } catch (e) {
      error = String(e);
    } finally {
      reloading = false;
    }
  }
</script>

<!-- Read-only pane content: mark, line number, coloured code. -->
{#snippet pane(lines: string[], toks: Toks, base: number, kind: string, top = 0)}
  {@const win = windowOf(top, lines.length)}
  {@const first = win.first}
  {@const last = win.last}
  <!-- The rows outside the window, as one box each, so the rows that ARE drawn
       keep their flow position inside the region. -->
  {#if win.padBefore > 0}
    <div class="pad" style="height:{win.padBefore * LH}px" aria-hidden="true"></div>
  {/if}
  {#each lines.slice(first, last + 1) as line, k}
    <div class="line k-{kind}"><span class="mk">{MARK[kind] ?? ""}</span><span class="ln"
        >{base + first + k + 1}</span
      ><span class="src"
        >{#if line && toks[base + first + k]}{#each colorParts(line, toks[base + first + k]) as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
  {#if win.padAfter > 0}
    <div class="pad" style="height:{win.padAfter * LH}px" aria-hidden="true"></div>
  {/if}
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

<svelte:window onkeydown={onWindowKey} onresize={measurePan} />

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
      <!-- What the merge is bringing in. Without this a conflict-free merge looks
           like an empty window, and the incoming change is invisible until it
           shows up in the local diff looking like someone else's junk. -->
      {#if fromDepot}
        <span class="incoming" title="Lines the merge takes from {data.theirsLabel}. They are part of your file after saving — deleting them later would revert that change.">
          {fromDepot} block{fromDepot === 1 ? "" : "s"} incoming from {data.theirsLabel}
        </span>
      {/if}
      <span class="dim">
        {fromUs ? `· ${fromUs} yours` : ""}{fromBoth ? ` · ${fromBoth} identical on both sides` : ""}
      </span>
      <span class="grow"></span>
      <span class="legend dim">
        <span class="chip add">+ kept</span><span class="chip del">− dropped</span><span
          class="chip vs">! conflict</span
        >
      </span>
      <!-- The whole file from one side. p4 calls these "accept theirs" and
           "accept yours"; without them, taking the depot version of a file with
           twenty conflicts is twenty clicks. -->
      <span class="whole">
        <span class="dim">whole file:</span>
        <button
          title="Take {data.theirsLabel} for the ENTIRE file — every local change is dropped, auto-merged ones included (p4 'accept theirs'). Undoable with ctrl+z."
          onclick={() => takeAll("theirs")}>◀ depot</button
        >
        <button
          title="Keep {data.yoursLabel} for the ENTIRE file — nothing from {data.theirsLabel} comes in (p4 'accept yours'). Undoable with ctrl+z."
          onclick={() => takeAll("ours")}>workspace ▶</button
        >
      </span>
      <!-- Navigation walks conflicts when there are any, otherwise the changes
           the merge brought in — never nothing, or a clean merge cannot be read. -->
      {#if stops.length}
        <button onclick={() => goTo(current - 1)} title={conflicts.length ? "Previous conflict (alt+up)" : "Previous change (alt+up)"}>▲</button>
        <span class="dim">{current + 1}/{stops.length}</span>
        <button onclick={() => goTo(current + 1)} title={conflicts.length ? "Next conflict (alt+down)" : "Next change (alt+down)"}>▼</button>
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

  <!-- The file this merge is built on was written by something else. Saving now
       would put a merge of the OLD file over the new one, so this says so
       before the Save button is the thing that finds out. -->
  {#if changedOnDisk}
    <div class="stale">
      <span>
        <b>{data?.name}</b> has changed on disk since this merge was prepared — saving would
        overwrite that change with a merge of the older file.
      </span>
      <span class="grow"></span>
      <button disabled={reloading} onclick={reloadMerge}>
        {reloading ? "Rebuilding…" : "Rebuild from the file (loses your choices)"}
      </button>
      <button disabled={reloading} onclick={() => (changedOnDisk = false)}>Ignore</button>
    </div>
  {/if}

  {#if error}
    <div class="err mono">{error}</div>
  {:else if !data || !ds}
    <div class="dim pad">Loading…</div>
  {:else}
    <div class="viewport" onwheel={onPanWheel}>
      <div
        class="scroll"
        class:hasbar={canPan}
        bind:this={scrollEl}
        bind:clientHeight={viewH}
        onscroll={readView}
      >
      <div class="grid mono" style="--content-w: calc(4.2em + {maxCols}ch + 20px)">
        <div class="head">{data.theirsLabel}</div>
        <div class="head link"></div>
        <div class="head mid">
          result — merged file, editable<span class="dim"> (base: {data.baseLabel})</span>
        </div>
        <div class="head link"></div>
        <div class="head">{data.yoursLabel}</div>

        <!-- Every pane places its regions at the same y, from the same row counts. -->
        <div
          class="col"
          style="height:{total}px"
          bind:this={paneEls[0]}
          onscroll={() => onPaneScroll(0)}
        >
          {#each regions as r, i (i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              class:settled={origin[i] !== undefined}
              style="top:{tops[i]}px; height:{rows[i] * LH + (r.kind === 'conflict' ? TOOLBAR : 0)}px"
            >
              {#if r.kind === "conflict"}<div class="strip"></div>{/if}
              {@render pane(side(r, "theirs"), tokTheirs, starts[i].t - 1, sideKind(r, "theirs", i), tops[i] + (r.kind === "conflict" ? TOOLBAR : 0))}
            </div>
          {/each}
        </div>

        <div class="col link" style="height:{total}px">
          {#each regions as r, i (i)}
            {@const flow = flows(r, i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              class:settled={origin[i] !== undefined}
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

        <div class="resultcol" bind:this={paneEls[1]} onscroll={() => onPaneScroll(1)}>
          <MergeResult
            docState={ds}
            {rows}
            starts={starts.map((s) => s.m)}
            {kinds}
            tokens={tokResult}
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
              class:settled={origin[i] !== undefined}
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

        <div
          class="col"
          style="height:{total}px"
          bind:this={paneEls[2]}
          onscroll={() => onPaneScroll(2)}
        >
          {#each regions as r, i (i)}
            <div
              class="rgn"
              class:conflict={r.kind === "conflict"}
              class:settled={origin[i] !== undefined}
              style="top:{tops[i]}px; height:{rows[i] * LH + (r.kind === 'conflict' ? TOOLBAR : 0)}px"
            >
              {#if r.kind === "conflict"}<div class="strip"></div>{/if}
              {@render pane(side(r, "ours"), tokOurs, starts[i].o - 1, sideKind(r, "ours", i), tops[i] + (r.kind === "conflict" ? TOOLBAR : 0))}
            </div>
          {/each}
        </div>
        </div>
      </div>
      {#if canPan}
        <!-- The grid's own column template, so each bar sits under its own pane. -->
      <!-- The row stops at the vertical scrollbar, so its columns match the panes'
           (which are fractions of the scroller's CLIENT width). The ruler is an
           overlay on top of the content rather than a column, so it is not
           subtracted here — the last bar just keeps clear of it. -->
        <div class="hbars" style="right: {barWidth}px">
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
            bind:this={barEls[1]}
            bind:clientWidth={barWs[1]}
            onscroll={() => onBarScroll(1)}
          >
            <div class="hspace" style="width:{spacerW(1)}px"></div>
          </div>
          <div class="hgut"></div>
          <div
            class="hbar"
            style="margin-right: {marks.length ? 11 : 0}px"
            bind:this={barEls[2]}
            bind:clientWidth={barWs[2]}
            onscroll={() => onBarScroll(2)}
          >
            <div class="hspace" style="width:{spacerW(2)}px"></div>
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
  /* The incoming count is the one thing in this window a user must not miss. */
  .incoming {
    flex: none;
    font-size: 11px;
    font-weight: 600;
    line-height: 16px;
    padding: 0 6px;
    border: 1px solid currentColor;
    border-radius: 10px;
    color: var(--have, #5faf5f);
    white-space: nowrap;
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
  .hbars {
    position: absolute;
    left: 0;
    bottom: 0;
    display: grid;
    /* Must match .grid: one bar per pane, each as wide as its pane. */
    grid-template-columns: minmax(0, 1fr) 1.4rem minmax(0, 1fr) 1.4rem minmax(0, 1fr);
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
      /* Rows come and go as this scrolls, and Chromium's scroll anchoring reacts to
       that by "correcting" scrollTop toward whatever element it had anchored to
       — which, when the anchor is a row we just replaced with a spacer, throws
       the view a long way at random. A virtualized list has to opt out. */
    overflow-anchor: none;
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
    /* Sideways is the pane's own scroll; vertical belongs to the whole grid. */
    overflow-x: auto;
    overflow-y: hidden;
    min-width: 0;
    /* A pane is as tall as the file, so its own scrollbar would sit at the bottom
       of the document, out of reach — shift+wheel and trackpad gestures pan. */
    scrollbar-width: none;
  }
  .col::-webkit-scrollbar,
  .resultcol::-webkit-scrollbar {
    height: 0;
  }
  .col.link {
    background: var(--bg-alt, #1f1f1f);
    text-align: center;
  }
  .resultcol {
    background: rgba(255, 255, 255, 0.02);
    border-right: 1px solid var(--border, #333);
    overflow-x: auto;
    overflow-y: hidden;
    min-width: 0;
    scrollbar-width: none;
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
  .rgn.conflict {
    background: rgba(224, 85, 90, 0.12);
    box-shadow:
      inset 0 1px 0 rgba(224, 85, 90, 0.55),
      inset 0 -1px 0 rgba(224, 85, 90, 0.55);
  }
  /* Decided: the rules stay, so the block is still legible as one thing, but
     they stop shouting. */
  .rgn.conflict.settled {
    background: transparent;
    box-shadow:
      inset 0 1px 0 rgba(140, 140, 140, 0.35),
      inset 0 -1px 0 rgba(140, 140, 140, 0.35);
  }
  .rgn.on {
    background: rgba(124, 196, 124, 0.12);
  }
  /* Reserves the height of the result pane's conflict toolbar. */
  .strip {
    height: 24px;
    background: rgba(224, 85, 90, 0.1);
  }
  .rgn.settled .strip {
    background: rgba(140, 140, 140, 0.08);
  }
  .whole {
    display: flex;
    align-items: center;
    gap: 4px;
    padding-left: 8px;
    border-left: 1px solid var(--border, #333);
  }
  .whole button {
    font-size: 11px;
    padding: 1px 8px;
  }
  .stale {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    font-size: 12px;
    line-height: 1.4;
    color: #f0c674;
    background: color-mix(in srgb, #f0c674 12%, transparent);
    border-bottom: 1px solid color-mix(in srgb, #f0c674 40%, transparent);
  }
  .stale button {
    font-size: 11px;
    padding: 1px 8px;
    flex: none;
  }
  .arrow {
    color: #7cc47c;
    line-height: 1.45;
  }
  .arrow.open {
    color: #e0555a;
    font-weight: 600;
  }
  /* Stands in for rows outside the window: height only. */
  .pad {
    flex: none;
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
