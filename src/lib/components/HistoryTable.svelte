<script lang="ts">
  import { fmtTime, firstLine, type P4Record } from "$lib/p4";

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
      <span class="synced-badge" title="Workspace is synced up to this changelist">
        synced @ {haveChange}
      </span>
    {:else if mode === "file" && haveRev}
      <span class="synced-badge" title="You have this revision synced">have #{haveRev}</span>
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
        <thead>
          <tr><th>Change</th><th>Date</th><th>User</th><th>Description</th></tr>
        </thead>
        <tbody>
          {#each shown as r (r.change)}
            <tr
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
        <thead>
          <tr><th>Rev</th><th>Change</th><th>Date</th><th>Action</th><th>User</th><th>Description</th></tr>
        </thead>
        <tbody>
          {#each shown as r (r.rev)}
            <tr
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
</div>

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
    font-size: 11px;
    color: var(--have);
    background: var(--have-bg);
    border: 1px solid var(--have);
    border-radius: 10px;
    padding: 1px 8px;
    white-space: nowrap;
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
  td[title] {
    max-width: 1px;
  }
  .msg {
    padding: 12px;
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
