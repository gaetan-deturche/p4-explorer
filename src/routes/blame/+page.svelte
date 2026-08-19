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
  import { openDiff } from "$lib/opendiff";
  import { editor } from "$lib/editor.svelte";
  import {
    p4,
    emptyConn,
    setClipboard,
    openBlameWindow,
    type P4Conn,
    type Blame,
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
      const b = await p4.annotate(conn, file, revSpec);
      blame = b;
      const lang = langForFile(file);
      if (lang) {
        tokens = await tokenizeLines(
          b.lines.map((l) => l.text).join("\n"),
          lang,
          matchMedia("(prefers-color-scheme: dark)").matches,
        );
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  });

  /** Consecutive lines from one changelist. The gutter is drawn once per run. */
  interface Run {
    change: string;
    rev: string;
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
        out.push({ change: l.change, rev: l.rev, user: l.user, date: l.date, from: i, count: 1 });
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
  // One tick per change, at its place in the file: the strip reads as a map of
  // how the file is layered, including the parts scrolled out of sight.
  const marks = $derived<Mark[]>(
    runs.map((r, i) => ({
      pct: r.from / total,
      kind: "mod" as const,
      title: "@" + r.change + " · " + r.user + " · " + r.date,
      index: i,
    })),
  );

  let scrollEl = $state<HTMLDivElement>();
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
    void openDiff(conn, { kind: "rev", file, rev: Number(r.rev) }, setNotice);
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
    void openBlameWindow(conn, file, "#" + (rev - 1)).catch((e) => (error = String(e)));
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
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && close()} />

<div class="wrap">
  <div class="bar">
    <span class="name mono" title={file}>{file}{blame?.rev ? "#" + blame.rev : revSpec}</span>
    <span class="grow"></span>
    {#if blame}
      <span class="dim">{blame.lines.length} lines · {runs.length} blocks</span>
    {/if}
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
      <div class="scroll" bind:this={scrollEl}>
        <div class="grid mono">
          {#each runs as r, ri (r.from)}
            <!-- One gutter cell per run, spanning its lines: the provenance is
                 written where the change begins, not on every row. -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="gut"
              class:hot={hot === ri}
              style="grid-row: {r.from + 1} / span {r.count}; background: {shade(r.change, 0.14)};
                     border-left: 3px solid {shade(r.change, 0.95)}"
              title={"@" + r.change + " · " + r.user + " · " + r.date + (r.rev ? " · produced #" + r.rev : "") + "\nClick to diff this change · right-click for more"}
              onmouseenter={() => (hot = ri)}
              onmouseleave={() => (hot = -1)}
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

          {#each blame.lines as l, i (i)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="lno dim"
              class:hot={hot === runOf.get(i)}
              class:blockstart={startsRun.has(i)}
              style="grid-row: {i + 1}; background: {shade(l.change, 0.07)}"
              onmouseenter={() => (hot = runOf.get(i) ?? -1)}
              onmouseleave={() => (hot = -1)}
            >
              {i + 1}
            </div>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="code"
              class:hot={hot === runOf.get(i)}
              class:blockstart={startsRun.has(i)}
              style="grid-row: {i + 1}; background: {shade(l.change, 0.07)}"
              onmouseenter={() => (hot = runOf.get(i) ?? -1)}
              onmouseleave={() => (hot = -1)}
            >{#if tokens && tokens[i]}{#each tokens[i] as run}<span style:color={run.color}>{run.content}</span>{/each}{:else}{l.text}{/if}</div>
          {/each}
        </div>
      </div>
      {#if marks.length}
        <OverviewRuler {marks} onPick={jumpToRun} onSeek={scrollToFraction} />
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
    overflow: auto;
  }
  .grid {
    display: grid;
    /* provenance | line number | code */
    grid-template-columns: max-content max-content 1fr;
    align-items: stretch;
    font-size: 12px;
    line-height: 1.45;
  }
  .gut {
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
    padding: 0 8px 0 6px;
    text-align: right;
    font-variant-numeric: tabular-nums;
    user-select: none;
  }
  .code {
    padding-right: 12px;
    white-space: pre;
  }
  /* A rule at each block's first line, across all three columns — two adjacent
     blocks can still land on close hues, and the rule settles it. */
  .lno.blockstart,
  .code.blockstart {
    border-top: 1px solid var(--border);
  }
  /* Hover wins over the per-change wash, which is set inline. */
  .lno.hot,
  .code.hot {
    background: var(--bg-hover) !important;
  }
</style>
