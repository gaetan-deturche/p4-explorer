<script lang="ts">
  import { fmtTime, firstLine, type P4Record } from "$lib/p4";
  import DiffView from "$lib/components/DiffView.svelte";

  let {
    rows,
    loading,
    client,
    onLocalFiles,
    onShelvedFiles,
    onLocalDiff,
    onShelvedDiff,
    onOpenLocalDiff,
    onOpenShelvedDiff,
    onContext,
  }: {
    rows: P4Record[];
    loading: boolean;
    client: string; // resets the per-CL cache when the workspace changes
    onLocalFiles: (change: string) => Promise<P4Record[]>; // opened (workspace) files
    onShelvedFiles: (change: string) => Promise<P4Record[]>; // shelved files
    onLocalDiff: (depotFile: string) => Promise<string>; // local vs server
    onShelvedDiff: (depotFile: string, rev: number, change: string) => Promise<string>;
    onOpenLocalDiff: (depotFile: string) => void;
    onOpenShelvedDiff: (depotFile: string, rev: number, change: string) => void;
    onContext: (cl: P4Record, e: MouseEvent) => void; // right-click a changelist
  } = $props();

  type CL = {
    open: boolean;
    loading: boolean;
    local: P4Record[];
    shelved: P4Record[];
    shelvedOpen: boolean;
  };
  let cls = $state<Record<string, CL>>({});

  // Per-file inline diff, keyed by "<change>|<kind>|<depotFile>".
  let fdiff = $state<Record<string, { open: boolean; loading: boolean; text: string }>>({});

  async function loadCL(change: string) {
    cls[change] = { open: true, loading: true, local: [], shelved: [], shelvedOpen: false };
    const [local, shelved] = await Promise.all([onLocalFiles(change), onShelvedFiles(change)]);
    const local2 = local.filter((f) => f.depotFile);
    const shelved2 = shelved.filter((f) => f.depotFile);
    cls[change] = {
      open: cls[change]?.open ?? true,
      loading: false,
      local: local2,
      shelved: shelved2,
      shelvedOpen: cls[change]?.shelvedOpen ?? false,
    };
  }

  function toggleCL(change: string) {
    const cur = cls[change];
    if (!cur) {
      loadCL(change);
      return;
    }
    cls[change] = { ...cur, open: !cur.open };
  }
  function toggleShelved(change: string) {
    const cur = cls[change];
    if (cur) cls[change] = { ...cur, shelvedOpen: !cur.shelvedOpen };
  }

  // Expand every changelist by default. Reset the cache when the workspace
  // changes — CL keys like "default" are reused across workspaces.
  let lastClient = "";
  $effect(() => {
    if (client !== lastClient) {
      lastClient = client;
      cls = {};
      fdiff = {};
    }
    for (const r of rows) {
      if (!cls[r.change]) loadCL(r.change);
    }
  });

  async function toggleFileDiff(change: string, kind: "local" | "shelved", f: P4Record) {
    const key = `${change}|${kind}|${f.depotFile}`;
    const cur = fdiff[key];
    if (cur?.open) {
      fdiff[key] = { ...cur, open: false };
      return;
    }
    if (cur && !cur.loading) {
      fdiff[key] = { ...cur, open: true };
      return;
    }
    fdiff[key] = { open: true, loading: true, text: "" };
    const text =
      kind === "local"
        ? await onLocalDiff(f.depotFile)
        : await onShelvedDiff(f.depotFile, Number(f.rev), change);
    fdiff[key] = { open: true, loading: false, text };
  }
  function openExt(change: string, kind: "local" | "shelved", f: P4Record) {
    if (kind === "local") onOpenLocalDiff(f.depotFile);
    else onOpenShelvedDiff(f.depotFile, Number(f.rev), change);
  }

  function splitPath(p: string): { dir: string; name: string } {
    const i = p.lastIndexOf("/");
    return i >= 0 ? { dir: p.slice(0, i + 1), name: p.slice(i + 1) } : { dir: "", name: p };
  }
</script>

<div class="panel">
  <div class="scroll body">
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if rows.length === 0}
      <div class="msg dim">No pending changelists.</div>
    {:else}
      {#each rows as r (r.change)}
        {@const s = cls[r.change]}
        <button class="cl" onclick={() => toggleCL(r.change)} oncontextmenu={(e) => onContext(r, e)}>
          <span class="tw">{s?.open ? "▾" : "▸"}</span>
          <span class="cnum mono">{r.change === "default" ? "Default" : "@" + r.change}</span>
          <span class="desc" title={r.desc}>
            {r.change === "default" ? "" : firstLine(r.desc) || "(no description)"}
          </span>
          <span class="user dim">{r.user}</span>
          <span class="date dim">{fmtTime(r.time)}</span>
        </button>
        {#if s?.open}
          {#if s.loading}
            <div class="finfo dim">Loading files…</div>
          {:else if s.local.length === 0 && s.shelved.length === 0}
            <div class="finfo dim">No files.</div>
          {:else}
            {#if s.shelved.length}
              <button class="subfolder" onclick={() => toggleShelved(r.change)}>
                <span class="tw">{s.shelvedOpen ? "▾" : "▸"}</span>
                <span class="ic">📁</span> Shelved <span class="dim">({s.shelved.length})</span>
              </button>
              {#if s.shelvedOpen}
                {#each s.shelved as f (f.depotFile)}
                  {@render fileRow(f, r.change, "shelved", 2)}
                {/each}
              {/if}
            {/if}
            {#each s.local as f (f.depotFile)}
              {@render fileRow(f, r.change, "local", 1)}
            {/each}
          {/if}
        {/if}
      {/each}
    {/if}
  </div>
</div>

{#snippet fileRow(f: P4Record, change: string, kind: "local" | "shelved", depth: number)}
  {@const key = `${change}|${kind}|${f.depotFile}`}
  {@const fd = fdiff[key]}
  {@const sp = splitPath(f.depotFile)}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="frow mono"
    style="padding-left:{depth * 16 + 4}px"
    title={"Double-click to open in external diff\n" + f.depotFile}
    ondblclick={() => openExt(change, kind, f)}
  >
    <button
      class="fchev"
      title="Show diff"
      onclick={(e) => {
        e.stopPropagation();
        toggleFileDiff(change, kind, f);
      }}
      ondblclick={(e) => e.stopPropagation()}
    >
      {fd?.open ? "▾" : "▸"}
    </button>
    <span class="act act-{f.action}">{f.action ?? ""}</span>
    <span class="fpath"><span class="pdir dim">{sp.dir}</span>{sp.name}</span>
    <span class="ftype dim">{f.type ?? ""}</span>
  </div>
  {#if fd?.open}
    {#if fd.loading}
      <div class="finfo dim" style="padding-left:{depth * 16 + 20}px">Loading diff…</div>
    {:else if !fd.text.trim()}
      <div class="finfo dim" style="padding-left:{depth * 16 + 20}px">
        No textual diff (added, binary, or identical).
      </div>
    {:else}
      <DiffView text={fd.text} />
    {/if}
  {/if}
{/snippet}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
  }
  .body {
    flex: 1;
    padding: 2px 0;
  }
  .cl {
    display: flex;
    align-items: baseline;
    gap: 6px;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 0;
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
    cursor: pointer;
    border-bottom: 1px solid var(--border);
  }
  .cl:hover {
    background: var(--bg-hover);
  }
  .tw {
    flex: none;
    width: 12px;
    color: var(--text-dim);
    font-size: 10px;
  }
  .cnum {
    flex: none;
    font-weight: 600;
  }
  .desc {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .user,
  .date {
    flex: none;
    font-size: 11px;
  }
  .subfolder {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 0;
    padding: 3px 10px 3px 20px;
    font-size: 12px;
    color: var(--text);
    cursor: pointer;
  }
  .subfolder:hover {
    background: var(--bg-hover);
  }
  .finfo {
    padding: 3px 10px;
    font-size: 11px;
    font-style: italic;
  }
  .frow {
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 2px 10px 2px 4px;
    font-size: 12px;
    white-space: nowrap;
    cursor: default;
  }
  .frow:hover {
    background: var(--bg-hover);
  }
  .fchev {
    flex: none;
    border: none;
    background: none;
    border-radius: 0;
    padding: 0 2px;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 10px;
    line-height: 1;
  }
  .fchev:hover {
    color: var(--text);
  }
  .act {
    flex: none;
    text-transform: capitalize;
    width: 4rem;
  }
  .act-add {
    color: var(--have);
  }
  .act-delete {
    color: var(--warn);
  }
  .act-edit {
    color: var(--accent);
  }
  .fpath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ftype {
    flex: none;
    font-size: 11px;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
</style>
