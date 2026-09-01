<script lang="ts">
  //! Blame: the file, with the changelist that introduced each line.
  //!
  //! A sibling of the diff window, not a mode of it. It shares the chrome, the
  //! syntax highlighting, the overview ruler and the window geometry; it shares
  //! none of the diff's block/caret/edit model, because blame has one text and
  //! nothing to edit.
  //!
  //! Lines are grouped into RUNS of consecutive lines from the same changelist:
  //! repeating the same change, author and date down forty rows is noise, and
  //! what the eye needs is where one change ends and the next begins.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import OverviewRuler, { type Mark } from "$lib/components/OverviewRuler.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import { cacheGet, cacheSet } from "$lib/store.svelte";
  import { openDiff } from "$lib/opendiff";
  import { editor } from "$lib/editor.svelte";
  import { shortcuts } from "$lib/shortcuts.svelte";
  import {
    p4,
    emptyConn,
    setClipboard,
    openBlameWindow,
    type P4Conn,
    type Blame,
    type BlameLine,
  } from "$lib/p4";

  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  let conn = $state<P4Conn>(emptyConn());
  let file = $state("");
  let revSpec = $state("");
  let blame = $state<Blame | null>(null);
  let error = $state("");
  let notice = $state("");
  let loading = $state(true);
  let tokens = $state<TokenRun[][] | null>(null);
  /** Follow the branch this file was created from. ON by default, for the reason
   *  the History tab has it: a migrated depot credits every line to whoever ran
   *  the migration otherwise. */
  let follow = $state(true);

  function setNotice(m: string, ms = 4000) {
    notice = m;
    window.setTimeout(() => (notice = ""), ms);
  }

  onMount(async () => {
    try {
      const job = await invoke<{ conn: P4Conn; depotFile: string; revSpec: string }>(
        "file_history_job",
        { id },
      );
      conn = job.conn;
      file = job.depotFile;
      revSpec = job.revSpec ?? "";
      void editor.init(); // openDiff reads the diff-tool choice from this store
      void shortcuts.init(); // honour rebindings here too
      follow = ((await cacheGet("nav", "follow-branches")) ?? "1") === "1";
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  });

  /** (Re)read the blame. Syntax colouring is redone with it: the text can change
   *  when the credit does not (a different revision of the file). */
  async function load() {
    const b = await p4.annotate(conn, file, revSpec, follow);
    blame = b;
    const lang = langForFile(file);
    if (lang) {
      tokens = await tokenizeLines(
        b.lines.map((l) => l.text).join("\n"),
        lang,
        matchMedia("(prefers-color-scheme: dark)").matches,
      );
    }
  }

  /** The preference is shared with the History tab — it is one idea, not two. */
  async function setFollow(v: boolean) {
    follow = v;
    cacheSet("nav", "follow-branches", v ? "1" : "0");
    loading = true;
    try {
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  /** Consecutive lines from one changelist. The gutter is drawn once per run. */
  interface Run {
    change: string;
    rev: string;
    /** Set when this change's revision belongs to another file (a line written
     *  before this path was branched): the revision number means nothing on the
     *  file on screen, so every action on the run uses this instead. */
    file: string;
    user: string;
    date: string;
    from: number; // 0-based index of the first line
    count: number;
  }
  const runs = $derived.by<Run[]>(() => {
    const out: Run[] = [];
    for (const [i, l] of (blame?.lines ?? []).entries()) {
      const last = out[out.length - 1];
      if (last && last.change === l.change) last.count++;
      else
        out.push({
          change: l.change,
          rev: l.rev,
          file: l.file,
          user: l.user,
          date: l.date,
          from: i,
          count: 1,
        });
    }
    return out;
  });
  /** Line index to its run, so hovering one line can light the whole change. */
  const runOf = $derived.by(() => {
    const m = new Map<number, number>();
    runs.forEach((r, ri) => {
      for (let i = r.from; i < r.from + r.count; i++) m.set(i, ri);
    });
    return m;
  });
  let hot = $state(-1); // hovered run
  /** First line index of every run: where a block starts gets a rule across the
   *  whole row, so a boundary is visible even between two same-age blocks. */
  const startsRun = $derived(new Set(runs.map((r) => r.from)));

  /** A colour that belongs to the CHANGE, not to its position in the file: the
   *  same changelist is the same hue wherever it appears, so two blocks pages
   *  apart are visibly one edit. Age tinting used to live here and had to go —
   *  two colour encodings on one surface fight, and "which blocks are the same
   *  change" is the question worth answering at a glance.
   *
   *  Hues are spread by RANK among the changes present in this file, not hashed
   *  from the changelist number. Hashing was tried first (golden-angle stepping,
   *  `change * 137.508 % 360`) and measured against a real file: it spreads
   *  CONSECUTIVE numbers well, but these differ by thousands and it aliased —
   *  eight changes landed on hues 112/118/126/126/128/176/316/337, five of them
   *  within 16° and two identical. Even spacing over however many changes are on
   *  screen guarantees the separation instead of hoping for it.
   *
   *  The cost, taken deliberately: a change's colour belongs to this view, not to
   *  the changelist globally, so the same change may be a different hue in
   *  another file's blame. Telling THESE blocks apart is what the colour is for. */
  const hues = $derived.by(() => {
    const uniq = [...new Set(runs.map((r) => Number(r.change)))].sort((a, b) => a - b);
    const step = 360 / Math.max(1, uniq.length);
    return new Map(uniq.map((c, i) => [c, i * step]));
  });
  function hueOf(change: string): number {
    return hues.get(Number(change)) ?? 0;
  }
  /** The change's hue at a given strength. Alpha over the page background, so it
   *  works in both themes without a second palette. */
  function shade(change: string, alpha: number): string {
    return `hsl(${hueOf(change).toFixed(1)} 65% 55% / ${alpha})`;
  }

  const total = $derived(Math.max(1, blame?.lines.length ?? 1));
  // One band per change, as tall as the block and in the same colour the block
  // has in the code: the strip is a map of how the file is layered, including
  // the parts scrolled out of sight. It deliberately does NOT use the diff
  // palette the ruler defaults to — nothing here differs from anything, and
  // green/orange ticks read as though something did.
  const marks = $derived.by<Mark[]>(() => {
    // A band under about two pixels cannot be read, and a file like
    // HLSLMaterialTranslator.cpp has 2880 blocks — at full density the strip is
    // a rainbow with a DOM node per block. Consecutive runs are merged until a
    // band is worth drawing, and it takes the colour of the longest run in it,
    // so the strip shows who dominates each stretch of the file.
    const MIN = 0.003; // ~2px on a 700px strip
    const out: Mark[] = [];
    let i = 0;
    while (i < runs.length) {
      let end = i;
      let lines = runs[i].count;
      let biggest = i;
      while (lines / total < MIN && end + 1 < runs.length) {
        end += 1;
        lines += runs[end].count;
        if (runs[end].count > runs[biggest].count) biggest = end;
      }
      const r = runs[biggest];
      const merged = end - i + 1;
      out.push({
        pct: runs[i].from / total,
        span: lines / total,
        color: shade(r.change, 0.7),
        kind: "mod" as const, // unused: `color` wins
        title:
          "@" + r.change + " · " + r.user + " · " + r.date +
          (merged > 1 ? ` · and ${merged - 1} more change${merged > 2 ? "s" : ""} here` : ""),
        index: biggest,
      });
      i = end + 1;
    }
    return out;
  });

  /** Row height: 12px * 1.45, as the diff window uses. Rows are placed by
   *  arithmetic, so this and the CSS must agree exactly. */
  const LH = 17.4;
  let scrollEl = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let viewH = $state(600);

  /** The window of lines worth rendering, with a little either side so a fast
   *  scroll does not show a blank band before the next frame. */
  const OVERSCAN = 24;
  const first = $derived(Math.max(0, Math.floor(scrollTop / LH) - OVERSCAN));
  const last = $derived(
    Math.min(total - 1, Math.ceil((scrollTop + viewH) / LH) + OVERSCAN),
  );
  const visLines = $derived.by(() => {
    const out: { i: number; l: BlameLine }[] = [];
    const lines = blame?.lines ?? [];
    for (let i = first; i <= last && i < lines.length; i++) out.push({ i, l: lines[i] });
    return out;
  });
  /** The runs overlapping that window — the gutter cells to draw. The index
   *  comes along: `hot` is a run index, and looking it up per row would search
   *  all 2880 of them on every frame. */
  const visRuns = $derived(
    runs
      .map((r, ri) => ({ r, ri }))
      .filter(({ r }) => r.from + r.count > first && r.from <= last),
  );
  /** The code column is as wide as the longest line, so the horizontal scrollbar
   *  does not change size as rows come and go. Computed once per blame. */
  const maxCols = $derived.by(() => {
    let n = 40;
    for (const l of blame?.lines ?? []) if (l.text.length > n) n = l.text.length;
    return n;
  });
  /** Same for the provenance column, whose width used to come from its content. */
  const gutCols = $derived.by(() => {
    let n = 18;
    for (const r of runs) {
      const w = ("@" + r.change).length + r.user.length + 12;
      if (w > n) n = w;
    }
    return Math.min(n, 44);
  });

  function onScroll() {
    if (!scrollEl) return;
    scrollTop = scrollEl.scrollTop;
    viewH = scrollEl.clientHeight;
  }
  /** Which run is under the pointer, from the pointer's y — one listener for the
   *  whole file instead of two per row. */
  function onHover(e: MouseEvent) {
    if (!scrollEl) return;
    const y = e.clientY - scrollEl.getBoundingClientRect().top + scrollEl.scrollTop;
    hot = runOf.get(Math.floor(y / LH)) ?? -1;
  }
  /** The native scrollbar's width, so the ruler sits beside the slider rather
   *  than over it. Sparse ticks let the slider show through between them; bands
   *  do not, which is how this long-standing omission became visible. */
  const barWidth = $derived(scrollEl ? scrollEl.offsetWidth - scrollEl.clientWidth : 0);
  /** Rows here are uniform, so a fraction of the scroll height IS the line. */
  function scrollToFraction(f: number) {
    if (!scrollEl) return;
    scrollEl.scrollTop = Math.max(0, f * scrollEl.scrollHeight - scrollEl.clientHeight / 3);
  }
  function jumpToRun(i: number) {
    const r = runs[i];
    if (r) scrollToFraction(r.from / total);
  }

  function copyChange(change: string) {
    void setClipboard(change).then(() => setNotice("Copied @" + change + ".", 2000));
  }
  function diffChange(r: Run) {
    if (!r.rev) {
      setNotice(
        "@" + r.change + " is older than the revisions filelog returned — no diff for it.",
        6000,
      );
      return;
    }
    // A pre-branch revision belongs to the path it was written in.
    void openDiff(conn, { kind: "rev", file: r.file || file, rev: Number(r.rev) }, setNotice);
  }
  /** Blame the file as it was BEFORE this change — the revision below the one
   *  the change produced. This is how you walk past a reformat or a rename to
   *  the change that actually wrote the line. */
  function blamePrevious(r: Run) {
    const rev = Number(r.rev);
    if (!rev) {
      setNotice("@" + r.change + " has no revision here, so there is nothing to step back to.", 6000);
      return;
    }
    if (rev <= 1) {
      setNotice("@" + r.change + " added this file — there is nothing before it.", 6000);
      return;
    }
    void openBlameWindow(conn, r.file || file, "#" + (rev - 1)).catch((e) => (error = String(e)));
  }

  // --- gutter context menu ---------------------------------------------------
  let ctx = $state<{ x: number; y: number; run: Run } | null>(null);
  function menuItems(r: Run) {
    return [
      { label: "Diff against previous revision", disabled: !r.rev, action: () => diffChange(r) },
      {
        label: !r.rev
          ? `Blame before @${r.change}…`
          : Number(r.rev) <= 1
            ? `Blame before @${r.change} — it added this file`
            : `Blame #${Number(r.rev) - 1}, just before @${r.change}…`,
        disabled: !r.rev || Number(r.rev) <= 1,
        action: () => blamePrevious(r),
      },
      { label: "", sep: true },
      { label: `Copy @${r.change}`, action: () => copyChange(r.change) },
    ];
  }

  function close() {
    void getCurrentWindow().close();
  }
  /** Escape always closes; the bindable shortcut is honoured too, so a rebound
   *  "Close the window" works in the child windows as well as the main one. */
  function onWinKey(e: KeyboardEvent) {
    if (e.key === "Escape") return close();
    if (shortcuts.match(e, ["app"]) === "closeWindow") {
      e.preventDefault();
      close();
    }
  }

  /** Shift+wheel (and a horizontal wheel or swipe) pans a line wider than the
   *  window. Handled here rather than left to the webview, which does not
   *  reliably turn a shifted wheel into horizontal scroll. The scrollbar itself
   *  is the scroller's own — unlike the diff panes, this one is window-height,
   *  so it is where you can reach it. */
  function onPanWheel(e: WheelEvent) {
    if (!scrollEl) return;
    const dx = e.deltaX !== 0 ? e.deltaX : e.shiftKey ? e.deltaY : 0;
    if (!dx || scrollEl.scrollWidth <= scrollEl.clientWidth) return;
    e.preventDefault();
    scrollEl.scrollLeft += dx;
  }
</script>

<svelte:window onkeydown={onWinKey} />

<div class="wrap">
  <div class="bar">
    <span class="name mono" title={file}>{file}{blame?.rev ? "#" + blame.rev : revSpec}</span>
    <span class="grow"></span>
    {#if blame}
      <span class="dim">{blame.lines.length} lines · {runs.length} blocks</span>
    {/if}
    <!-- Whether the credit walks back past the branch this file came from. -->
    <label
      class="opt"
      title="Credit lines written before this file was branched into this path to whoever wrote them (p4 annotate -i), instead of to whoever branched it."
    >
      <input type="checkbox" checked={follow} onchange={(e) => void setFollow(e.currentTarget.checked)} />
      branch history
    </label>
    <button onclick={close}>Close</button>
  </div>

  {#if error}
    <div class="err mono">{error}</div>
  {/if}
  {#if notice}
    <div class="note">{notice}</div>
  {/if}

  {#if ctx}
    <ContextMenu x={ctx.x} y={ctx.y} items={menuItems(ctx.run)} onClose={() => (ctx = null)} />
  {/if}

  {#if loading}
    <div class="pad dim">Annotating…</div>
  {:else if blame}
    <div class="viewport">
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="scroll"
        bind:this={scrollEl}
        bind:clientHeight={viewH}
        onscroll={onScroll}
        onwheel={onPanWheel}
        onmousemove={onHover}
        onmouseleave={() => (hot = -1)}
      >
        <!-- As tall as the file and as wide as its longest line, so the
             scrollbars mean what they say while only the visible rows exist. -->
        <div
          class="canvas mono"
          style="height:{total * LH}px; width: max(100%, calc({gutCols}ch + {String(total).length + 1}ch + {maxCols}ch + 24px)); --gut-w:{gutCols}ch; --lno-w:{String(total).length + 1}ch"
        >
          {#each visRuns as { r, ri } (r.from)}
            <!-- One gutter cell per run, spanning its lines: the provenance is
                 written where the change begins, not on every row. -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="gut"
              class:hot={hot === ri}
              style="top:{r.from * LH}px; height:{r.count * LH}px; background: {shade(r.change, 0.14)}; border-left: 3px solid {shade(r.change, 0.95)}"
              title={"@" + r.change + " · " + r.user + " · " + r.date + (r.rev ? " · produced #" + r.rev : "") + "\nClick to diff this change · right-click for more"}
              onclick={() => diffChange(r)}
              oncontextmenu={(e) => {
                e.preventDefault();
                ctx = { x: e.clientX, y: e.clientY, run: r };
              }}
            >
              <span class="chg" style="color: {shade(r.change, 1)}">@{r.change}</span>
              <span class="who">{r.user}</span>
              <span class="when dim">{r.date.slice(0, 10)}</span>
            </div>
          {/each}

          {#each visLines as { i, l } (i)}
            <div
              class="row"
              class:hot={hot === runOf.get(i)}
              class:blockstart={startsRun.has(i)}
              style="top:{i * LH}px; background: {shade(l.change, 0.07)}"
            >
              <span class="lno dim">{i + 1}</span><span class="code"
                >{#if tokens && tokens[i]}{#each tokens[i] as run}<span style:color={run.color}
                      >{run.content}</span
                    >{/each}{:else}{l.text}{/if}</span
              >
            </div>
          {/each}
        </div>
      </div>
      {#if marks.length}
        <OverviewRuler {marks} offsetRight={barWidth} onPick={jumpToRun} onSeek={scrollToFraction} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .grow {
    flex: 1;
  }
  .opt {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: var(--text-dim, #999);
    cursor: pointer;
    white-space: nowrap;
  }
  .dim {
    opacity: 0.65;
  }
  .mono {
    font-family: var(--mono);
  }
  .err,
  .note,
  .pad {
    flex: none;
    padding: 6px 10px;
    font-size: 12px;
  }
  .err {
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 12%, transparent);
  }
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .scroll {
    flex: 1;
    overflow: auto;    /* Rows come and go as this scrolls, and Chromium's scroll anchoring reacts to
       that by "correcting" scrollTop toward whatever element it had anchored to
       — which, when the anchor is a row we just replaced with a spacer, throws
       the view a long way at random. A virtualized list has to opt out. */
    overflow-anchor: none;
  }
  /* Only the visible rows exist, so they are placed by arithmetic rather than by
     the grid: a file-tall canvas keeps the scrollbar honest, and its width comes
     from the longest line so the horizontal scrollbar does not resize as rows
     come and go. */
  .canvas {
    position: relative;
    font-size: 12px;
    line-height: 17.4px;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 17.4px;
    display: flex;
    align-items: flex-start;
    white-space: pre;
  }
  .gut {
    position: absolute;
    left: 0;
    width: var(--gut-w);
    box-sizing: border-box;
    z-index: 1;
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 0 8px;
    border-right: 1px solid var(--border);
    border-top: 1px solid var(--border);
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
  }
  .gut.hot {
    background: var(--bg-hover) !important;
  }
  .chg {
    color: var(--accent);
  }
  .who {
    max-width: 14ch;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .when {
    font-variant-numeric: tabular-nums;
  }
  .lno {
    flex: none;
    width: var(--lno-w);
    padding: 0 8px 0 6px;
    text-align: right;
    font-variant-numeric: tabular-nums;
    user-select: none;
  }
  .code {
    padding-right: 12px;
    white-space: pre;
  }
  /* A rule at each block's first line — two adjacent blocks can still land on
     close hues, and the rule settles it. */
  .row.blockstart {
    border-top: 1px solid var(--border);
  }
  /* Hover wins over the per-change wash, which is set inline. */
  .row.hot {
    background: var(--bg-hover) !important;
  }
  /* The code starts after the provenance column, which floats above it. */
  .lno {
    margin-left: var(--gut-w);
  }
</style>
