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
  } = $props();

  // Changelist number the workspace is synced to (the "you are here" anchor).
  const anchorNum = $derived.by(() => {
    if (mode === "folder") return haveChange ? Number(haveChange) : NaN;
    if (!haveRev) return NaN;
    const r = rows.find((x) => x.rev === haveRev);
    return r ? Number(r.change) : NaN;
  });

  // A changelist newer than the synced one — i.e. not yet pulled into the workspace.
  function isAhead(r: P4Record): boolean {
    return Number.isFinite(anchorNum) && Number(r.change) > anchorNum;
  }

  // Fuzzy search over changelist id, user, and description. Every whitespace-
  // separated term must subsequence-match at least one field (order preserved —
  // history stays chronological, rows just drop out).
  let query = $state("");
  function fuzzy(term: string, text: string): boolean {
    let i = 0;
    for (const ch of text) {
      if (ch === term[i] && ++i === term.length) return true;
    }
    return term.length === 0;
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
      return terms.every((t) => fields.some((f) => fuzzy(t, f)));
    });
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

  <div class="scroll body">
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if rows.length === 0}
      <div class="msg dim">No history. Pick a file or a folder on the left.</div>
    {:else if shown.length === 0}
      <div class="msg dim">No changelists matching “{query.trim()}”.</div>
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
</style>
