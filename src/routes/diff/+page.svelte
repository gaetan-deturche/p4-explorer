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
  import { diffLines, type DiffRow } from "$lib/linediff";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import MergeResult from "$lib/components/MergeResult.svelte";
  import {
    clampCaret,
    deleteBackward,
    deleteForward,
    deleteRange,
    deleteWord,
    docText,
    emptyHistory,
    insertLineBreak,
    insertOverRange,
    insertText,
    lineRange,
    moveByLines,
    moveLeft,
    moveLineEnd,
    moveLineStart,
    moveRight,
    moveVertical,
    push,
    redo,
    sameCaret,
    selectAll,
    selectedText,
    undo,
    wordLeft,
    wordRange,
    wordRight,
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

  const LH = 17.4; // one row: 12px * 1.45
  const TOOLBAR = 0; // the diff has no per-region toolbar

  /** One run of rows that are all the same kind of change. */
  interface Block {
    kind: "same" | "del" | "add" | "mod";
    left: string[];
    right: string[];
    leftFrom: number; // 1-based line number in the left file
  }

  let leftText = "";
  let blocks = $state<Block[]>([]);
  let ds = $state<DocState | null>(null);
  let hist = $state<History>(emptyHistory());
  let anchor = $state<Caret | null>(null);
  let dirty = $state(false);
  let saving = $state(false);
  let error = $state("");
  let loading = $state(true);
  let tokens = $state<Map<string, TokenRun[]>>(new Map());
  let current = $state(0);
  let typing = false;

  const changes = $derived(blocks.map((b, i) => (b.kind === "same" ? -1 : i)).filter((i) => i >= 0));

  /** Group the aligned rows into runs — the regions of the document. */
  function toBlocks(rows: DiffRow[]): Block[] {
    const out: Block[] = [];
    for (const row of rows) {
      const last = out[out.length - 1];
      if (last && last.kind === row.type) {
        if (row.l) last.left.push(row.l.text);
        if (row.r) last.right.push(row.r.text);
        continue;
      }
      out.push({
        kind: row.type,
        left: row.l ? [row.l.text] : [],
        right: row.r ? [row.r.text] : [],
        leftFrom: row.l?.no ?? (last ? last.leftFrom + last.left.length : 1),
      });
    }
    return out;
  }

  /** Colour by add / drop, the same scale as the resolve window. */
  function leftKind(b: Block): string {
    return b.kind === "same" ? "" : "del";
  }
  const kinds = $derived(blocks.map((b) => (b.kind === "same" ? "" : "add")));

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
  function rebuild(rightText: string, caret: Caret | number) {
    blocks = toBlocks(diffLines(leftText, rightText));
    const regions = blocks.map((b, i) => ({
      region: i,
      kind: b.kind === "same" ? "" : "add",
      conflict: false,
      lines: b.right.slice(),
    }));
    let target: Caret = { region: 0, line: 0, col: 0 };
    if (typeof caret === "number") {
      let n = caret;
      for (const r of regions) {
        if (n < r.lines.length) {
          target = { region: r.region, line: n, col: 0 };
          break;
        }
        n -= r.lines.length;
        target = { region: r.region, line: Math.max(0, r.lines.length - 1), col: 0 };
      }
    } else {
      target = caret;
    }
    ds = { doc: { regions }, caret: target };
    anchor = null;
  }

  /** The caret's line counted from the top of the right file. */
  function absoluteLine(): number {
    if (!ds) return 0;
    let n = 0;
    for (const r of ds.doc.regions) {
      if (r.region === ds.caret.region) return n + ds.caret.line;
      n += r.lines.length;
    }
    return n;
  }

  function goTo(n: number) {
    if (!changes.length) return;
    current = ((n % changes.length) + changes.length) % changes.length;
    document.querySelector(`[data-change="${changes[current]}"]`)?.scrollIntoView({ block: "center" });
  }

  /** A live selection, or null. */
  function selection(): { from: Caret; to: Caret } | null {
    if (!ds || !anchor || sameCaret(anchor, ds.caret)) return null;
    return { from: anchor, to: ds.caret };
  }

  /** Apply an intent to the document. Identical handling to the resolve window,
   *  because it is the same model. */
  function apply(a: MergeAction) {
    if (!ds || !editable) return;
    const before = ds;
    const sel = selection();
    const edit = (next: DocState, coalesce = false) => {
      hist = push(hist, before, coalesce);
      ds = next;
      anchor = null;
      dirty = true;
    };
    switch (a.t) {
      case "insert":
        edit(sel ? insertOverRange(before, sel.from, sel.to, a.text) : insertText(before, a.text), typing && !sel);
        typing = true;
        break;
      case "enter":
        typing = false;
        edit(sel ? insertOverRange(before, sel.from, sel.to, "\n") : insertLineBreak(before));
        break;
      case "backspace":
        typing = false;
        edit(sel ? deleteRange(before, sel.from, sel.to) : deleteBackward(before));
        break;
      case "delete":
        typing = false;
        edit(sel ? deleteRange(before, sel.from, sel.to) : deleteForward(before));
        break;
      case "deleteWord":
        typing = false;
        edit(sel ? deleteRange(before, sel.from, sel.to) : deleteWord(before, a.forward));
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
        anchor = a.extend ? (anchor ?? before.caret) : null;
        ds = { doc: before.doc, caret: next };
        break;
      }
      case "moveLines":
        typing = false;
        anchor = a.extend ? (anchor ?? before.caret) : null;
        ds = { doc: before.doc, caret: moveByLines(before.doc, before.caret, a.delta) };
        break;
      case "caret":
        typing = false;
        anchor = a.extend ? (anchor ?? before.caret) : null;
        ds = { doc: before.doc, caret: clampCaret(before.doc, a.caret) };
        break;
      case "selectWord": {
        const w = wordRange(before.doc, a.caret);
        anchor = w.from;
        ds = { doc: before.doc, caret: w.to };
        break;
      }
      case "selectLine": {
        const l = lineRange(before.doc, a.caret);
        anchor = l.from;
        ds = { doc: before.doc, caret: l.to };
        break;
      }
      case "selectAll": {
        const all = selectAll(before.doc);
        if (all) {
          anchor = all.anchor;
          ds = { doc: before.doc, caret: all.head };
        }
        break;
      }
      case "copy": {
        if (!sel) break;
        void setClipboard(selectedText(before.doc, sel.from, sel.to)).catch((e) => (error = String(e)));
        if (a.cut) edit(deleteRange(before, sel.from, sel.to));
        break;
      }
      case "undo": {
        const u = undo(hist, before);
        if (u) {
          ds = u.state;
          hist = u.history;
          anchor = null;
          dirty = true;
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
          anchor = null;
          dirty = true;
        }
        break;
      }
    }
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
{#snippet pane(lines: string[], from: number, kind: string)}
  {#each lines as line, k}
    <div class="line k-{kind}"><span class="mk">{kind === "del" ? "-" : ""}</span><span class="ln"
        >{from + k}</span
      ><span class="src"
        >{#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
{/snippet}

{#snippet noToolbar(_region: number)}{/snippet}

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
    <span class="legend dim">
      <span class="chip add">+ local</span><span class="chip del">− other side</span>
    </span>
    {#if changes.length > 1}
      <button onclick={() => goTo(current - 1)} title="Previous change">▲</button>
      <span class="dim">{current + 1}/{changes.length}</span>
      <button onclick={() => goTo(current + 1)} title="Next change">▼</button>
    {/if}
    <button onclick={close}>{dirty ? "Close without saving" : "Close"}</button>
    {#if editable}
      <button class="primary" disabled={!dirty || saving} onclick={save} title={rightPath}>
        {saving ? "Saving…" : "Save"}
      </button>
    {/if}
  </div>

  {#if error}
    <div class="err mono">{error}</div>
  {:else if loading || !ds}
    <div class="dim pad">Loading…</div>
  {:else}
    <div class="scroll">
      <div class="grid mono">
        <div class="head">{leftLabel}</div>
        <div class="head mid">
          {rightLabel}{#if editable}<span class="dim"> — editable</span>{/if}
        </div>

        <div class="col" style="height:{total}px">
          {#each blocks as b, i (i)}
            <div
              class="rgn"
              class:change={b.kind !== "same"}
              data-change={b.kind === "same" ? undefined : i}
              style="top:{tops[i]}px; height:{rows[i] * LH}px"
            >
              {@render pane(b.left, starts[i].l, leftKind(b))}
            </div>
          {/each}
        </div>

        <div class="resultcol">
          <MergeResult
            docState={ds}
            {anchor}
            {rows}
            starts={starts.map((s) => s.r)}
            {kinds}
            {tokens}
            lineHeight={LH}
            toolbarHeight={TOOLBAR}
            toolbar={noToolbar}
            onAction={apply}
          />
        </div>
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
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
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
  .col {
    position: relative;
    border-right: 1px solid var(--border, #333);
    overflow: hidden;
    min-width: 0;
  }
  .resultcol {
    background: rgba(255, 255, 255, 0.02);
    overflow: hidden;
    min-width: 0;
  }
  .rgn {
    position: absolute;
    left: 0;
    right: 0;
    overflow: hidden;
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
