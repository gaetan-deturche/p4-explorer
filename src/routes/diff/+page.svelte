<script lang="ts">
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { diffLines, changeBlocks, type DiffRow } from "$lib/linediff";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";

  // Opened by the Rust `open_diff_window` command with the two (already
  // materialized) sides as query params: left/right = file paths on disk,
  // ll/rl = display labels, title = window subject.
  const params = new URLSearchParams(window.location.search);
  const leftPath = params.get("left") ?? "";
  const rightPath = params.get("right") ?? "";
  const leftLabel = params.get("ll") ?? "left";
  const rightLabel = params.get("rl") ?? "right";

  let rows = $state<DiffRow[]>([]);
  let blocks = $state<number[]>([]);
  let error = $state("");
  let loading = $state(true);
  let current = $state(-1); // index into blocks (prev/next navigation)
  let body = $state<HTMLDivElement>();
  // Per-line syntax tokens for each side (Shiki), or null → plain text.
  let ltoks = $state<TokenRun[][] | null>(null);
  let rtoks = $state<TokenRun[][] | null>(null);

  onMount(async () => {
    try {
      const [l, r] = await Promise.all([
        invoke<string>("read_text_file", { path: leftPath }),
        invoke<string>("read_text_file", { path: rightPath }),
      ]);
      rows = diffLines(l, r);
      blocks = changeBlocks(rows);
      loading = false;
      // Land on the first change, like P4Merge. `await tick()` matters: the rows
      // are only rendered after Svelte flushes, and goTo scrolls to a [data-row]
      // element — without it the query finds nothing and the window stays at the
      // top of the file.
      if (blocks.length) {
        await tick();
        goTo(0);
      }
      // Syntax coloration in the background — the diff is already readable, the
      // colors land when Shiki finishes. Language from the file paths (the temp
      // names keep the real extension last; labels carry #rev suffixes).
      const base = (p: string) => p.split(/[\\/]/).pop() ?? "";
      const lang = langForFile(base(rightPath)) ?? langForFile(base(leftPath));
      if (lang) {
        const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
        const [lt, rt] = await Promise.all([
          tokenizeLines(l, lang, dark),
          tokenizeLines(r, lang, dark),
        ]);
        ltoks = lt;
        rtoks = rt;
      }
    } catch (e) {
      error = String(e);
      loading = false;
    }
  });

  /** The last row of the change block starting at `start` (blocks hold starts). */
  function blockEnd(start: number): number {
    let end = start;
    while (end + 1 < rows.length && rows[end + 1].type !== "same") end++;
    return end;
  }

  /** Scroll a change into view. Placement depends on its size: a block that fits
   *  comfortably is CENTRED, while a long one is put a few lines below the top —
   *  the changed content matters more than the context above it, and centring a
   *  long block pushes its start (and most of its body) off screen. */
  function goTo(i: number) {
    if (!blocks.length || !body) return;
    current = Math.max(0, Math.min(blocks.length - 1, i));
    const row = blocks[current];
    const el = body.querySelector<HTMLElement>(`[data-row="${row}"]`);
    if (!el) {
      // Not mounted yet (first paint) — retry next frame rather than silently
      // leaving the view where it was.
      requestAnimationFrame(() => goTo(i));
      return;
    }
    const box = body.getBoundingClientRect();
    const top = el.getBoundingClientRect();
    const endEl = body.querySelector<HTMLElement>(`[data-row="${blockEnd(row)}"]`);
    const blockH = (endEl?.getBoundingClientRect().bottom ?? top.bottom) - top.top;
    const rowH = top.height || 18;
    const lead =
      blockH < box.height * 0.6
        ? Math.max(0, (box.height - blockH) / 2) // small change: centre it
        : Math.min(4 * rowH, box.height * 0.2); // long change: a few lines of context
    body.scrollTop = Math.max(0, body.scrollTop + (top.top - box.top) - lead);
    syncCounter(); // the counter follows the resulting position, not the index
  }
  /** The first row visible at the top of the viewport. */
  function topRow(): number {
    if (!body) return 0;
    const top = body.getBoundingClientRect().top;
    let last = 0;
    for (const el of body.querySelectorAll<HTMLElement>("[data-row]")) {
      last = Number(el.dataset.row);
      if (el.getBoundingClientRect().bottom > top + 1) return last;
    }
    return last;
  }

  /** Where we are in the change list: the block containing the top of the view,
   *  else the next one coming up (`inBlock` false). This one notion drives both
   *  the counter and stepping, so they can never disagree — and it's independent
   *  of where goTo happens to place a block on screen. */
  function position(): { idx: number; inBlock: boolean; past: boolean; row: number } {
    const row = topRow();
    for (let k = 0; k < blocks.length; k++) {
      const start = blocks[k];
      if (row < start) return { idx: k, inBlock: false, past: false, row }; // gap: k is upcoming
      if (row <= blockEnd(start)) return { idx: k, inBlock: true, past: false, row };
    }
    // Past the last change (trailing context): nothing ahead.
    return { idx: Math.max(0, blocks.length - 1), inBlock: false, past: true, row };
  }
  // End-of-list feedback: the first press at the last (or first) change says so
  // instead of moving — a big block would otherwise scroll back to its own start,
  // looking like a broken button — and a second press wraps around.
  let hint = $state("");
  let hintTimer: number | null = null;
  let wrapArmed: 1 | -1 | null = null;
  function say(msg: string, arm: 1 | -1 | null) {
    hint = msg;
    wrapArmed = arm;
    if (hintTimer !== null) clearTimeout(hintTimer);
    hintTimer = window.setTimeout(() => {
      hint = "";
      wrapArmed = null; // the offer to wrap expires with the message
    }, 3000);
  }
  function clearHint() {
    if (hintTimer !== null) clearTimeout(hintTimer);
    hintTimer = null;
    hint = "";
    wrapArmed = null;
  }

  /** Step to the next/previous change relative to the visible position. */
  function step(dir: 1 | -1) {
    if (!blocks.length) return;
    const { idx, inBlock, past, row } = position();
    // Next: the change coming up (in a gap) or the one after the current change;
    // nothing at all once past the last change — never jump backwards.
    // Previous: from deep inside a long change, its own start first (that IS the
    // move back); otherwise the change before.
    let target: number;
    if (dir === 1) target = past ? -1 : inBlock ? idx + 1 : idx;
    else if (past) target = idx; // back into the last change
    else target = inBlock && row > blocks[idx] ? idx : idx - 1;
    const i = target >= 0 && target < blocks.length ? target : -1;
    if (i !== -1) {
      clearHint();
      goTo(i);
      return;
    }
    // Nowhere to go in that direction.
    if (wrapArmed === dir) {
      clearHint();
      goTo(dir === 1 ? 0 : blocks.length - 1); // wrap
      return;
    }
    const last = dir === 1;
    say(
      blocks.length === 1
        ? "Only change in this file — press again to jump to it"
        : `${last ? "Last" : "First"} change — press again to wrap to the ${last ? "first" : "last"}`,
      dir,
    );
  }
  /** Keep the "N / M changes" counter in step with scrolling: it should say
   *  where you ARE, not where you last jumped from. Same `position()` the
   *  stepping uses, so the counter and the buttons always agree. */
  function syncCounter() {
    if (!blocks.length) return;
    current = position().idx;
  }
  /** Scrolling means the user moved on: drop any pending wrap offer. */
  function onBodyScroll() {
    if (hint) clearHint();
    syncCounter();
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "F7" && e.shiftKey) {
      e.preventDefault();
      step(-1);
    } else if (e.key === "F7") {
      e.preventDefault();
      step(1);
    } else if (e.key === "Escape") {
      window.close();
    }
  }

  // A renderable run of a cell: text + syntax color + changed-range membership.
  type Piece = { t: string; c?: string; m: boolean };

  // Combine a line's syntax tokens with the intra-line changed range: split the
  // colored runs at the range boundaries so the change marker overlays cleanly.
  // Falls back to one plain run without tokens (or if they don't cover the text).
  function pieces(text: string, toks: TokenRun[] | undefined, range?: [number, number]): Piece[] {
    let base: Piece[];
    if (toks && toks.reduce((n, t) => n + t.content.length, 0) === text.length) {
      base = toks.map((t) => ({ t: t.content, c: t.color, m: false }));
    } else {
      base = text ? [{ t: text, m: false }] : [];
    }
    if (!range || range[0] === range[1]) return base;
    const out: Piece[] = [];
    let pos = 0;
    for (const p of base) {
      let s = 0;
      while (s < p.t.length) {
        let end = p.t.length;
        for (const b of range) {
          const rel = b - pos;
          if (rel > s && rel < end) end = rel;
        }
        const abs = pos + s;
        out.push({ t: p.t.slice(s, end), c: p.c, m: abs >= range[0] && abs < range[1] });
        s = end;
      }
      pos += p.t.length;
    }
    return out;
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="app">
  <div class="head">
    <span class="label mono" title={leftPath}>{leftLabel}</span>
    <div class="nav">
      <!-- Enabled whenever there are changes: the step direction is resolved
           against the visible position, not a remembered index. -->
      <button onclick={() => step(-1)} disabled={!blocks.length} title="Previous change (Shift+F7)">
        ▲
      </button>
      <span class="count dim">
        {#if loading}…{:else if blocks.length}{current + 1} / {blocks.length} changes{:else}no changes{/if}
      </span>
      <!-- Floating bubble (like the main window's toasts) so showing it never
           reflows the nav buttons. -->
      {#if hint}
        <div class="navhint">{hint}</div>
      {/if}
      <button onclick={() => step(1)} disabled={!blocks.length} title="Next change (F7)">▼</button>
    </div>
    <span class="label right mono" title={rightPath}>{rightLabel}</span>
  </div>

  <div class="scroll body" bind:this={body} onscroll={onBodyScroll}>
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if error}
      <div class="msg err">{error}</div>
    {:else if rows.length === 0}
      <div class="msg dim">Both files are empty.</div>
    {:else}
      <div class="grid mono">
        {#each rows as row, i (i)}
          {@const lp = row.l ? pieces(row.l.text, ltoks?.[row.l.no - 1], row.lh) : []}
          {@const rp = row.r ? pieces(row.r.text, rtoks?.[row.r.no - 1], row.rh) : []}
          <span class="num" class:hl={row.type === "del" || row.type === "mod"} data-row={i}>
            {row.l?.no ?? ""}
          </span>
          <span
            class="code left"
            class:del={row.type === "del" || row.type === "mod"}
            class:void={!row.l}
          >{#each lp as p, j (j)}<span class="run" class:chg-del={p.m} style:color={p.c}>{p.t}</span>{/each}</span>
          <span class="num sep" class:hl={row.type === "add" || row.type === "mod"}>
            {row.r?.no ?? ""}
          </span>
          <span
            class="code"
            class:add={row.type === "add" || row.type === "mod"}
            class:void={!row.r}
          >{#each rp as p, j (j)}<span class="run" class:chg-add={p.m} style:color={p.c}>{p.t}</span>{/each}</span>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--bg);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-alt);
    flex: none;
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .label.right {
    text-align: right;
  }
  .nav {
    position: relative; /* anchors .navhint */
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  /* End-of-list feedback: floats under the change counter, so it never moves the
     buttons (same reasoning as the main window's toasts). */
  .navhint {
    position: absolute;
    top: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    z-index: 10;
    white-space: nowrap;
    font-size: 11px;
    color: var(--text);
    background: var(--bg-panel);
    border: 1px solid var(--accent);
    border-radius: 6px;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
    padding: 4px 10px;
    pointer-events: none;
  }
  .nav button {
    font-size: 10px;
    padding: 2px 8px;
  }
  .count {
    font-size: 11px;
    white-space: nowrap;
  }
  .body {
    flex: 1;
    min-height: 0;
  }
  .msg {
    padding: 20px;
    font-size: 12px;
  }
  .err {
    color: var(--warn);
  }
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto minmax(0, 1fr);
    font-size: 12px;
    line-height: 1.5;
    min-height: 100%;
  }
  .num {
    text-align: right;
    padding: 0 8px;
    color: var(--text-dim);
    -webkit-user-select: none;
    user-select: none;
    white-space: nowrap;
    background: var(--bg-alt);
  }
  .num.sep {
    border-left: 1px solid var(--border);
  }
  .num.hl {
    color: var(--text);
  }
  .code {
    padding: 0 8px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    word-break: break-word;
  }
  .code.left {
    border-right: 1px solid var(--border);
  }
  /* Line-level tints (same language as the inline DiffView). */
  .del {
    background: rgba(192, 57, 43, 0.14);
  }
  .add {
    background: rgba(31, 157, 85, 0.14);
  }
  /* A side that has no line at this row (pure add/del alignment gap). */
  .void {
    background:
      repeating-linear-gradient(
        -45deg,
        rgba(128, 128, 128, 0.06),
        rgba(128, 128, 128, 0.06) 4px,
        transparent 4px,
        transparent 8px
      );
  }
  /* Intra-line changed range (mod rows) — stronger than the line tint; sits
     under the syntax color (background only, text color comes from the token). */
  .run {
    border-radius: 2px;
  }
  .chg-del {
    background: rgba(192, 57, 43, 0.35);
  }
  .chg-add {
    background: rgba(31, 157, 85, 0.35);
  }
</style>
