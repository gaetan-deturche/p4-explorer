<script lang="ts">
  import { fmtTime, type P4Record } from "$lib/p4";
  import DiffView from "$lib/components/DiffView.svelte";

  let {
    change,
    rows,
    loading,
    onDiff,
    onOpenDiff,
  }: {
    change: string;
    rows: P4Record[];
    loading: boolean;
    onDiff: (depotFile: string, rev: number) => Promise<string>;
    onOpenDiff: (depotFile: string, rev: number) => void;
  } = $props();

  // Inline diff state per file. Reset whenever the changelist changes.
  let diffState = $state<Record<string, { open: boolean; loading: boolean; text: string }>>({});
  $effect(() => {
    change; // dependency
    diffState = {};
  });

  async function toggleDiff(f: P4Record) {
    const key = f.depotFile;
    const cur = diffState[key];
    if (cur?.open) {
      diffState[key] = { ...cur, open: false };
      return;
    }
    if (cur && !cur.loading) {
      diffState[key] = { ...cur, open: true };
      return;
    }
    diffState[key] = { open: true, loading: true, text: "" };
    const text = await onDiff(f.depotFile, Number(f.rev));
    diffState[key] = { open: true, loading: false, text };
  }

  const header = $derived(rows[0]);
  const desc = $derived(header?.desc ?? "");
  const fileRows = $derived(rows.filter((r) => r.depotFile));

  function splitPath(p: string): { dir: string; name: string } {
    const i = p.lastIndexOf("/");
    return i >= 0 ? { dir: p.slice(0, i + 1), name: p.slice(i + 1) } : { dir: "", name: p };
  }

  // Resizable Action/Type columns; File flexes to fill (so no horizontal scroll).
  let colW = $state({ action: 66, type: 84 });
  function startResize(e: PointerEvent, key: "action" | "type", sign: number) {
    e.preventDefault();
    e.stopPropagation();
    const startX = e.clientX;
    const startW = colW[key];
    const move = (ev: PointerEvent) => {
      colW[key] = Math.max(40, startW + sign * (ev.clientX - startX));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // Custom hover popup showing the full path (fixed-positioned, escapes clipping).
  let tip = $state<{ x: number; y: number; text: string } | null>(null);
  function showTip(e: PointerEvent, text: string) {
    tip = { x: e.clientX, y: e.clientY, text };
  }
  function moveTip(e: PointerEvent) {
    if (tip) tip = { ...tip, x: e.clientX, y: e.clientY };
  }
  function hideTip() {
    tip = null;
  }
</script>

<div class="panel">
  <div class="head">
    <span class="title mono">{change ? "Changelist @" + change : "Changelist"}</span>
    {#if header}<span class="dim">{header.status ?? ""}</span>{/if}
  </div>

  <div class="scroll body">
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if !change}
      <div class="msg dim">Select a changelist to see its details.</div>
    {:else if !header}
      <div class="msg dim">No details.</div>
    {:else}
      <div class="meta">
        <div><span class="k">User</span> {header.user ?? ""}</div>
        <div><span class="k">Date</span> {fmtTime(header.time)}</div>
        <div><span class="k">Client</span> <span class="mono">{header.client ?? ""}</span></div>
      </div>
      {#if desc}<pre class="desc">{desc}</pre>{/if}
      <div class="files-title dim">Files ({fileRows.length})</div>
      <table class="grid files">
        <colgroup>
          <col style="width:{colW.action}px" />
          <col />
          <col style="width:{colW.type}px" />
        </colgroup>
        <thead>
          <tr>
            <th>
              <span class="th-label">Action</span>
              <span class="rz rz-right" role="separator" aria-orientation="vertical"
                onpointerdown={(e) => startResize(e, "action", 1)}></span>
            </th>
            <th><span class="th-label">File</span></th>
            <th>
              <span class="rz rz-left" role="separator" aria-orientation="vertical"
                onpointerdown={(e) => startResize(e, "type", -1)}></span>
              <span class="th-label">Type</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {#each fileRows as f (f.depotFile)}
            {@const sp = splitPath(f.depotFile)}
            {@const d = diffState[f.depotFile]}
            <tr>
              <td class="act act-{f.action}">{f.action ?? ""}</td>
              <td
                class="filecell mono"
                title="Double-click to open in external diff"
                ondblclick={() => onOpenDiff(f.depotFile, Number(f.rev))}
                onpointerenter={(e) => showTip(e, f.depotFile)}
                onpointermove={moveTip}
                onpointerleave={hideTip}
              >
                <button
                  class="dchev"
                  title="Show diff"
                  onclick={(e) => {
                    e.stopPropagation();
                    toggleDiff(f);
                  }}
                  ondblclick={(e) => e.stopPropagation()}
                >
                  {d?.open ? "▾" : "▸"}
                </button>
                <span class="path">
                  <span class="pdir">{sp.dir}</span><span class="pfile">{sp.name}</span>
                </span>
              </td>
              <td class="dim">{f.type ?? ""}</td>
            </tr>
            {#if d?.open}
              <tr class="diffrow">
                <td colspan="3">
                  {#if d.loading}
                    <div class="diffmsg dim">Loading diff…</div>
                  {:else if !d.text.trim()}
                    <div class="diffmsg dim">No textual diff (added, binary, or identical).</div>
                  {:else}
                    <DiffView text={d.text} />
                  {/if}
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    {/if}
  </div>

  {#if tip}
    <div class="tip mono" style="left:{tip.x + 14}px; top:{tip.y + 16}px">{tip.text}</div>
  {/if}
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
    flex: 1;
  }
  .body {
    flex: 1;
    padding: 0 0 10px;
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    font-size: 12px;
  }
  .k {
    display: inline-block;
    width: 3.5rem;
    color: var(--text-dim);
  }
  .desc {
    margin: 0 10px 8px;
    padding: 8px;
    background: var(--bg-alt);
    border-radius: 5px;
    font-family: var(--mono);
    font-size: 11px;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 30vh;
    overflow: auto;
  }
  .files-title {
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 600;
  }
  .files {
    table-layout: fixed;
    width: 100%;
  }
  .files th {
    position: relative;
    overflow: hidden;
  }
  .th-label {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rz {
    position: absolute;
    top: 0;
    width: 7px;
    height: 100%;
    cursor: col-resize;
    touch-action: none;
  }
  .rz-right {
    right: 0;
  }
  .rz-left {
    left: 0;
  }
  .rz:hover {
    background: var(--accent);
    opacity: 0.5;
  }
  .filecell {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .dchev {
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
  .dchev:hover {
    color: var(--text);
  }
  /* File cell: truncate the directory, always keep the filename+rev visible. */
  .path {
    display: flex;
    min-width: 0;
    overflow: hidden;
    flex: 1;
  }
  .diffrow td {
    padding: 0;
    white-space: normal;
  }
  .diffmsg {
    padding: 8px 12px;
    font-size: 11px;
    background: var(--bg-alt);
    border-top: 1px solid var(--border);
  }
  .pdir {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
  }
  .pfile {
    flex: none;
    white-space: nowrap;
  }
  .act {
    text-transform: capitalize;
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
  .tip {
    position: fixed;
    z-index: 100;
    pointer-events: none;
    max-width: 60vw;
    padding: 4px 8px;
    font-size: 11px;
    background: var(--bg-alt);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 5px;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
    word-break: break-all;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
</style>
