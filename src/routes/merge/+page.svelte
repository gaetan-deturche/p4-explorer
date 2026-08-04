<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { langForFile, tokenizeLines, type TokenRun } from "$lib/syntax";
  import type { MergeData, MergeRegion } from "$lib/p4";

  // Opened by the Rust `open_merge_window` command; the job itself is fetched by
  // id (regions don't fit in a query string).
  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  type Side = "theirs" | "ours";

  let data = $state<MergeData | null>(null);
  let error = $state("");
  let saving = $state(false);
  /** Region → the result text for it. Set means settled; the side buttons only
   *  seed it, so anything can be typed over afterwards. */
  let edited = $state<Record<number, string>>({});
  /** Region → where its text came from, for the origin arrows. */
  let origin = $state<Record<number, string>>({});
  let showBase = $state<Record<number, boolean>>({});
  let current = $state(0); // which conflict the prev/next buttons are on
  /** line text → colored runs, from tokenizing all three sides once. */
  let tokens = $state<Map<string, TokenRun[]>>(new Map());

  // The region being typed in. While it is live its lines are rendered from
  // `frozen` — a plain array, deliberately NOT reactive — so nothing re-renders
  // under the caret while the browser owns that subtree.
  let live = $state<number | null>(null);
  // A plain box, never $state and never reassigned: reading it must not create a
  // dependency, or the subtree the caret sits in would re-render as state moves.
  const frozen: { lines: string[] } = { lines: [] };

  const regions = $derived(data?.regions ?? []);
  const conflicts = $derived(
    regions.map((r, i) => (r.kind === "conflict" ? i : -1)).filter((i) => i >= 0),
  );
  const unsettled = $derived(conflicts.filter((i) => edited[i] === undefined));
  const resultLines = $derived(regions.flatMap((r, i) => linesFor(r, i)));

  /** The result text for a region: what was typed, else the auto-merge. A
   *  conflict with nothing taken or typed yet has no text at all. */
  function resultText(r: MergeRegion, i: number): string | null {
    if (edited[i] !== undefined) return edited[i];
    return r.kind === "conflict" ? null : r.lines.join("\n");
  }
  function linesFor(r: MergeRegion, i: number): string[] {
    const t = resultText(r, i);
    return t === null || t === "" ? [] : t.split("\n");
  }

  /** What a side pane shows: its own text, or the base where it didn't change. */
  function side(r: MergeRegion, which: Side): string[] {
    if (r.kind === "same" || r.kind === "both") return r.lines;
    if (r.kind === "conflict") return r[which];
    return r.kind === which ? r.lines : r.base;
  }

  // --- add / drop, the same meaning in every pane ---------------------------
  // "add" = text the merge keeps, "del" = base text it drops, "!" = contested.
  // Which SIDE a change came from is shown by the arrows, not by the colour.
  function sideKind(r: MergeRegion, which: Side): string {
    if (r.kind === "same") return "";
    if (r.kind === "conflict") return "vs";
    if (r.kind === "both") return "add";
    return r.kind === which ? "add" : "del";
  }
  function resultKind(r: MergeRegion, i: number): string {
    if (r.kind === "same" && edited[i] === undefined) return "";
    return origin[i] === "base" ? "keep" : "add";
  }
  const MARK: Record<string, string> = { add: "+", del: "-", vs: "!", keep: "=" };

  /** Which side(s) feed the result here — drawn as arrows in the link columns. */
  function flows(r: MergeRegion, i: number): { left: boolean; right: boolean; open: boolean } {
    const o = origin[i];
    if (o) {
      return {
        left: o === "theirs" || o === "both",
        right: o === "ours" || o === "both",
        open: false,
      };
    }
    if (r.kind === "conflict") return { left: false, right: false, open: true };
    if (r.kind === "same") return { left: false, right: false, open: false };
    if (r.kind === "both") return { left: true, right: true, open: false };
    return { left: r.kind === "theirs", right: r.kind === "ours", open: false };
  }

  // First line number of each region, per pane: the panes are whole files, so
  // each one's numbering runs continuously across regions.
  const starts = $derived.by(() => {
    let t = 1,
      o = 1,
      m = 1;
    return regions.map((r, i) => {
      const at = { t, o, m };
      t += side(r, "theirs").length;
      o += side(r, "ours").length;
      m += linesFor(r, i).length;
      return at;
    });
  });

  /** Copy a side's text into the result, leaving it editable. */
  function take(i: number, what: "theirs" | "ours" | "both" | "base") {
    const r = regions[i];
    if (r.kind !== "conflict") return;
    const text =
      what === "both"
        ? [...r.theirs, ...r.ours].join("\n")
        : what === "base"
          ? r.base.join("\n")
          : r[what].join("\n");
    live = null; // let the region re-render from the new text
    edited = { ...edited, [i]: text };
    origin = { ...origin, [i]: what };
  }

  /** The text of an editable region: one line per row. Line numbers live in a
   *  pseudo element, so nothing but code is ever inside the editable. */
  function readLines(el: HTMLElement): string[] {
    const rows = Array.from(el.children).filter((c): c is HTMLElement => c instanceof HTMLElement);
    if (!rows.length) return el.innerText.replace(/\r/g, "").split("\n");
    // innerText, not textContent: a <br> inside a row (shift+enter) is a real
    // line break and has to come back as one.
    return rows.flatMap((row) => row.innerText.replace(/\r/g, "").split("\n"));
  }
  /** Typing in a region: the DOM is the source of truth until focus leaves. */
  function onType(i: number, el: HTMLElement) {
    edited = { ...edited, [i]: readLines(el).join("\n") };
    origin = { ...origin, [i]: "manual" };
  }
  /** Paste plain text only — a rich paste would inject markup into the code. */
  function pasteAsText(e: ClipboardEvent) {
    const text = e.clipboardData?.getData("text/plain");
    if (text === undefined) return;
    e.preventDefault();
    document.execCommand("insertText", false, text.replace(/\r/g, ""));
  }
  function onEnter(r: MergeRegion, i: number) {
    frozen.lines = linesFor(r, i);
    live = i;
  }
  async function onLeave() {
    live = null;
    if (data) await recolor(data); // colour whatever was just typed
  }
  /** Drop a hand edit and go back to what the merge produced. */
  function revert(i: number) {
    const nextE = { ...edited };
    const nextO = { ...origin };
    delete nextE[i];
    delete nextO[i];
    live = null;
    edited = nextE;
    origin = nextO;
  }

  function goTo(n: number) {
    if (!conflicts.length) return;
    current = ((n % conflicts.length) + conflicts.length) % conflicts.length;
    document
      .querySelector(`[data-region="${conflicts[current]}"]`)
      ?.scrollIntoView({ block: "center" });
  }

  async function save() {
    if (!data || unsettled.length) return;
    saving = true;
    try {
      await invoke<string>("merge_save", { id, text: resultLines.join("\n") + "\n" });
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

  /** Colour lines once per distinct text — identical text tokenizes the same, so
   *  one map serves the side panes AND the assembled result. */
  async function recolor(d: MergeData) {
    const lang = langForFile(d.name);
    if (!lang) return;
    const dark = !window.matchMedia?.("(prefers-color-scheme: light)").matches;
    const batches: string[][] = [[], [], [], []];
    for (const r of d.regions) {
      batches[0].push(...(r.kind === "conflict" ? r.theirs : r.lines));
      batches[1].push(...(r.kind === "conflict" ? r.ours : r.lines));
      if (r.kind !== "same") batches[2].push(...r.base);
    }
    batches[3] = resultLines; // includes anything hand-typed
    const map = new Map(tokens);
    for (const lines of batches) {
      const fresh = lines.filter((l) => !map.has(l));
      if (!fresh.length) continue;
      const runs = await tokenizeLines(lines.join("\n"), lang, dark);
      if (!runs) continue;
      lines.forEach((l, i) => {
        if (runs[i] && !map.has(l)) map.set(l, runs[i]);
      });
    }
    tokens = map;
  }

  onMount(async () => {
    try {
      data = await invoke<MergeData>("merge_data", { id });
      // Nothing is auto-settled: conflicts must be dealt with deliberately. Open
      // on the first one so the window lands on the work to be done.
      if (data.conflicts > 0) setTimeout(() => goTo(0), 0);
      void recolor(data);
    } catch (e) {
      error = String(e);
    }
  });
</script>

<!-- Read-only pane content: mark + line number + coloured code. -->
{#snippet code(lines: string[], from: number, kind: string)}
  {#each lines as line, k}
    <div class="line k-{kind}"><span class="mk">{MARK[kind] ?? ""}</span><span class="ln"
        >{from ? from + k : ""}</span
      ><span class="src"
        >{#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span
              style:color={run.color}>{run.content}</span>{/each}{:else}{line || " "}{/if}</span
      ></div>
  {/each}
{/snippet}

<!-- Just the code of one line, coloured — used inside the editable pane, where
     the gutter must stay OUT of the text the browser edits. -->
{#snippet codeOnly(line: string)}
  {#if tokens.get(line)}{#each tokens.get(line) ?? [] as run}<span style:color={run.color}
        >{run.content}</span
      >{/each}{:else}{line}{/if}
{/snippet}

<div class="wrap">
  <div class="bar">
    {#if data}
      <span class="name mono">{data.name}</span>
      <span class="dim">
        {data.conflicts} conflict{data.conflicts === 1 ? "" : "s"}{data.conflicts
          ? ` · ${unsettled.length} still to settle`
          : ""} · {resultLines.length} lines
      </span>
      <span class="grow"></span>
      <span class="legend dim">
        <span class="chip add">+ kept</span><span class="chip del">− dropped</span><span
          class="chip vs">! conflict</span
        >
        <span class="lgd">▶ ◀ origin</span>
      </span>
      {#if conflicts.length > 1}
        <button onclick={() => goTo(current - 1)} title="Previous conflict">▲</button>
        <span class="dim">{current + 1}/{conflicts.length}</span>
        <button onclick={() => goTo(current + 1)} title="Next conflict">▼</button>
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
  {:else if !data}
    <div class="dim pad">Loading…</div>
  {:else}
    <!-- Headers live in the same grid as the content, so a scrollbar can never
         push a column out of line with its title. The two narrow link columns
         carry the arrows that say which side a region came from. -->
    <div class="scroll">
      <div class="grid mono">
        <div class="head">{data.theirsLabel}</div>
        <div class="head link"></div>
        <div class="head mid">
          result — merged file, type anywhere<span class="dim"> (base: {data.baseLabel})</span>
        </div>
        <div class="head link"></div>
        <div class="head">{data.yoursLabel}</div>

        {#each regions as r, i (i)}
          {@const conflict = r.kind === "conflict"}
          {@const flow = flows(r, i)}
          {@const mine = linesFor(r, i)}
          <!-- A conflict tints its whole row, all five cells, so the band reads
               as one block instead of a few coloured lines. -->
          <div class="cell" class:conflict>
            {#if conflict}<div class="chead side"></div>{/if}
            {@render code(side(r, "theirs"), starts[i].t, sideKind(r, "theirs"))}
          </div>

          <div class="cell link" class:conflict class:on={flow.left}>
            {#if conflict}<div class="chead side"></div>{/if}
            {#if flow.left}
              <div class="arrow" title="This region's text came from the depot side">▶</div>
            {:else if flow.open}
              <div class="arrow open" title="Undecided conflict">?</div>
            {/if}
          </div>

          <div class="cell mid" class:conflict>
            {#if conflict}
              <div class="chead" data-region={i}>
                <span class="cnum">conflict {conflicts.indexOf(i) + 1}</span>
                <button class:on={origin[i] === "theirs"} onclick={() => take(i, "theirs")}>
                  ◀ depot
                </button>
                <button class:on={origin[i] === "ours"} onclick={() => take(i, "ours")}>
                  workspace ▶
                </button>
                <button class:on={origin[i] === "both"} onclick={() => take(i, "both")}>
                  both
                </button>
                <button class:on={origin[i] === "base"} onclick={() => take(i, "base")}>base</button>
                {#if origin[i] === "manual"}
                  <button onclick={() => revert(i)} title="Back to the merged text">revert</button>
                {/if}
                <button
                  class="peek"
                  class:on={showBase[i]}
                  title="Show the common ancestor for this conflict"
                  onclick={() => (showBase = { ...showBase, [i]: !showBase[i] })}
                >
                  {showBase[i] ? "hide base" : "base?"}
                </button>
              </div>
              {#if showBase[i] && r.kind === "conflict"}
                <div class="baseblock">{@render code(r.base, 0, "keep")}</div>
              {/if}
            {:else if origin[i] === "manual"}
              <div class="editbar thin">
                <span class="cnum">hand-edited</span>
                <button onclick={() => revert(i)} title="Back to the merged text">revert</button>
              </div>
            {/if}

            <!-- The result pane is always typeable: no edit mode, no reflow. The
                 gutter sits outside the editable element so line numbers can
                 never end up in the file; while a region is `live` its lines
                 come from `frozen`, which no state change can re-render. -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <div
              class="redit k-{resultKind(r, i)}"
              class:empty={!mine.length}
              role="textbox"
              tabindex="0"
              aria-multiline="true"
              aria-label="merged text"
              contenteditable="true"
              spellcheck="false"
              data-ph={conflict ? "take a side above, or type the resolution" : ""}
              onfocusin={() => onEnter(r, i)}
              onfocusout={onLeave}
              oninput={(e) => onType(i, e.currentTarget)}
              onpaste={pasteAsText}
            >
              {#each live === i ? frozen.lines : mine as line, k}
                <div class="rw" data-mk={MARK[resultKind(r, i)] ?? ""} data-n={starts[i].m + k}
                  >{@render codeOnly(line)}</div
                >
              {/each}
            </div>
          </div>

          <div class="cell link" class:conflict class:on={flow.right}>
            {#if conflict}<div class="chead side"></div>{/if}
            {#if flow.right}
              <div class="arrow" title="This region's text came from the workspace side">◀</div>
            {:else if flow.open}
              <div class="arrow open" title="Undecided conflict">?</div>
            {/if}
          </div>

          <div class="cell" class:conflict>
            {#if conflict}<div class="chead side"></div>{/if}
            {@render code(side(r, "ours"), starts[i].o, sideKind(r, "ours"))}
          </div>
        {/each}
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
  .chip.vs {
    color: #e0555a;
  }
  .lgd {
    margin-left: 4px;
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
    /* pane | link | result | link | pane */
    grid-template-columns: minmax(0, 1fr) 1.4rem minmax(0, 1fr) 1.4rem minmax(0, 1fr);
    align-items: stretch;
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
  .cell {
    border-right: 1px solid var(--border, #333);
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
    min-width: 0;
  }
  .cell.mid {
    background: rgba(255, 255, 255, 0.02);
  }
  /* The conflict band: the whole row, not just its lines. */
  .cell.conflict {
    background: rgba(224, 85, 90, 0.12);
    border-top: 1px solid rgba(224, 85, 90, 0.55);
    border-bottom: 1px solid rgba(224, 85, 90, 0.55);
  }
  .cell.mid.conflict {
    background: rgba(224, 85, 90, 0.09);
  }
  /* Link columns: the visual connection between a side and the result. */
  .cell.link {
    background: var(--bg-alt, #1f1f1f);
    text-align: center;
  }
  .cell.link.on {
    background: rgba(124, 196, 124, 0.12);
  }
  .arrow {
    color: #7cc47c;
    line-height: 1.45;
  }
  .arrow.open {
    color: #e0555a;
    font-weight: 600;
  }
  .line,
  .rw {
    line-height: 1.45;
  }
  .line {
    display: flex;
    align-items: flex-start;
    border-left: 3px solid transparent;
  }
  /* Wrap long lines inside their own pane instead of bleeding into the next. */
  .src {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .src {
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
  /* Colour says add / drop, exactly as in a diff — never which side. */
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
    opacity: 0.8;
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
  /* The editable result. The gutter sits inside each row, so a number stays
     beside its own wrapped line, but is contenteditable="false": out of the text
     and atomic to the caret. */
  .redit {
    border-left: 3px solid transparent;
    outline: none;
    cursor: text;
  }
  /* An inset ring, not a background: the add/drop tint must survive focus. */
  .redit:focus {
    box-shadow: inset 0 0 0 1px rgba(217, 141, 58, 0.55);
  }
  .redit {
    padding-left: 4.4em;
    padding-right: 6px;
  }
  .rw {
    position: relative;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-height: 1.45em;
  }
  /* Number + mark drawn beside the row's first visual line, out of the text. */
  .rw::before {
    content: attr(data-mk) " " attr(data-n);
    position: absolute;
    left: -4.4em;
    width: 4em;
    text-align: right;
    white-space: pre;
    opacity: 0.55;
    color: var(--text-dim, #999);
    user-select: none;
  }
  .k-add .rw::before {
    color: #7cc47c;
  }
  .k-del .rw::before {
    color: #d9873a;
  }
  .k-vs .rw::before {
    color: #e0555a;
  }
  .redit.empty::before {
    content: attr(data-ph);
    color: #e0555a;
    font-style: italic;
  }
  .chead,
  .editbar {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: nowrap;
    min-height: 24px;
    padding: 2px 6px;
    box-sizing: border-box;
  }
  .chead {
    background: rgba(224, 85, 90, 0.16);
  }
  .chead.side {
    background: rgba(224, 85, 90, 0.1);
    padding: 2px 0;
  }
  .editbar {
    background: rgba(255, 255, 255, 0.06);
  }
  .editbar.thin {
    min-height: 0;
    padding: 1px 6px;
  }
  .cnum {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim, #999);
    white-space: nowrap;
  }
  .baseblock {
    opacity: 0.85;
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
  button.on {
    border-color: var(--accent, #d98d3a);
    color: var(--accent, #d98d3a);
  }
  .peek {
    margin-left: auto;
  }
  .primary {
    border-color: var(--accent, #d98d3a);
    color: var(--accent, #d98d3a);
  }
</style>
