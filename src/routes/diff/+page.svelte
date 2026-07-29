<script lang="ts">
  import { onMount } from "svelte";
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
      if (blocks.length) goTo(0); // land on the first change, like P4Merge
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

  function goTo(i: number) {
    if (!blocks.length) return;
    current = Math.max(0, Math.min(blocks.length - 1, i));
    const el = body?.querySelector(`[data-row="${blocks[current]}"]`);
    el?.scrollIntoView({ block: "center" });
  }
  /** The row at the VERTICAL CENTRE of the viewport — the anchor for "where am I".
   *  Navigation follows where you scrolled rather than the last button press, and
   *  the centre (not the top) is the right reference because goTo centres its
   *  target: anchoring on the top row would leave a just-centred block still
   *  "below" the anchor, so Next would keep re-finding and re-centring it instead
   *  of reporting that there's nothing further. */
  function anchorRow(): number {
    if (!body) return 0;
    const box = body.getBoundingClientRect();
    const mid = box.top + box.height / 2;
    let last = 0;
    for (const el of body.querySelectorAll<HTMLElement>("[data-row]")) {
      const b = el.getBoundingClientRect();
      last = Number(el.dataset.row);
      if (b.bottom > mid) return last; // first row reaching the centre line
    }
    return last;
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

  /** Step to the next/previous change block relative to the visible position. */
  function step(dir: 1 | -1) {
    if (!blocks.length) return;
    const anchor = anchorRow();
    // The next block starting after the centre, or the last one starting before
    // it (inside a long block, that's the block's own start — so Previous takes
    // you to the top of the change you're reading, then to the one before).
    let i = -1;
    if (dir === 1) {
      i = blocks.findIndex((r) => r > anchor);
    } else {
      for (let k = 0; k < blocks.length; k++) {
        if (blocks[k] < anchor) i = k;
        else break;
      }
    }
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
   *  where you ARE, not where you last jumped from. */
  function syncCounter() {
    if (hint) clearHint(); // moved on — the wrap offer no longer applies
    if (!blocks.length) return;
    const anchor = anchorRow();
    let i = 0;
    for (let k = 0; k < blocks.length; k++) {
      if (blocks[k] <= anchor) i = k; // last block start at/above the centre
      else break;
    }
    current = i;
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

  <div class="scroll body" bind:this={body} onscroll={syncCounter}>
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
