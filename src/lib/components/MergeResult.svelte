<script lang="ts">
  //! The editable result pane, drawn from the merge document model.
  //!
  //! We own the caret and the rendering; the browser only ever sees a hidden
  //! textarea that absorbs input. That is how editors do it, and it is why dead
  //! keys and IME work: composition happens in a real text control positioned at
  //! the caret, and we take the finished text from it. Nothing the browser does to
  //! that textarea can touch the document.
  import { tick, type Snippet } from "svelte";
  import { orderCarets, sameCaret, type Caret, type DocState, type MergeAction } from "$lib/mergedoc";
  import type { TokenRun } from "$lib/syntax";

  // NOT named `state`: that shadows the $state rune, and `$state` would then read
  // as a store subscription.
  let {
    docState,
    anchor,
    rows,
    starts,
    kinds,
    tokens,
    lineHeight,
    toolbarHeight,
    toolbar,
    onAction,
  }: {
    docState: DocState;
    /** The fixed end of the selection; null when there is none. */
    anchor: Caret | null;
    /** Aligned row count per region index (max of the three panes). */
    rows: number[];
    /** First line number of each region in the merged file. */
    starts: number[];
    /** Band kind per region: add | del | vs | keep | "". */
    kinds: string[];
    tokens: Map<string, TokenRun[]>;
    lineHeight: number;
    toolbarHeight: number;
    /** Rendered inside a conflict region, above its lines. */
    toolbar: Snippet<[number]>;
    onAction: (a: MergeAction) => void;
  } = $props();

  const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };

  let sink: HTMLTextAreaElement | undefined = $state();
  let pane: HTMLDivElement | undefined = $state();
  let composing = $state(false);

  /** Top of each region, in px: rows above it plus a strip for each conflict. */
  const tops = $derived.by(() => {
    let y = 0;
    return docState.doc.regions.map((r, i) => {
      const at = y;
      y += (rows[i] ?? r.lines.length) * lineHeight + (r.conflict ? toolbarHeight : 0);
      return at;
    });
  });
  const total = $derived(
    docState.doc.regions.reduce(
      (sum, r, i) => sum + (rows[i] ?? r.lines.length) * lineHeight + (r.conflict ? toolbarHeight : 0),
      0,
    ),
  );

  /** The caret's row, and the text before it on that row. */
  const caretAt = $derived.by(() => {
    const i = docState.doc.regions.findIndex((r) => r.region === docState.caret.region);
    if (i < 0) return null;
    const r = docState.doc.regions[i];
    const top =
      tops[i] + (r.conflict ? toolbarHeight : 0) + Math.min(docState.caret.line, r.lines.length) * lineHeight;
    const line = r.lines[docState.caret.line] ?? "";
    return { top, prefix: line.slice(0, docState.caret.col) };
  });

  let probe: HTMLSpanElement | undefined = $state();
  let caretLeft = $state(0);

  /** Width of `text` as this pane renders it. The probe carries the same font,
   *  white-space and tab-size as a code line, so measuring beats computing:
   *  tabs, ch units and font metrics cannot get out of step with the rendering. */
  function widthOf(text: string): number {
    if (!probe) return 0;
    probe.textContent = text;
    return probe.getBoundingClientRect().width;
  }
  /** Where the code column starts, read from a real line rather than assumed. */
  function gutter(): number {
    const code = pane?.querySelector(".code") as HTMLElement | null;
    return code ? code.offsetLeft : 0;
  }

  /** Rectangles covering the selection, one per selected line. */
  const bands = $derived.by(() => {
    if (!anchor || sameCaret(anchor, docState.caret) || !probe) return [];
    const { from, to } = orderCarets(docState.doc, anchor, docState.caret);
    const fi = docState.doc.regions.findIndex((r) => r.region === from.region);
    const ti = docState.doc.regions.findIndex((r) => r.region === to.region);
    if (fi < 0 || ti < 0) return [];
    const gut = gutter();
    const out: { top: number; left: number; width: number }[] = [];
    for (let i = fi; i <= ti; i++) {
      const r = docState.doc.regions[i];
      const base = tops[i] + (r.conflict ? toolbarHeight : 0);
      const first = i === fi ? from.line : 0;
      const last = i === ti ? to.line : r.lines.length - 1;
      for (let l = first; l <= last && l < r.lines.length; l++) {
        const line = r.lines[l];
        const a0 = i === fi && l === from.line ? from.col : 0;
        const b0 = i === ti && l === to.line ? to.col : line.length;
        const x1 = gut + widthOf(line.slice(0, a0));
        const x2 = gut + widthOf(line.slice(0, b0));
        // A selected line-break shows as a sliver, so an empty line still reads
        // as selected.
        const w = Math.max(x2 - x1, b0 >= line.length && !(i === ti && l === to.line) ? 4 : 0);
        out.push({ top: base + l * lineHeight, left: x1, width: w });
      }
    }
    return out;
  });

  // Re-measure whenever the caret or the text under it changes.
  $effect(() => {
    const c = caretAt;
    if (!c || !probe) return;
    caretLeft = gutter() + widthOf(c.prefix);
  });

  /** Keep the caret visible without yanking the view around. */
  async function reveal() {
    await tick();
    if (!pane || !caretAt) return;
    const box = pane.closest(".scroll") as HTMLElement | null;
    if (!box) return;
    const y = pane.offsetTop + caretAt.top;
    if (y < box.scrollTop + lineHeight) box.scrollTop = Math.max(0, y - lineHeight * 3);
    else if (y + lineHeight > box.scrollTop + box.clientHeight - lineHeight)
      box.scrollTop = y - box.clientHeight + lineHeight * 4;
  }

  export function focus() {
    sink?.focus({ preventScroll: true });
  }

  function act(a: MergeAction) {
    onAction(a);
    void reveal();
  }

  let dragging = $state(false);
  let lastMouse: { x: number; y: number } | null = null;
  let scroller: number | null = null;

  function scrollBox(): HTMLElement | null {
    return (pane?.closest(".scroll") as HTMLElement) ?? null;
  }
  /** Rows that fit in the viewport — the size of a page. */
  function viewportRows(): number {
    const box = scrollBox();
    return box ? box.clientHeight / lineHeight : 20;
  }

  /** While dragging past an edge, keep scrolling and keep extending. */
  function autoscroll() {
    if (scroller !== null) return;
    scroller = window.setInterval(() => {
      const box = scrollBox();
      if (!dragging || !box || !lastMouse) return stopScroll();
      const r = box.getBoundingClientRect();
      const above = r.top + lineHeight - lastMouse.y;
      const below = lastMouse.y - (r.bottom - lineHeight);
      const step = above > 0 ? -Math.min(4 * lineHeight, above) : below > 0 ? Math.min(4 * lineHeight, below) : 0;
      if (!step) return;
      box.scrollTop += step;
      // The content moved under the pointer, so re-read the position from it.
      extendTo(lastMouse.x, lastMouse.y);
    }, 50);
  }
  function stopScroll() {
    if (scroller !== null) window.clearInterval(scroller);
    scroller = null;
  }

  /** Which region, line and column a point maps to. */
  function hit(clientX: number, clientY: number): Caret | null {
    if (!pane) return null;
    const box = pane.getBoundingClientRect();
    const y = clientY - box.top;
    // Which region owns this y, then which of its lines.
    let idx = 0;
    for (let i = 0; i < tops.length; i++) if (y >= tops[i]) idx = i;
    const r = docState.doc.regions[idx];
    const inner = y - tops[idx] - (r.conflict ? toolbarHeight : 0);
    const line = Math.max(0, Math.min(Math.floor(inner / lineHeight), Math.max(0, r.lines.length - 1)));
    const x = clientX - box.left - gutter();
    return { region: r.region, line, col: columnAtX(r.lines[line] ?? "", x) };
  }

  function place(e: MouseEvent, extend = false) {
    const caret = hit(e.clientX, e.clientY);
    if (!caret) return;
    act({ t: "caret", caret, extend });
    focus();
  }
  function extendTo(clientX: number, clientY: number) {
    const caret = hit(clientX, clientY);
    if (caret) act({ t: "caret", caret, extend: true });
  }

  function onDown(e: MouseEvent) {
    if (e.button !== 0) return;
    const caret = hit(e.clientX, e.clientY);
    if (!caret) return;
    focus();
    if (e.detail >= 3) {
      act({ t: "selectLine", caret });
      return;
    }
    if (e.detail === 2) {
      act({ t: "selectWord", caret });
      return;
    }
    dragging = true;
    lastMouse = { x: e.clientX, y: e.clientY };
    act({ t: "caret", caret, extend: e.shiftKey });
  }
  function onMove(e: MouseEvent) {
    if (!dragging) return;
    e.preventDefault();
    lastMouse = { x: e.clientX, y: e.clientY };
    extendTo(e.clientX, e.clientY);
    autoscroll();
  }
  function onUp() {
    dragging = false;
    lastMouse = null;
    stopScroll();
  }

  /** The character index whose rendered edge is nearest `x`, by binary search over
   *  measured prefixes — correct through tabs and any font. */
  function columnAtX(line: string, x: number): number {
    if (x <= 0) return 0;
    let lo = 0;
    let hi = line.length;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (widthOf(line.slice(0, mid)) <= x) lo = mid;
      else hi = mid - 1;
    }
    if (lo >= line.length) return line.length;
    const before = widthOf(line.slice(0, lo));
    const after = widthOf(line.slice(0, lo + 1));
    return x - before <= after - x ? lo : lo + 1;
  }

  function onKey(e: KeyboardEvent) {
    if (composing) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && e.key.toLowerCase() === "z") {
      e.preventDefault();
      act(e.shiftKey ? { t: "redo" } : { t: "undo" });
      return;
    }
    if (ctrl && e.key.toLowerCase() === "y") {
      e.preventDefault();
      act({ t: "redo" });
      return;
    }
    if (ctrl && e.key.toLowerCase() === "a") {
      e.preventDefault();
      act({ t: "selectAll" });
      return;
    }
    if (ctrl && (e.key.toLowerCase() === "c" || e.key.toLowerCase() === "x")) {
      e.preventDefault();
      act({ t: "copy", cut: e.key.toLowerCase() === "x" });
      return;
    }
    if (ctrl && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      act({ t: "move", dir: e.key === "ArrowLeft" ? "wordLeft" : "wordRight", extend: e.shiftKey });
      return;
    }
    if (ctrl && (e.key === "Backspace" || e.key === "Delete")) {
      e.preventDefault();
      act({ t: "deleteWord", forward: e.key === "Delete" });
      return;
    }
    if (ctrl) return; // leave the rest of the ctrl space alone for now
    const moves: Record<string, MergeAction> = {
      ArrowLeft: { t: "move", dir: "left" },
      ArrowRight: { t: "move", dir: "right" },
      ArrowUp: { t: "move", dir: "up" },
      ArrowDown: { t: "move", dir: "down" },
      Home: { t: "move", dir: "home" },
      End: { t: "move", dir: "end" },
    };
    const move = moves[e.key];
    if (move) {
      e.preventDefault();
      act(move.t === "move" ? { ...move, extend: e.shiftKey } : move);
      return;
    }
    if (e.key === "PageUp" || e.key === "PageDown") {
      e.preventDefault();
      const page = Math.max(1, Math.floor(viewportRows()) - 1);
      act({ t: "moveLines", delta: e.key === "PageUp" ? -page : page, extend: e.shiftKey });
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      act({ t: "enter" });
      return;
    }
    if (e.key === "Backspace") {
      e.preventDefault();
      act({ t: "backspace" });
      return;
    }
    if (e.key === "Delete") {
      e.preventDefault();
      act({ t: "delete" });
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      act({ t: "insert", text: "\t" });
    }
  }

  /** Text arrives here: plain typing, dead-key results, IME output and paste. */
  function onInput() {
    if (!sink || composing) return;
    const text = sink.value;
    sink.value = "";
    if (text) act({ t: "insert", text });
  }
  function onPaste(e: ClipboardEvent) {
    const text = e.clipboardData?.getData("text/plain");
    if (text === undefined) return;
    e.preventDefault();
    act({ t: "insert", text });
  }
</script>

{#snippet codeOf(line: string)}
  {#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span style:color={run.color}
        >{run.content}</span
      >{/each}{:else}{line}{/if}
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="pane"
  bind:this={pane}
  style="height:{total}px; --lh:{lineHeight}px"
  role="textbox"
  aria-multiline="true"
  aria-label="merged result"
  tabindex="0"
  onmousedown={onDown}
  onmousemove={onMove}
  onmouseup={onUp}
  onmouseleave={onUp}
  ondblclick={(e) => e.preventDefault()}
  onfocus={focus}
>
  <!-- Measuring probe: same font, white-space and tab-size as a code line. -->
  <span class="probe" bind:this={probe} aria-hidden="true"></span>

  {#each docState.doc.regions as r, i (r.region)}
    {@const kind = kinds[i] ?? ""}
    <div
      class="rgn"
      class:conflict={r.conflict}
      data-rgn={r.region}
      style="top:{tops[i]}px; height:{(rows[i] ?? r.lines.length) * lineHeight +
        (r.conflict ? toolbarHeight : 0)}px"
    >
      {#if r.conflict}
        <div class="bar" style="height:{toolbarHeight}px">{@render toolbar(r.region)}</div>
      {/if}
      {#each r.lines as line, k (k)}
        <div class="rl k-{kind}" style="height:{lineHeight}px">
          <span class="mk">{MARK[kind] ?? ""}</span><span class="ln">{starts[i] + k}</span><span
            class="code">{@render codeOf(line)}</span
          >
        </div>
      {/each}
    </div>
  {/each}

  {#each bands as b (b.top + ":" + b.left)}
    <div class="sel" style="top:{b.top}px; left:{b.left}px; width:{b.width}px"></div>
  {/each}

  {#if caretAt}
    <div class="caret" style="top:{caretAt.top}px; left:{caretLeft}px"></div>
  {/if}

  <!-- The input sink: invisible, but a real text control at the caret, so dead
       keys and IME candidate windows behave normally. -->
  <textarea
    class="sink"
    bind:this={sink}
    style="top:{caretAt?.top ?? 0}px; left:{caretLeft}px"
    spellcheck="false"
    autocapitalize="off"
    autocomplete="off"
    oninput={onInput}
    onkeydown={onKey}
    onpaste={onPaste}
    oncompositionstart={() => (composing = true)}
    oncompositionend={() => {
      composing = false;
      onInput();
    }}
></textarea>
</div>

<style>
  .pane {
    position: relative;
    min-width: 0;
    /* mark + line number + gap, matching the read-only panes */
    --gut: calc(4.2em + 8px);
    font-family: var(--mono, ui-monospace, Consolas, monospace);
    font-size: 12px;
    cursor: text;
    outline: none;
  }
  .probe {
    position: absolute;
    visibility: hidden;
    white-space: pre;
    tab-size: 4;
    top: 0;
    left: 0;
  }
  .rgn {
    position: absolute;
    left: 0;
    right: 0;
    overflow: hidden;
  }
  .rgn.conflict {
    background: rgba(224, 85, 90, 0.09);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 6px;
    box-sizing: border-box;
    background: rgba(224, 85, 90, 0.16);
  }
  .rl {
    display: flex;
    align-items: flex-start;
    line-height: var(--lh);
    border-left: 3px solid transparent;
    box-sizing: border-box;
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
  .code {
    white-space: pre;
    tab-size: 4; /* must match TAB_WIDTH */
    min-width: 0;
    padding-right: 6px;
  }
  /* Colour says add / drop, never which side — same scale as the other panes. */
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
  .sel {
    position: absolute;
    height: var(--lh);
    background: rgba(217, 141, 58, 0.28);
    pointer-events: none;
  }
  .caret {
    position: absolute;
    width: 2px;
    height: var(--lh);
    background: var(--accent, #d98d3a);
    animation: blink 1.1s steps(1) infinite;
    pointer-events: none;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  /* Invisible, but present and focusable: composition needs a real control. */
  .sink {
    position: absolute;
    width: 1px;
    height: var(--lh);
    padding: 0;
    border: 0;
    outline: none;
    resize: none;
    overflow: hidden;
    opacity: 0;
    background: transparent;
    color: transparent;
    caret-color: transparent;
    font: inherit;
  }
</style>
