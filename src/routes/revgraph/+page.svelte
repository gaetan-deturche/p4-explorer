<script lang="ts">
  //! Revision graph: every depot path a file has lived at, one lane each, its
  //! revisions along them, and an arrow for each branch, copy or move between
  //! them — P4V's timeline view.
  //!
  //! Columns are EVENTS, not time: every revision of every lane gets its own
  //! column, in time order, so years of quiet and a busy afternoon take the same
  //! room and nothing overlaps. A revision's box runs on to its lane's next one.
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import ChangeDetails from "$lib/components/ChangeDetails.svelte";
  import { history } from "$lib/history.svelte";
  import { cacheGet, cacheSet } from "$lib/store.svelte";
  import { openDiff } from "$lib/opendiff";
  import { editor } from "$lib/editor.svelte";
  import { shortcuts } from "$lib/shortcuts.svelte";
  import {
    p4,
    emptyConn,
    setClipboard,
    openBlameWindow,
    type P4Conn,
    type RevGraph,
    type GraphRev,
    type GraphEdge,
  } from "$lib/p4";

  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  let conn = $state<P4Conn>(emptyConn());
  let file = $state("");
  let graph = $state<RevGraph | null>(null);
  let error = $state("");
  let notice = $state("");
  let loading = $state(true);
  /** Show the streams it was branched into too (P4V hides them as well). */
  let includeInto = $state(false);

  function setNotice(m: string, ms = 4000) {
    notice = m;
    window.setTimeout(() => (notice = ""), ms);
  }

  onMount(async () => {
    try {
      const job = await invoke<{ conn: P4Conn; depotFile: string }>("file_history_job", { id });
      conn = job.conn;
      file = job.depotFile;
      void editor.init(); // openDiff reads the diff-tool choice from this store
      void shortcuts.init();
      history.init({ conn: () => conn, setNotice, toQuery: (path: string) => path });
      includeInto = ((await cacheGet("nav", "revgraph-into")) ?? "0") === "1";
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  });

  async function load() {
    graph = await p4.revisionGraph(conn, file, includeInto);
    atEnd = false;
  }

  // The newest revisions are the ones asked about: start at the right end, once
  // the graph is on screen (it is not while `loading` still holds the spinner).
  let atEnd = false;
  $effect(() => {
    if (!scrollEl || !layout || atEnd) return;
    atEnd = true;
    void tick().then(() => scrollEl && (scrollEl.scrollLeft = scrollEl.scrollWidth));
  });

  async function setIncludeInto(v: boolean) {
    includeInto = v;
    cacheSet("nav", "revgraph-into", v ? "1" : "0");
    loading = true;
    try {
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  // --- layout -------------------------------------------------------------------
  const COL_W = 50;
  const LANE_H = 58;
  const LABEL_H = 16;
  const BOX_H = 22;
  const TOP = 22; // room for the date ticks
  const TAIL = 3; // columns a lane's last revision fades out over

  type Node = { lane: number; file: string; rev: GraphRev; col: number; end: number };
  const key = (f: string, r: number) => `${f}#${r}`;
  /** A revision named so two of the same file on different paths differ:
   *  `//CuriosityP4/Dev/Main/…/HurtboxComponent.cpp#2`. */
  function short(k: string): string {
    const parts = k.slice(2).split("/");
    return parts.length <= 4 ? k : `//${parts.slice(0, 3).join("/")}/…/${parts[parts.length - 1]}`;
  }

  const layout = $derived.by(() => {
    if (!graph) return null;
    const all: { lane: number; rev: GraphRev }[] = [];
    graph.lanes.forEach((l, lane) => l.revs.forEach((rev) => all.push({ lane, rev })));
    all.sort((a, b) => a.rev.time - b.rev.time || Number(a.rev.change) - Number(b.rev.change) || a.lane - b.lane);
    const cols = all.length;
    const byKey = new Map<string, Node>();
    const nodes: Node[] = all.map((e, col) => {
      const n = { lane: e.lane, file: graph!.lanes[e.lane].file, rev: e.rev, col, end: -1 };
      byKey.set(key(n.file, e.rev.rev), n);
      return n;
    });
    // Each box runs on to its lane's next revision.
    for (let lane = 0; lane < graph.lanes.length; lane++) {
      const mine = nodes.filter((n) => n.lane === lane);
      mine.forEach((n, i) => (n.end = i + 1 < mine.length ? mine[i + 1].col : -1));
    }
    // Month ticks along the top, where the month changes.
    const ticks: { col: number; text: string }[] = [];
    let last = "";
    for (const n of nodes) {
      const d = new Date(n.rev.time * 1000);
      const m = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
      if (m !== last) ticks.push({ col: n.col, text: m });
      last = m;
    }
    const edges = graph.edges
      .map((e) => ({ e, a: byKey.get(key(e.fromFile, e.fromRev)), b: byKey.get(key(e.toFile, e.toRev)) }))
      .filter((x): x is { e: GraphEdge; a: Node; b: Node } => !!x.a && !!x.b);
    const firstCol = graph.lanes.map((_, lane) => nodes.find((n) => n.lane === lane)?.col ?? 0);
    return { nodes, byKey, edges, cols, ticks, firstCol };
  });

  const width = $derived(layout ? (layout.cols + TAIL + 1) * COL_W : 0);
  const height = $derived(graph ? TOP + graph.lanes.length * LANE_H + 8 : 0);
  const boxY = (lane: number) => TOP + lane * LANE_H + LABEL_H + 4;

  /** The arrow from a source revision's box to the box it produced. */
  function arrow(a: Node, b: Node): string {
    const down = b.lane > a.lane;
    const x1 = a.col * COL_W + COL_W / 2;
    const y1 = boxY(a.lane) + (down ? BOX_H : 0);
    const x2 = b.col * COL_W + 8;
    const y2 = boxY(b.lane) + (down ? 0 : BOX_H) + (down ? -1 : 1);
    if (a.lane === b.lane) {
      // Same path (an undo, or a copy within the file): arc over the lane.
      const y = boxY(a.lane) - 6;
      return `M${x1},${boxY(a.lane)} C${x1},${y} ${x2},${y} ${x2},${boxY(a.lane)}`;
    }
    return `M${x1},${y1} L${x2},${y2}`;
  }

  const verb: Record<string, string> = {
    branch: "branched",
    copy: "copied",
    merge: "merged",
    move: "moved",
    edit: "integrated (edited)",
    add: "integrated (added)",
    delete: "integrated (deleted)",
    ignore: "ignored",
    undo: "undone",
  };
  function edgeTitle(e: GraphEdge): string {
    const range = e.fromStart && e.fromStart !== e.fromRev ? `#${e.fromStart}–${e.fromRev}` : `#${e.fromRev}`;
    return `${e.toFile}#${e.toRev}\n${verb[e.how] ?? e.how} from ${e.fromFile}${range}`;
  }
  function when(t: number): string {
    const d = new Date(t * 1000);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  }
  function nodeTitle(n: Node): string {
    const r = n.rev;
    return `${n.file}#${r.rev}\n@${r.change} · ${r.action} · ${r.user} · ${when(r.time)}\n${r.desc}\n\nClick: changelist · Ctrl+click: pick two to diff · Double-click: diff against previous`;
  }

  /** The focus file's head: where you came from, marked as P4V does. */
  const focusKey = $derived.by(() => {
    const lane = graph?.lanes.find((l) => l.file === graph!.focus);
    const head = lane?.revs[lane.revs.length - 1];
    return lane && head ? key(lane.file, head.rev) : "";
  });

  // --- selection, diffs ------------------------------------------------------------
  /** Up to two picked revisions: one shows its changelist, two can be diffed. */
  let picked = $state<string[]>([]);
  let clShown = $state("");
  function pick(n: Node, e: MouseEvent) {
    const k = key(n.file, n.rev.rev);
    if (e.ctrlKey || e.metaKey || e.shiftKey) {
      picked = picked.includes(k) ? picked.filter((x) => x !== k) : [...picked.slice(-1), k];
      return;
    }
    picked = [k];
    clShown = n.rev.change;
    void history.selectChange(n.rev.change);
  }

  /** What a revision should be compared with: the lane's previous revision, or
   *  for a lane's first one, the revision it was branched/copied/moved from. */
  function previousOf(n: Node): string {
    const lane = graph!.lanes[n.lane];
    const i = lane.revs.findIndex((r) => r.rev === n.rev.rev);
    if (i > 0) return key(lane.file, lane.revs[i - 1].rev);
    const from = layout?.edges.find((x) => x.b === n);
    return from ? key(from.a.file, from.a.rev.rev) : "";
  }
  function diffPrevious(n: Node) {
    const prev = previousOf(n);
    const here = key(n.file, n.rev.rev);
    if (!prev) {
      // Nothing before it anywhere: show it as added.
      void openDiff(conn, { kind: "rev", file: n.file, rev: n.rev.rev }, setNotice);
      return;
    }
    void openDiff(conn, { kind: "specs", file: n.file, left: prev, right: here }, setNotice);
  }
  /** The two picked revisions, older on the left. */
  function diffPicked() {
    const [a, b] = picked.map((k) => layout!.byKey.get(k)!).sort((x, y) => x.col - y.col);
    if (!a || !b) return;
    void openDiff(
      conn,
      { kind: "specs", file: b.file, left: key(a.file, a.rev.rev), right: key(b.file, b.rev.rev) },
      setNotice,
    );
  }

  // --- context menu ----------------------------------------------------------------
  let ctx = $state<{ x: number; y: number; node: Node } | null>(null);
  function menuItems(n: Node) {
    const here = key(n.file, n.rev.rev);
    const other = picked.find((k) => k !== here);
    return [
      { label: `Show @${n.rev.change}`, action: () => pick(n, new MouseEvent("click")) },
      { label: "", sep: true },
      { label: "Diff against previous revision", action: () => diffPrevious(n) },
      ...(other
        ? [
            {
              label: `Diff against ${short(other)}`,
              action: () => {
                picked = [other, here];
                diffPicked();
              },
            },
          ]
        : []),
      { label: `Blame ${here.split("/").pop()}…`, action: () => void openBlameWindow(conn, n.file, `#${n.rev.rev}`) },
      { label: "", sep: true },
      { label: "Copy revision", action: () => void setClipboard(here).then(() => setNotice(`Copied ${here}.`, 2000)) },
      { label: "Copy changelist number", action: () => void setClipboard(n.rev.change) },
    ];
  }

  // --- window ----------------------------------------------------------------------
  let scrollEl = $state<HTMLDivElement | undefined>();
  function close() {
    void getCurrentWindow().close();
  }
  function onWinKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      if (ctx) ctx = null;
      else if (clShown) clShown = "";
      else close();
      return;
    }
    if (shortcuts.match(e, ["app"]) === "closeWindow") {
      e.preventDefault();
      close();
    }
  }
  /** A plain wheel scrolls along the timeline: the graph is wide, rarely tall. */
  function onWheel(e: WheelEvent) {
    if (!scrollEl || e.ctrlKey) return;
    if (e.shiftKey || scrollEl.scrollHeight <= scrollEl.clientHeight) {
      e.preventDefault();
      scrollEl.scrollLeft += e.deltaY || e.deltaX;
    }
  }
</script>

<svelte:window onkeydown={onWinKey} />

<div class="wrap">
  <div class="bar">
    <span class="name mono" title={file}>{file}</span>
    <span class="grow"></span>
    {#if graph}
      <span class="dim">
        {graph.lanes.length} path{graph.lanes.length === 1 ? "" : "s"} ·
        {graph.lanes.reduce((n, l) => n + l.revs.length, 0)} revisions
      </span>
    {/if}
    {#if picked.length === 2}
      <button onclick={diffPicked} title="Diff the two picked revisions, older on the left">
        Diff {picked.map(short).join(" ↔ ")}
      </button>
    {/if}
    <label class="opt" title="Also show the paths this file was branched or copied into (other streams).">
      <input type="checkbox" checked={includeInto} onchange={(e) => void setIncludeInto(e.currentTarget.checked)} />
      branches into other paths{graph && !includeInto && graph.hiddenInto ? ` (${graph.hiddenInto})` : ""}
    </label>
    <button onclick={close}>Close</button>
  </div>

  {#if error}
    <div class="err mono">{error}</div>
  {/if}
  {#if notice}
    <div class="note">{notice}</div>
  {/if}

  {#if ctx}
    <ContextMenu x={ctx.x} y={ctx.y} items={menuItems(ctx.node)} onClose={() => (ctx = null)} />
  {/if}

  {#if clShown}
    <aside class="clpanel">
      <div class="clhead">
        <span class="mono">@{clShown}</span>
        <button class="clclose" title="Close (Esc)" onclick={() => (clShown = "")}>✕</button>
      </div>
      <div class="clbody">
        <ChangeDetails
          change={clShown}
          rows={history.descRows}
          loading={history.descLoading}
          onDiff={(f, r) => history.fileDiff(f, r)}
          onOpenDiff={(f, r) => history.openFileDiff(f, r)}
        />
      </div>
    </aside>
  {/if}

  {#if loading}
    <div class="pad dim">Reading the file's history…</div>
  {:else if graph && layout}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="scroll" bind:this={scrollEl} onwheel={onWheel}>
      <svg {width} {height} class="mono" role="img" aria-label="Revision graph of {file}">
        <defs>
          <marker id="arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0,0 L8,4 L0,8 z" class="head" />
          </marker>
          {#each graph.lanes as _, lane}
            <linearGradient id="fade{lane}" x1="0" x2="1">
              <stop offset="0" class="tailstop" />
              <stop offset="1" class="tailstop" stop-opacity="0" />
            </linearGradient>
          {/each}
        </defs>

        <!-- One stripe per column: an event each, so the eye can follow it down. -->
        {#each { length: layout.cols + TAIL + 1 } as _, c}
          {#if c % 2}
            <rect x={c * COL_W} y={TOP} width={COL_W} height={height - TOP} class="stripe" />
          {/if}
        {/each}
        {#each layout.ticks as t (t.col)}
          <text x={t.col * COL_W + 2} y={14} class="tick">{t.text}</text>
          <line x1={t.col * COL_W} x2={t.col * COL_W} y1={16} y2={height} class="tickline" />
        {/each}

        {#each graph.lanes as lane, li (lane.file)}
          <text x={layout.firstCol[li] * COL_W + 2} y={TOP + li * LANE_H + 12} class="path" class:related={lane.related}>
            {lane.file}
          </text>
        {/each}

        <!-- Tails first, so every box and arrow draws over them. -->
        {#each layout.nodes as n (key(n.file, n.rev.rev))}
          {@const x = n.col * COL_W}
          {#if n.end > 0}
            <rect x={x + COL_W - 4} y={boxY(n.lane) + 3} width={(n.end - n.col) * COL_W - COL_W + 2} height={BOX_H - 6} class="tail" />
          {:else}
            <rect x={x + COL_W - 4} y={boxY(n.lane) + 3} width={TAIL * COL_W} height={BOX_H - 6} fill="url(#fade{n.lane})" />
          {/if}
        {/each}

        {#each layout.edges as { e, a, b } (key(e.fromFile, e.fromRev) + ">" + key(e.toFile, e.toRev))}
          <path d={arrow(a, b)} class="edge {e.how}" marker-end="url(#arrow)"><title>{edgeTitle(e)}</title></path>
        {/each}

        {#each layout.nodes as n (key(n.file, n.rev.rev))}
          {@const k = key(n.file, n.rev.rev)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <g
            class="node"
            class:picked={picked.includes(k)}
            class:focus={k === focusKey}
            class:deleted={n.rev.action.includes("delete")}
            onclick={(e) => pick(n, e)}
            ondblclick={() => diffPrevious(n)}
            oncontextmenu={(e) => {
              e.preventDefault();
              ctx = { x: e.clientX, y: e.clientY, node: n };
            }}
          >
            <title>{nodeTitle(n)}</title>
            <rect x={n.col * COL_W + 2} y={boxY(n.lane)} width={COL_W - 4} height={BOX_H} rx="3" />
            <text x={n.col * COL_W + 8} y={boxY(n.lane) + 15}>{n.rev.rev}</text>
          </g>
        {/each}
      </svg>
    </div>
  {/if}
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .grow {
    flex: 1;
  }
  .opt {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: var(--text-dim, #999);
    cursor: pointer;
    white-space: nowrap;
  }
  .dim {
    opacity: 0.65;
  }
  .mono {
    font-family: var(--mono);
  }
  .err,
  .note,
  .pad {
    flex: none;
    padding: 6px 10px;
    font-size: 12px;
  }
  .err {
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 12%, transparent);
  }
  .clpanel {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(46vw, 620px);
    z-index: 40;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border-left: 1px solid var(--border);
    box-shadow: -6px 0 18px rgb(0 0 0 / 0.28);
  }
  .clhead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 6px 4px 10px;
    border-bottom: 1px solid var(--border);
  }
  .clclose {
    border: none;
    background: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 2px 6px;
  }
  .clclose:hover {
    color: var(--text);
  }
  .clbody {
    flex: 1;
    overflow: auto;
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  svg {
    display: block;
    font-size: 11px;
  }
  .stripe {
    fill: color-mix(in srgb, var(--text, #ddd) 5%, transparent);
  }
  .tick {
    fill: var(--text-dim, #999);
    font-size: 10px;
  }
  .tickline {
    stroke: color-mix(in srgb, var(--text, #ddd) 12%, transparent);
  }
  .path {
    fill: var(--text-dim, #999);
    font-size: 10.5px;
  }
  .path.related {
    font-style: italic;
  }
  .tail {
    fill: color-mix(in srgb, var(--accent, #6b8afd) 22%, transparent);
  }
  .tailstop {
    stop-color: color-mix(in srgb, var(--accent, #6b8afd) 22%, transparent);
  }
  .node {
    cursor: pointer;
  }
  .node rect {
    fill: color-mix(in srgb, var(--accent, #6b8afd) 38%, var(--bg-panel, #222));
    stroke: color-mix(in srgb, var(--accent, #6b8afd) 80%, transparent);
    stroke-width: 1;
  }
  .node text {
    fill: var(--text, #ddd);
    pointer-events: none;
  }
  .node:hover rect {
    fill: color-mix(in srgb, var(--accent, #6b8afd) 60%, var(--bg-panel, #222));
  }
  .node.deleted rect {
    fill: color-mix(in srgb, #e06c6c 35%, var(--bg-panel, #222));
    stroke: #e06c6c;
  }
  /* Where the window was opened from, as P4V marks it. */
  .node.focus rect {
    fill: #e8d44d;
    stroke: #b49d1c;
  }
  .node.focus text {
    fill: #222;
  }
  .node.picked rect {
    stroke: var(--text, #fff);
    stroke-width: 2.5;
  }
  .edge {
    fill: none;
    stroke: var(--text-dim, #aaa);
    stroke-width: 1.3;
  }
  .edge:hover {
    stroke: var(--text, #fff);
    stroke-width: 2.2;
  }
  .edge.move {
    stroke: #c58af9;
  }
  .edge.merge,
  .edge.edit,
  .edge.add,
  .edge.delete {
    stroke-dasharray: 5 3;
  }
  .edge.ignore,
  .edge.undo {
    stroke-dasharray: 2 3;
    opacity: 0.7;
  }
  .head {
    fill: var(--text-dim, #aaa);
  }
</style>
