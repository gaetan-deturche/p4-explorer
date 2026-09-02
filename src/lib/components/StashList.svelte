<script lang="ts">
  import type { StashRow } from "$lib/p4";

  let {
    rows,
    loading,
    client,
    onApply,
    onRename,
    onDelete,
  }: {
    rows: StashRow[];
    loading: boolean;
    /** The workspace in use, so a stash from somewhere else says so. */
    client: string;
    onApply: (row: StashRow) => void;
    onRename: (row: StashRow) => void;
    onDelete: (row: StashRow) => void;
  } = $props();

  let openIds = $state(new Set<number>());
  function toggle(id: number) {
    const next = new Set(openIds);
    if (!next.delete(id)) next.add(id);
    openIds = next;
  }

  function when(seconds: number): string {
    const d = new Date(seconds * 1000);
    const day = d.toLocaleDateString(undefined, { day: "2-digit", month: "2-digit", year: "numeric" });
    const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
    return `${day} ${time}`;
  }
  function size(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }
  const name = (depot: string) => depot.split("/").pop() || depot;
  const dir = (depot: string) => depot.slice(0, depot.lastIndexOf("/") + 1);
</script>

<div class="panel">
  <div class="head">
    <span class="count">{rows.length} stash{rows.length === 1 ? "" : "es"}</span>
    <!-- Saying where they live is the whole point of the feature: one database
         for the machine, so a stash taken in one workspace applies in another. -->
    <span class="dim">on this machine — a stash can be applied in any workspace</span>
  </div>

  <div class="scroll">
    {#if loading && !rows.length}
      <div class="empty dim">Loading…</div>
    {:else if !rows.length}
      <div class="empty dim">
        No stashes yet. Right-click a changelist or a file selection in <b>Pending</b> and choose
        <b>Stash…</b> — it copies the change into the app's database and leaves your files exactly as
        they are.
      </div>
    {:else}
      {#each rows as s (s.id)}
        {@const mine = s.client === client}
        <div class="row" class:open={openIds.has(s.id)}>
          <button class="disc" onclick={() => toggle(s.id)} title="Show the files">
            {openIds.has(s.id) ? "▾" : "▸"}
          </button>
          <span class="name">{s.name}</span>
          <span class="meta dim">
            {s.files.length} file{s.files.length === 1 ? "" : "s"} · {size(s.bytes)} · {when(
              s.created,
            )}
          </span>
          <span class="from" class:elsewhere={!mine} title={mine ? "" : `Taken in ${s.client}`}>
            {mine ? "this workspace" : s.client}
          </span>
          <span class="acts">
            <button class="primary" onclick={() => onApply(s)} title="Preview it against this workspace, then apply">
              Apply…
            </button>
            <button onclick={() => onRename(s)}>Rename</button>
            <button class="danger-btn" onclick={() => onDelete(s)}>Delete</button>
          </span>
        </div>
        {#if openIds.has(s.id)}
          <div class="files">
            {#each s.files as f (f.depotFile)}
              <div class="f">
                <span class="act {f.action}">{f.action}</span>
                <span class="fname mono">{name(f.depotFile)}</span>
                <span class="fdir mono dim">{dir(f.depotFile)}</span>
                <!-- The revision it was a change AGAINST: an apply on a workspace
                     at a different revision is where a hunk has to be placed by
                     context rather than by position. -->
                <span class="rev dim">{f.rev ? `#${f.rev}` : "new file"}</span>
                {#if f.binary}<span class="tag">binary</span>{/if}
              </div>
            {/each}
            {#each s.skipped as d (d)}
              <div class="f">
                <span class="act delete">delete</span>
                <span class="fname mono">{name(d)}</span>
                <span class="fdir mono dim">{dir(d)}</span>
                <!-- Recorded, not carried: a unified diff has no way to say
                     "remove this file", so applying the stash will not. -->
                <span class="rev warn">not carried by a patch</span>
              </div>
            {/each}
          </div>
        {/if}
      {/each}
    {/if}
  </div>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border, #333);
    font-size: 12px;
    flex: none;
  }
  .count {
    font-weight: 600;
  }
  .dim {
    color: var(--text-dim, #999);
  }
  .scroll {
    flex: 1;
    overflow: auto;
    min-width: 0;
  }
  .empty {
    padding: 18px 14px;
    font-size: 12px;
    line-height: 1.6;
    max-width: 60ch;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px 4px 2px;
    font-size: 12px;
    border-bottom: 1px solid var(--border, #333);
    min-width: 0;
  }
  .row.open {
    background: var(--bg-alt, #202020);
  }
  .disc {
    border: none;
    background: none;
    color: var(--text-dim, #999);
    width: 18px;
    cursor: pointer;
  }
  .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 28ch;
  }
  .meta {
    white-space: nowrap;
  }
  .from {
    font-size: 11px;
    color: var(--text-dim, #999);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1 1 auto;
    min-width: 0;
  }
  /* A stash from another workspace is the interesting case, so it is the one
     that gets marked rather than the ordinary one. */
  .from.elsewhere {
    color: var(--accent, #6ea8fe);
  }
  .acts {
    display: flex;
    gap: 4px;
    flex: none;
  }
  .acts button {
    font-size: 11px;
    padding: 1px 8px;
  }
  .primary {
    border-color: var(--accent, #6ea8fe);
    color: var(--accent, #6ea8fe);
  }
  .danger-btn {
    border-color: var(--warn, #e0a33a);
    color: var(--warn, #e0a33a);
  }
  .files {
    padding: 2px 0 6px 28px;
    border-bottom: 1px solid var(--border, #333);
    background: var(--bg-alt, #202020);
  }
  .f {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 1px 10px;
    font-size: 11px;
    min-width: 0;
  }
  .act {
    width: 6ch;
    flex: none;
    color: var(--text-dim, #999);
  }
  .act.add {
    color: #7cc47c;
  }
  .act.delete {
    color: #d9873a;
  }
  .fname {
    flex: none;
  }
  .fdir {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rev {
    flex: none;
  }
  .warn {
    color: var(--warn, #e0a33a);
  }
  .tag {
    flex: none;
    border: 1px solid var(--border, #333);
    border-radius: 3px;
    padding: 0 4px;
    color: var(--text-dim, #999);
  }
  .mono {
    font-family: var(--mono, ui-monospace, Consolas, monospace);
  }
</style>
