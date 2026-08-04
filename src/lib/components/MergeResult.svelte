<script lang="ts">
  //! The editable result pane, drawn from the merge document model.
  //!
  //! We own the caret and the rendering; the browser only ever sees a hidden
  //! textarea that absorbs input. That is how editors do it, and it is why dead
  //! keys and IME work: composition happens in a real text control positioned at
  //! the caret, and we take the finished text from it. Nothing the browser does to
  //! that textarea can touch the document.
  import { tick, type Snippet } from "svelte";
  import type { Caret, DocState, MergeAction } from "$lib/mergedoc";
  import type { TokenRun } from "$lib/syntax";

  // NOT named `state`: that shadows the $state rune, and `$state` would then read
  // as a store subscription.
  let {
    docState,
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

  /** The caret's pixel position: which row it sits on, and its column. */
  const caretAt = $derived.by(() => {
    const i = docState.doc.regions.findIndex((r) => r.region === docState.caret.region);
    if (i < 0) return null;
    const r = docState.doc.regions[i];
    const top =
      tops[i] + (r.conflict ? toolbarHeight : 0) + Math.min(docState.caret.line, r.lines.length) * lineHeight;
    return { top, col: docState.caret.col };
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

  /** Place the caret from a click: the row gives region+line, x gives the column. */
  function place(e: MouseEvent) {
    if (!pane) return;
    const row = (e.target as HTMLElement).closest("[data-rgn]") as HTMLElement | null;
    const box = pane.getBoundingClientRect();
    const y = e.clientY - box.top;
    // Which region owns this y, then which of its lines.
    let idx = 0;
    for (let i = 0; i < tops.length; i++) if (y >= tops[i]) idx = i;
    const r = docState.doc.regions[idx];
    const inner = y - tops[idx] - (r.conflict ? toolbarHeight : 0);
    const line = Math.max(0, Math.min(Math.floor(inner / lineHeight), Math.max(0, r.lines.length - 1)));
    const gut = pane.querySelector(".code") as HTMLElement | null;
    const charW = charWidth();
    const left = gut ? gut.getBoundingClientRect().left : box.left;
    const col = Math.max(0, Math.round((e.clientX - left) / charW));
    void row;
    act({ t: "caret", caret: { region: r.region, line, col } });
    focus();
  }

  /** Monospace: one probe is enough, and it follows zoom because we re-measure. */
  function charWidth(): number {
    const probe = pane?.querySelector(".probe") as HTMLElement | null;
    return probe ? probe.getBoundingClientRect().width / 10 : 7;
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
    if (ctrl) return; // leave the rest of the ctrl space alone for now
    const moves: Record<string, MergeAction> = {
      ArrowLeft: { t: "move", dir: "left" },
      ArrowRight: { t: "move", dir: "right" },
      ArrowUp: { t: "move", dir: "up" },
      ArrowDown: { t: "move", dir: "down" },
      Home: { t: "move", dir: "home" },
      End: { t: "move", dir: "end" },
    };
    if (moves[e.key]) {
      e.preventDefault();
      act(moves[e.key]);
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
  onmousedown={place}
  onfocus={focus}
>
  <!-- Width probe for column arithmetic; ten characters of the real font. -->
  <span class="probe" aria-hidden="true">0000000000</span>

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

  {#if caretAt}
    <div class="caret" style="top:{caretAt.top}px; left:calc(var(--gut) + {caretAt.col}ch)"></div>
  {/if}

  <!-- The input sink: invisible, but a real text control at the caret, so dead
       keys and IME candidate windows behave normally. -->
  <textarea
    class="sink"
    bind:this={sink}
    style="top:{caretAt?.top ?? 0}px; left:calc(var(--gut) + {caretAt?.col ?? 0}ch)"
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
