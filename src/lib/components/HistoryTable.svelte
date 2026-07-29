<script lang="ts">
  import { tick } from "svelte";
  import { fmtTime, firstLine, type P4Record } from "$lib/p4";
  import { loadHistCols, saveHistCols } from "$lib/nav";

  let {
    mode,
    subject,
    rows,
    loading,
    more = false,
    haveChange,
    haveRev,
    selectedChange,
    onSelectChange,
    onContextMenu,
    onDeepen,
    deepening = false,
  }: {
    mode: "folder" | "file";
    subject: string;
    rows: P4Record[];
    loading: boolean;
    more?: boolean;
    haveChange: string;
    haveRev: string;
    selectedChange: string;
    onSelectChange: (change: string) => void;
    onContextMenu?: (change: string, e: MouseEvent) => void;
    onDeepen?: () => void; // fetch older history (search covers loaded rows only)
    deepening?: boolean;
  } = $props();

  // Changelist number the workspace is synced to (the "you are here" anchor).
  // An empty have on a LOADED history means nothing under the subject is synced
  // (fresh workspace) — anchor 0 so every row reads as not-yet-pulled, instead
  // of the all-white "everything synced" look NaN produced.
  const anchorNum = $derived.by(() => {
    if (rows.length === 0) return NaN;
    if (mode === "folder") return haveChange ? Number(haveChange) : 0;
    if (!haveRev) return 0;
    const r = rows.find((x) => x.rev === haveRev);
    return r ? Number(r.change) : NaN;
  });

  // A changelist newer than the synced one — i.e. not yet pulled into the workspace.
  function isAhead(r: P4Record): boolean {
    return Number.isFinite(anchorNum) && Number(r.change) > anchorNum;
  }

  // --- column widths -------------------------------------------------------
  // The fixed-content columns (rev/change/date/action/user) are auto-fitted to
  // their widest actual value — measured, not guessed, because the date is
  // toLocaleString() and its width depends on the locale — and each is
  // resizable, with manual widths persisted. Description takes the rest.
  type ColKey = "rev" | "change" | "date" | "action" | "user";
  let manualW = $state<Partial<Record<ColKey, number>>>(loadHistCols());
  let probe: HTMLSpanElement | undefined = $state(); // supplies the real fonts
  let monoProbe: HTMLSpanElement | undefined = $state();
  let measureCtx: CanvasRenderingContext2D | null = null;

  /** Width of `text` in the table's font (mono variant for the code-ish cells). */
  function measure(text: string, mono: boolean): number {
    const el = mono ? monoProbe : probe;
    if (!el) return text.length * 7; // pre-mount fallback
    measureCtx ??= document.createElement("canvas").getContext("2d");
    if (!measureCtx) return text.length * 7;
    measureCtx.font = getComputedStyle(el).font;
    return measureCtx.measureText(text).width;
  }

  /** Fit a column to its widest value across the rows (clamped). */
  function fitTo(pick: (r: P4Record) => string, mono: boolean, min: number, max = 260): number {
    let widest = "";
    for (const r of rows) {
      const v = pick(r);
      if (v.length > widest.length) widest = v;
    }
    return Math.min(max, Math.max(min, Math.ceil(measure(widest, mono)) + 20)); // + cell padding
  }
  // Auto-fit widths. The probes are read through measure(), so this recomputes
  // once they mount; the minimums keep the header labels readable.
  const autoW = $derived.by<Record<ColKey, number>>(() => {
    void probe;
    void monoProbe;
    return {
      rev: fitTo((r) => "#" + (r.rev ?? ""), true, 54, 110),
      change: fitTo((r) => "▸@" + (r.change ?? ""), true, 66, 150),
      date: fitTo((r) => fmtTime(r.time), false, 60, 200),
      action: fitTo((r) => r.action ?? "", false, 56, 140),
      user: fitTo((r) => r.user ?? "", false, 60, 240),
    };
  });
  const w = $derived({ ...autoW, ...manualW });

  function startResize(e: PointerEvent, key: ColKey) {
    e.preventDefault();
    e.stopPropagation();
    const startX = e.clientX;
    const startW = w[key];
    const move = (ev: PointerEvent) => {
      manualW = { ...manualW, [key]: Math.max(40, startW + (ev.clientX - startX)) };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      saveHistCols(manualW); // remember the user's sizing
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  /** Double-click a divider: drop the manual width and auto-fit that column again. */
  function autoFit(key: ColKey) {
    const rest = { ...manualW };
    delete rest[key];
    manualW = rest;
    saveHistCols(manualW);
  }

  // Search over changelist id, user, and description. Every whitespace-
  // separated term must match at least one field (order preserved — history
  // stays chronological, rows just drop out). A term matches as a SUBSTRING,
  // or — for short simple terms — as a subsequence of a single WORD (typo
  // tolerance: "jlclp" finds "Jellyclip"). Whole-text subsequence matching is
  // deliberately avoided: a long term scatter-matches almost any description.
  let query = $state("");
  function fuzzyWord(term: string, word: string): boolean {
    let i = 0;
    for (const ch of word) {
      if (ch === term[i] && ++i === term.length) return true;
    }
    return term.length === 0;
  }
  function termMatches(term: string, field: string): boolean {
    if (field.includes(term)) return true;
    if (term.length <= 8 && /^[a-z0-9]+$/.test(term)) {
      for (const w of field.split(/[^a-z0-9]+/)) {
        if (w.length >= term.length && fuzzyWord(term, w)) return true;
      }
    }
    return false;
  }
  const shown = $derived.by(() => {
    const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!terms.length) return rows;
    return rows.filter((r) => {
      const fields = [
        "@" + (r.change ?? ""),
        r.rev ? "#" + r.rev : "",
        r.user ?? "",
        r.desc ?? "",
      ].map((f) => f.toLowerCase());
      return terms.every((t) => fields.some((f) => termMatches(t, f)));
    });
  });

  /** Scroll the synced changelist / revision into view (the header badge). It
   *  clears an active search first if that row is filtered out, and loads older
   *  history when the synced point is beyond what's fetched — the have CL can be
   *  well behind head on a stale workspace. */
  async function jumpToHave() {
    const key = mode === "folder" ? haveChange : haveRev;
    if (!key) return;
    const sel = mode === "folder" ? `[data-change="${key}"]` : `[data-rev="${key}"]`;
    const present = () =>
      mode === "folder" ? rows.some((r) => r.change === key) : rows.some((r) => r.rev === key);
    // A filter would hide it: clear the query rather than silently doing nothing.
    if (query.trim() && present()) query = "";
    if (!present() && onDeepen && !exhausted && !deepening) {
      lenBeforeDeepen = rows.length;
      onDeepen(); // fetch older history, then land on it below
    }
    // Wait for the row (a deepen streams in progressively), then centre it.
    for (let i = 0; i < 40; i++) {
      await tick();
      const el = body?.querySelector(sel);
      if (el) {
        el.scrollIntoView({ block: "center" });
        return;
      }
      if (!deepening && i > 2) break; // nothing loading and still absent
      await new Promise((r) => setTimeout(r, 150));
    }
  }

  // Infinite scroll: nearing the bottom loads older history automatically.
  // `exhausted` latches when a deepen adds no rows (true end of history) so we
  // stop asking; it resets when the subject changes.
  let exhausted = $state(false);
  let lenBeforeDeepen = 0;
  let wasDeepening = false;
  $effect(() => {
    void subject;
    void mode;
    exhausted = false;
  });
  $effect(() => {
    if (wasDeepening && !deepening && rows.length === lenBeforeDeepen) exhausted = true;
    wasDeepening = deepening;
  });
  function onScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    if (!onDeepen || deepening || exhausted || loading || rows.length === 0) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 300) {
      lenBeforeDeepen = rows.length;
      onDeepen();
    }
  }

  // While a search is active, keep loading older history automatically as long
  // as the results don't fill the view (no scrollbar → the scroll trigger can
  // never fire). Stops when the view fills (scrolling takes over), the query
  // clears, or history is exhausted.
  let body = $state<HTMLDivElement>();
  $effect(() => {
    void shown.length; // re-check as results stream in
    if (!body || !onDeepen || deepening || exhausted || loading) return;
    if (!query.trim() || rows.length === 0) return;
    if (body.scrollHeight <= body.clientHeight + 50) {
      lenBeforeDeepen = rows.length;
      onDeepen();
    }
  });
</script>

<div class="panel">
  <div class="head">
    <span class="title mono" title={subject}>{subject || "History"}</span>
    <input
      class="search"
      placeholder="Search @cl / user / message"
      bind:value={query}
      spellcheck="false"
    />
    {#if mode === "folder" && haveChange}
      <button
        class="synced-badge"
        title="Workspace is synced up to this changelist — click to scroll to it"
        onclick={jumpToHave}
      >
        synced @ {haveChange}
      </button>
    {:else if mode === "file" && haveRev}
      <button
        class="synced-badge"
        title="You have this revision synced — click to scroll to it"
        onclick={jumpToHave}
      >
        have #{haveRev}
      </button>
    {/if}
  </div>

  <div class="scroll body" bind:this={body} onscroll={onScroll}>
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if rows.length === 0}
      <div class="msg dim">No history. Pick a file or a folder on the left.</div>
    {:else if shown.length === 0}
      {#if exhausted || !onDeepen}
        <div class="msg dim">
          No changelists matching “{query.trim()}” in the full history ({rows.length} changelists).
        </div>
      {:else}
        <!-- Auto-deepening (see the fill effect) — keep the user informed. -->
        <div class="msg dim">
          No matches in the {rows.length} newest changelists — searching older history…
        </div>
      {/if}
    {:else if mode === "folder"}
      <table class="grid">
        <colgroup>
          <col style="width:{w.change}px" />
          <col style="width:{w.date}px" />
          <col style="width:{w.user}px" />
          <col />
        </colgroup>
        <thead>
          <tr>
            <th>Change{@render grip("change")}</th>
            <th>Date{@render grip("date")}</th>
            <th>User{@render grip("user")}</th>
            <th>Description</th>
          </tr>
        </thead>
        <tbody>
          {#each shown as r (r.change)}
            <tr
              data-change={r.change}
              class:have={r.change === haveChange}
              class:ahead={isAhead(r)}
              class:selected={r.change === selectedChange}
              onclick={() => onSelectChange(r.change)}
              oncontextmenu={(e) => onContextMenu?.(r.change, e)}
            >
              <td class="mono">
                {#if r.change === haveChange}<span class="you">▸</span>{/if}@{r.change}
              </td>
              <td class="dim">{fmtTime(r.time)}</td>
              <td>{r.user}</td>
              <td title={r.desc}>{firstLine(r.desc)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <table class="grid">
        <colgroup>
          <col style="width:{w.rev}px" />
          <col style="width:{w.change}px" />
          <col style="width:{w.date}px" />
          <col style="width:{w.action}px" />
          <col style="width:{w.user}px" />
          <col />
        </colgroup>
        <thead>
          <tr>
            <th>Rev{@render grip("rev")}</th>
            <th>Change{@render grip("change")}</th>
            <th>Date{@render grip("date")}</th>
            <th>Action{@render grip("action")}</th>
            <th>User{@render grip("user")}</th>
            <th>Description</th>
          </tr>
        </thead>
        <tbody>
          {#each shown as r (r.rev)}
            <tr
              data-rev={r.rev}
              class:have={r.rev === haveRev}
              class:ahead={isAhead(r)}
              class:selected={r.change === selectedChange}
              onclick={() => onSelectChange(r.change)}
              oncontextmenu={(e) => onContextMenu?.(r.change, e)}
            >
              <td class="mono">
                {#if r.rev === haveRev}<span class="you">▸</span>{/if}#{r.rev}
              </td>
              <td class="mono">@{r.change}</td>
              <td class="dim">{fmtTime(r.time)}</td>
              <td>{r.action}</td>
              <td>{r.user}</td>
              <td title={r.desc}>{firstLine(r.desc)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    {#if more}
      <div class="more dim">loading more…</div>
    {/if}
    {#if deepening && !query.trim()}
      <div class="more dim">loading older history…</div>
    {/if}
    <!-- While a search is active the coverage matters: say how far back the
         loaded rows go and offer to page deeper. -->
    {#if query.trim() && rows.length > 0 && shown.length > 0 && onDeepen}
      <div class="deepen">
        <span class="dim">searching the {rows.length} loaded changelists</span>
        <button disabled={deepening} onclick={onDeepen}>
          {deepening ? "Loading older history…" : "Load older history"}
        </button>
      </div>
    {/if}
  </div>
  <!-- Off-screen probes: supply the exact fonts the cells use, so column widths
       are measured rather than guessed (see measure()). -->
  <span class="probe" bind:this={probe}>0</span>
  <span class="probe mono" bind:this={monoProbe}>0</span>
</div>

<!-- Column divider: drag to resize, double-click to auto-fit again. -->
{#snippet grip(key: ColKey)}
  <span
    class="rz"
    role="separator"
    aria-orientation="vertical"
    title="Drag to resize · double-click to fit contents"
    onpointerdown={(e) => startResize(e, key)}
    ondblclick={() => autoFit(key)}
  ></span>
{/snippet}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    font-size: 12px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .search {
    flex: none;
    width: 15rem;
    font-size: 12px;
    padding: 2px 8px;
  }
  .synced-badge {
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    color: var(--have);
    background: var(--have-bg);
    border: 1px solid var(--have);
    border-radius: 10px;
    padding: 1px 8px;
    white-space: nowrap;
  }
  .synced-badge:hover {
    background: var(--have);
    color: var(--bg-panel);
  }
  .body {
    flex: 1;
  }
  .you {
    color: var(--have);
    margin-right: 2px;
  }
  /* Synced changelist: the "you are here" row, emphasised. */
  tbody tr.have td {
    font-weight: 700;
  }
  /* Changelists newer than what's synced (not yet pulled): de-emphasised. */
  tbody tr.ahead td {
    color: var(--text-dim);
  }
  tr {
    cursor: pointer;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
  /* Fixed-width columns must not be stretched by long descriptions. */
  table.grid {
    table-layout: fixed;
    width: 100%;
  }
  th {
    position: relative; /* anchors the resize grip */
  }
  .rz {
    position: absolute;
    top: 0;
    right: -3px;
    width: 7px;
    height: 100%;
    cursor: col-resize;
    -webkit-user-select: none;
    user-select: none;
  }
  .rz:hover {
    background: var(--accent);
    opacity: 0.5;
  }
  /* Font probes for measuring — present in layout but invisible. */
  .probe {
    position: absolute;
    visibility: hidden;
    pointer-events: none;
    font-size: 12px;
  }
  .more {
    padding: 6px 10px;
    font-size: 11px;
    font-style: italic;
  }
  .deepen {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    font-size: 11px;
  }
  .deepen button {
    font-size: 11px;
    padding: 2px 10px;
  }
</style>
