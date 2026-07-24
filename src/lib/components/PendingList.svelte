<script lang="ts">
  import { fmtTime, firstLine, type P4Record, type ReviewInfo } from "$lib/p4";
  import DiffView from "$lib/components/DiffView.svelte";

  let {
    rows,
    loading,
    client,
    refreshKey,
    reviews,
    onLocalFiles,
    onShelvedFiles,
    onLocalDiff,
    onShelvedDiff,
    onOpenLocalDiff,
    onOpenShelvedDiff,
    contextChange,
    onContext,
    onFileContext,
    onMoveFile,
  }: {
    rows: P4Record[];
    loading: boolean;
    client: string; // resets the per-CL cache when the workspace changes
    refreshKey: number; // bumps when pending data changes → refetch open CLs' files
    reviews: Record<string, ReviewInfo | null>; // change → Swarm review status
    contextChange: string; // the changelist whose context menu is open (highlight it)
    onLocalFiles: (change: string) => Promise<P4Record[]>; // opened (workspace) files
    onShelvedFiles: (change: string) => Promise<P4Record[]>; // shelved files
    onLocalDiff: (depotFile: string) => Promise<string>; // local vs server
    onShelvedDiff: (depotFile: string, rev: number, change: string) => Promise<string>;
    onOpenLocalDiff: (depotFile: string) => void;
    onOpenShelvedDiff: (depotFile: string, rev: number, change: string) => void;
    onContext: (cl: P4Record, e: MouseEvent) => void; // right-click a changelist
    // right-click a file → (file, change, event, selected depot files)
    onFileContext: (file: P4Record, change: string, e: MouseEvent, files: string[]) => void;
    onMoveFile: (file: string, toChange: string) => void; // drag a file onto another CL
  } = $props();

  // Multi-select of local (opened) files via click / Ctrl+click / Shift+click.
  let selected = $state<Set<string>>(new Set());
  let anchor: string | null = null;
  // Local files in render order (open changelists only), for Shift-range.
  const orderedFiles = $derived.by(() => {
    const out: string[] = [];
    for (const r of rows) {
      const s = cls[r.change];
      if (s?.open) for (const f of s.local) if (f.depotFile) out.push(f.depotFile);
    }
    return out;
  });
  function clickFile(file: string, e: MouseEvent | KeyboardEvent) {
    if (e.shiftKey && anchor) {
      const a = orderedFiles.indexOf(anchor);
      const b = orderedFiles.indexOf(file);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        selected = new Set(orderedFiles.slice(lo, hi + 1));
      }
    } else if (e.ctrlKey || e.metaKey) {
      const n = new Set(selected);
      if (n.has(file)) n.delete(file);
      else n.add(file);
      selected = n;
      anchor = file;
    } else {
      selected = new Set([file]);
      anchor = file;
    }
  }
  function clearSelection() {
    selected = new Set();
    anchor = null;
  }

  // Rubber-band (marquee) selection: drag over empty space to box-select local
  // file rows. Rows carry data-file; hit-testing is done in viewport (client)
  // coords so it works regardless of scroll.
  let bodyEl: HTMLDivElement;
  let marquee = $state<{ left: number; top: number; width: number; height: number } | null>(null);
  let marqStart: { x: number; y: number } | null = null;
  let marqBase: Set<string> = new Set(); // selection to preserve when Ctrl-adding

  function bodyMouseDown(e: MouseEvent) {
    // Only from empty space (not a row/button, which are child targets) and LMB.
    if (e.button !== 0 || e.target !== e.currentTarget) return;
    e.preventDefault();
    const additive = e.ctrlKey || e.metaKey;
    marqStart = { x: e.clientX, y: e.clientY };
    marqBase = additive ? new Set(selected) : new Set();
    if (!additive) clearSelection();
    window.addEventListener("mousemove", bodyMouseMove);
    window.addEventListener("mouseup", bodyMouseUp);
  }
  function bodyMouseMove(e: MouseEvent) {
    if (!marqStart) return;
    const r = bodyEl.getBoundingClientRect();
    const cx = Math.max(r.left, Math.min(e.clientX, r.right));
    const cy = Math.max(r.top, Math.min(e.clientY, r.bottom));
    const x0 = Math.min(marqStart.x, cx),
      x1 = Math.max(marqStart.x, cx);
    const y0 = Math.min(marqStart.y, cy),
      y1 = Math.max(marqStart.y, cy);
    marquee = { left: x0, top: y0, width: x1 - x0, height: y1 - y0 };
    const next = new Set(marqBase);
    for (const el of bodyEl.querySelectorAll<HTMLElement>("[data-file]")) {
      const b = el.getBoundingClientRect();
      if (b.bottom >= y0 && b.top <= y1) next.add(el.dataset.file!);
    }
    selected = next;
  }
  function bodyMouseUp() {
    marqStart = null;
    marquee = null;
    window.removeEventListener("mousemove", bodyMouseMove);
    window.removeEventListener("mouseup", bodyMouseUp);
  }

  // Drag-and-drop: move an opened file from one changelist to another.
  let drag = $state<{ file: string; from: string } | null>(null);
  let dragOver = $state<string | null>(null); // CL currently hovered as a drop target

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

  // Stale-while-revalidate: keep any existing files visible during a refetch so
  // a refresh (e.g. after moving a file) doesn't flash "Loading…".
  async function loadCL(change: string) {
    const prev = cls[change];
    cls[change] = {
      open: prev?.open ?? true,
      loading: !prev, // only the very first load shows the spinner
      local: prev?.local ?? [],
      shelved: prev?.shelved ?? [],
      shelvedOpen: prev?.shelvedOpen ?? false,
    };
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

  // Optimistic move: reflect the move in the UI immediately (we already have the
  // file record), then fire the p4 command. The reload it triggers reconciles —
  // and rolls back — once p4 answers, so a failed move snaps back on its own.
  // Exported so the right-click "Move to changelist" menu shares this one path.
  export function moveFile(file: string, from: string, to: string) {
    const src = cls[from];
    const rec = src?.local.find((f) => f.depotFile === file);
    if (src && rec) {
      cls[from] = { ...src, local: src.local.filter((f) => f.depotFile !== file) };
      const dst = cls[to];
      if (dst) cls[to] = { ...dst, local: [...dst.local, { ...rec }] };
    }
    onMoveFile(file, to);
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
  // changes — CL keys like "default" are reused across workspaces. When
  // refreshKey bumps (a pending mutation reloaded the list), refetch the files
  // of every already-open changelist so moved/reverted files show immediately.
  let lastClient = "";
  let lastKey = -1;
  $effect(() => {
    const key = refreshKey;
    if (client !== lastClient) {
      lastClient = client;
      cls = {};
      fdiff = {};
      selected = new Set();
      anchor = null;
    }
    const forced = key !== lastKey;
    lastKey = key;
    for (const r of rows) {
      if (!cls[r.change]) loadCL(r.change);
      else if (forced && cls[r.change].open) loadCL(r.change);
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
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="scroll body"
    bind:this={bodyEl}
    onmousedown={bodyMouseDown}
    onkeydown={(e) => e.key === "Escape" && clearSelection()}
  >
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if rows.length === 0}
      <div class="msg dim">{client ? "No pending changelists." : "Select a workspace to browse."}</div>
    {:else}
      {#each rows as r (r.change)}
        {@const s = cls[r.change]}
        {@const rv = reviews[r.change]}
        {@const empty = !!s && !s.loading && s.local.length === 0 && s.shelved.length === 0}
        <button
          class="cl"
          class:dropinto={dragOver === r.change}
          class:contextsel={contextChange === r.change}
          onclick={() => toggleCL(r.change)}
          oncontextmenu={(e) => onContext(r, e)}
          ondragover={(e) => {
            if (drag && drag.from !== r.change) {
              e.preventDefault();
              dragOver = r.change;
            }
          }}
          ondragleave={() => {
            if (dragOver === r.change) dragOver = null;
          }}
          ondrop={(e) => {
            e.preventDefault();
            if (drag && drag.from !== r.change) moveFile(drag.file, drag.from, r.change);
            drag = null;
            dragOver = null;
          }}
        >
          <span class="tw">{empty ? "" : s?.open ? "▾" : "▸"}</span>
          <span class="cnum mono">{r.change === "default" ? "Default" : "@" + r.change}</span>
          <span class="desc" title={r.desc}>
            {r.change === "default" ? "" : firstLine(r.desc) || "(no description)"}
          </span>
          {#if rv}
            <span
              class="review rv-{rv.state}"
              title={"Swarm review" + (rv.id ? " #" + rv.id : "") + ": " + rv.stateLabel}
            >
              {rv.stateLabel}
            </span>
          {/if}
          <span class="user dim">{r.user}</span>
          <span class="date dim">{fmtTime(r.time)}</span>
        </button>
        {#if s?.open && !empty}
          {#if s.loading}
            <div class="finfo dim">Loading files…</div>
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
  {#if marquee}
    <div
      class="marquee"
      style="left:{marquee.left}px;top:{marquee.top}px;width:{marquee.width}px;height:{marquee.height}px"
    ></div>
  {/if}
</div>

{#snippet fileRow(f: P4Record, change: string, kind: "local" | "shelved", depth: number)}
  {@const key = `${change}|${kind}|${f.depotFile}`}
  {@const fd = fdiff[key]}
  {@const sp = splitPath(f.depotFile)}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="frow mono"
    data-file={kind === "local" ? f.depotFile : undefined}
    class:dragging={drag?.file === f.depotFile}
    class:selected={kind === "local" && selected.has(f.depotFile)}
    style="padding-left:{depth * 16 + 4}px"
    title={"Double-click to open in external diff\n" + f.depotFile}
    draggable={kind === "local"}
    onclick={(e) => kind === "local" && clickFile(f.depotFile, e)}
    onkeydown={(e) => {
      if (kind === "local" && (e.key === "Enter" || e.key === " ")) {
        e.preventDefault();
        clickFile(f.depotFile, e);
      }
    }}
    ondblclick={() => openExt(change, kind, f)}
    oncontextmenu={(e) => {
      if (kind === "local") {
        e.preventDefault();
        // Right-click selects the item (unless it's already in the selection).
        if (!selected.has(f.depotFile)) {
          selected = new Set([f.depotFile]);
          anchor = f.depotFile;
        }
        onFileContext(f, change, e, [...selected]);
      }
    }}
    ondragstart={(e) => {
      if (kind !== "local") return;
      drag = { file: f.depotFile, from: change };
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", f.depotFile);
      }
    }}
    ondragend={() => {
      drag = null;
      dragOver = null;
    }}
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
    <span class="fpath"><span class="pfile">{sp.name}</span><span class="pdir dim">{sp.dir}</span></span>
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
  .marquee {
    position: fixed;
    z-index: 50;
    pointer-events: none;
    border: 1px solid var(--accent);
    background: var(--bg-sel);
    opacity: 0.35;
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
    user-select: none;
  }
  .cl:hover {
    background: var(--bg-hover);
  }
  .cl.dropinto {
    background: var(--bg-sel);
    outline: 1px dashed var(--accent);
    outline-offset: -2px;
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
  .review {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 10px;
    border: 1px solid currentColor;
    white-space: nowrap;
    color: var(--text-dim);
  }
  .rv-needsReview {
    color: var(--accent);
  }
  .rv-approved {
    color: var(--have);
  }
  .rv-needsRevision,
  .rv-rejected {
    color: var(--warn);
  }
  .rv-requested,
  .rv-archived {
    color: var(--text-dim);
    font-style: italic;
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
    user-select: none;
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
    user-select: none;
  }
  .frow:hover {
    background: var(--bg-hover);
  }
  .frow[draggable="true"] {
    cursor: grab;
  }
  .frow.dragging {
    opacity: 0.4;
  }
  .frow.selected {
    background: var(--bg-sel);
  }
  .frow.selected:hover {
    background: var(--bg-sel);
  }
  .cl.contextsel {
    background: var(--bg-sel);
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
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
  .fpath .pfile {
    flex: none;
    white-space: nowrap;
  }
  .fpath .pdir {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
