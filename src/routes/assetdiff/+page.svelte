<script lang="ts">
  //! The asset diff window: two revisions of a .uasset / .umap compared object by
  //! object and property by property, read straight from the files — hidden
  //! properties, graph links and references included, no editor needed.
  //!
  //! The rows come from Rust as one flat tree in display order (a row's children
  //! follow it with a greater depth). What is shown is decided here: changed
  //! branches open, unchanged ones closed, and "changes only" drops the rest.
  //!
  //! Above the tree, the asset's graphs (event graph, functions, a material's
  //! graph) are drawn: both revisions side by side, sharing one pan and zoom.
  //! Clicking a node shows its entry in the tree.
  import { onMount, tick, untrack } from "svelte";
  import { p4, assetDiffJob, setClipboard, type AssetDiff } from "$lib/p4";
  import { cacheGet, cacheSet } from "$lib/store.svelte";
  import { bounds, geom, type GNode } from "$lib/assetgraph";
  import AssetGraph from "$lib/components/AssetGraph.svelte";

  const params = new URLSearchParams(window.location.search);
  const leftPath = params.get("left") ?? "";
  const rightPath = params.get("right") ?? "";
  const leftLabel = params.get("ll") ?? "left";
  const rightLabel = params.get("rl") ?? "right";
  const title = params.get("title") ?? "";
  const job = params.get("job") ?? "";

  const RH = 22; // row height
  /** An added or removed branch this small opens by itself. */
  const SMALL = 8;

  let data = $state<AssetDiff | null>(null);
  let error = $state("");
  let notice = $state("");
  let loading = $state(true);
  let onlyChanges = $state(true);
  let filter = $state("");
  /** Branches the user opened or closed, overriding the default. */
  let toggled = $state(new Map<number, boolean>());
  let selected = $state(-1);
  let scrollEl = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let viewH = $state(600);

  /** The graph drawn above the tree. */
  let graphName = $state("");
  /** Height of the graph section, and whether it is folded away. */
  let graphH = $state(Math.round(window.innerHeight * 0.5));
  let graphOpen = $state(true);
  let view = $state({ x: 0, y: 0, k: 1 });
  let selectedNode = $state("");
  let dimSame = $state(true);
  let areaW = $state(0);
  let areaH = $state(0);
  let changeAt = $state(-1);
  let fitted = ""; // the graph the view was last fitted to

  const graph = $derived(data?.graphs.find((g) => g.name === graphName) ?? null);
  /** The side a graph exists on when only one revision has it, else null. */
  const graphOnly = $derived(graph && !graph.left ? "right" : graph && !graph.right ? "left" : null);
  /** Width of one graph pane: the whole area when only one side is drawn. */
  const paneW = $derived(graphOnly ? areaW : areaW / 2);

  function pickGraph(name: string) {
    graphName = name;
    selectedNode = "";
    changeAt = -1;
    fitted = "";
  }

  /** Drag the splitter between the graph and the tree. */
  function splitDown(e: PointerEvent) {
    const y0 = e.clientY;
    const h0 = graphH;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      graphH = Math.max(120, Math.min(window.innerHeight - 160, h0 + m.clientY - y0));
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      cacheSet("nav", "assetdiff-graphh", String(graphH));
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up, { once: true });
  }

  /** Both revisions in view, in the pane's size. */
  function fit() {
    const b = graph && bounds([graph.left, graph.right]);
    if (!b) return;
    const w = paneW;
    const h = areaH - 24;
    const k = Math.min(1, Math.max(0.03, Math.min((w - 40) / Math.max(b.w, 1), (h - 40) / Math.max(b.h, 1))));
    view = { k, x: (w - b.w * k) / 2 - b.x * k, y: (h - b.h * k) / 2 - b.y * k };
  }
  $effect(() => {
    if (graph && graphOpen && areaW > 0 && fitted !== graph.name) {
      fitted = graph.name;
      untrack(fit);
    }
  });

  /** The nodes that differ, top to bottom (moved-only nodes are not changes). */
  const changed = $derived.by(() => {
    const byId = new Map<string, GNode>();
    for (const n of [...(graph?.left?.nodes ?? []), ...(graph?.right?.nodes ?? [])]) {
      if (n.status !== "same") byId.set(n.id, n);
    }
    return [...byId.values()].sort((a, b) => a.y - b.y || a.x - b.x);
  });

  function nextChange(step: number) {
    if (!changed.length) return;
    changeAt = (changeAt + step + changed.length) % changed.length;
    const n = changed[changeAt];
    void selectNode(n.id);
    const g = geom(n);
    const k = Math.max(view.k, 0.6);
    view = { k, x: paneW / 2 - (g.x + g.w / 2) * k, y: (areaH - 24) / 2 - (g.y + g.h / 2) * k };
  }

  /** A node's row in the tree (under All data › Graphs), or -1. */
  function nodeRow(graphName: string, id: string): number {
    const path = ["All data", "Graphs", graphName, id];
    let lo = 0;
    let hi = rows.length;
    let found = -1;
    for (let depth = 0; depth < path.length; depth++) {
      found = -1;
      for (let j = lo; j < hi; j++) {
        if (rows[j].depth === depth && rows[j].key === path[depth]) {
          found = j;
          break;
        }
      }
      if (found < 0) return -1;
      lo = found + 1;
      hi = shape.end[found];
    }
    return found;
  }

  /** Select a node on both sides and show its entry in the tree, opened. */
  async function selectNode(id: string) {
    selectedNode = id;
    const at = graph ? nodeRow(graph.name, id) : -1;
    if (at < 0) return;
    filter = "";
    if (onlyChanges && rows[at].status === "same") setOnlyChanges(false);
    const next = new Map(toggled);
    for (let p = at; p >= 0; p = shape.parent[p]) next.set(p, true);
    toggled = next;
    selected = at;
    await tick();
    const n = visible.indexOf(at);
    if (n >= 0 && scrollEl) scrollEl.scrollTop = Math.max(0, n * RH - 2 * RH);
  }

  function setNotice(m: string, ms = 5000) {
    notice = m;
    window.setTimeout(() => (notice = ""), ms);
  }

  onMount(async () => {
    onlyChanges = ((await cacheGet("nav", "assetdiff-changes")) ?? "1") === "1";
    const h = Number(await cacheGet("nav", "assetdiff-graphh"));
    if (h > 0) graphH = Math.min(h, window.innerHeight - 160);
    try {
      data = await p4.assetDiff(leftPath, rightPath);
      // Open on the graph that changed most.
      const g = [...data.graphs].sort((a, b) => b.changes - a.changes)[0];
      if (g) graphName = g.name;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  });

  const rows = $derived(data?.rows ?? []);
  /** The only side the asset exists on (added or deleted), else null. */
  const only = $derived.by(() => {
    const top = rows.filter((r) => r.depth === 0);
    if (!top.length) return null;
    if (top.every((r) => r.status === "added")) return "right";
    if (top.every((r) => r.status === "removed")) return "left";
    return null;
  });
  /** Index just past each row's subtree, and each row's parent. */
  const shape = $derived.by(() => {
    const end = new Array<number>(rows.length);
    const parent = new Array<number>(rows.length).fill(-1);
    const stack: number[] = [];
    for (let i = 0; i < rows.length; i++) {
      while (stack.length && rows[stack[stack.length - 1]].depth >= rows[i].depth) end[stack.pop()!] = i;
      if (stack.length) parent[i] = stack[stack.length - 1];
      stack.push(i);
    }
    for (const i of stack) end[i] = rows.length;
    return { end, parent };
  });

  function isOpen(i: number): boolean {
    const t = toggled.get(i);
    if (t !== undefined) return t;
    const r = rows[i];
    // The raw package contents behind an asset-type summary: there when wanted.
    if (r.depth === 0 && r.key === "All data") return false;
    if (r.status === "changed") return true;
    if (r.status === "same") return false;
    return shape.end[i] - i - 1 <= SMALL;
  }

  function matches(i: number, term: string): boolean {
    const r = rows[i];
    return (
      r.key.toLowerCase().includes(term) ||
      (r.left ?? "").toLowerCase().includes(term) ||
      (r.right ?? "").toLowerCase().includes(term)
    );
  }

  /** The rows on screen, in order. */
  const visible = $derived.by(() => {
    const out: number[] = [];
    const term = filter.trim().toLowerCase();
    // With a filter: the matching rows, the branches leading to them, and what
    // an open match holds.
    let keep: Uint8Array | null = null;
    if (term) {
      keep = new Uint8Array(rows.length);
      for (let i = 0; i < rows.length; i++) {
        if (!matches(i, term)) continue;
        for (let p = i; p >= 0 && !keep[p]; p = shape.parent[p]) keep[p] = 1;
        for (let c = i + 1; c < shape.end[i]; c++) keep[c] = 1;
      }
    }
    let i = 0;
    while (i < rows.length) {
      const r = rows[i];
      if ((onlyChanges && r.status === "same") || (keep && !keep[i])) {
        i = shape.end[i];
        continue;
      }
      out.push(i);
      const open = keep && keep[i] && !matches(i, term) ? true : isOpen(i);
      i = r.kids && !open ? shape.end[i] : i + 1;
    }
    return out;
  });

  const first = $derived(Math.max(0, Math.floor(scrollTop / RH) - 10));
  const last = $derived(Math.min(visible.length, Math.ceil((scrollTop + viewH) / RH) + 10));

  function toggle(i: number, open = !isOpen(i)) {
    const next = new Map(toggled);
    next.set(i, open);
    toggled = next;
  }

  /** Open or close every branch at once. */
  function setAll(open: boolean) {
    const next = new Map<number, boolean>();
    rows.forEach((r, i) => r.kids && next.set(i, open));
    toggled = next;
  }

  function setOnlyChanges(v: boolean) {
    onlyChanges = v;
    cacheSet("nav", "assetdiff-changes", v ? "1" : "0");
  }

  /** "Objects › BP_C › Properties › Speed" for the clipboard. */
  function pathOf(i: number): string {
    const parts: string[] = [];
    for (let p = i; p >= 0; p = shape.parent[p]) parts.unshift(rows[p].key);
    return parts.join(" › ");
  }

  function copyRow(i: number) {
    const r = rows[i];
    const side = (v: string | null) => v ?? "(none)";
    const text =
      r.status === "same" ? `${pathOf(i)} = ${side(r.right)}` : `${pathOf(i)}: ${side(r.left)} → ${side(r.right)}`;
    setClipboard(text).then(() => setNotice("Copied."), (e) => setNotice(String(e)));
  }

  function scrollToRow(n: number) {
    if (!scrollEl) return;
    const y = n * RH;
    if (y < scrollEl.scrollTop) scrollEl.scrollTop = y;
    else if (y + RH > scrollEl.scrollTop + scrollEl.clientHeight) scrollEl.scrollTop = y + RH - scrollEl.clientHeight;
  }

  function onKey(e: KeyboardEvent) {
    if ((e.target as HTMLElement).tagName === "INPUT") return;
    const at = visible.indexOf(selected);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = Math.max(0, Math.min(visible.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
      selected = visible[n] ?? -1;
      scrollToRow(n);
    } else if (e.key === "ArrowRight" && selected >= 0 && rows[selected].kids) {
      toggle(selected, true);
    } else if (e.key === "ArrowLeft" && selected >= 0) {
      if (rows[selected].kids && isOpen(selected)) toggle(selected, false);
      else if (shape.parent[selected] >= 0) {
        selected = shape.parent[selected];
        scrollToRow(visible.indexOf(selected));
      }
    } else if ((e.ctrlKey || e.metaKey) && e.key === "c" && selected >= 0 && !window.getSelection()?.toString()) {
      e.preventDefault();
      copyRow(selected);
    }
  }

  async function openInUnreal() {
    try {
      const j = await assetDiffJob(job);
      setNotice("Opening Unreal diff…", 15000);
      const mode = await p4.openUnrealDiff(j.conn, leftPath, rightPath, j.name, leftLabel, rightLabel);
      setNotice(
        mode === "remote"
          ? "Diff opened in the running Unreal Editor."
          : mode === "nocompare"
            ? "No earlier revision of this asset to compare."
            : mode.startsWith("single")
              ? "No earlier revision — opened the asset in Unreal instead."
              : "Launching Unreal Editor for the diff — this takes a moment…",
        8000,
      );
    } catch (e) {
      setNotice(String(e), 8000);
    }
  }

  const MARK = { same: "", changed: "~", added: "+", removed: "−" } as const;

  /** A container's size ("{4}", "[12]") rather than a value or a node's label. */
  function isSize(v: string | null): boolean {
    return v !== null && /^(\{\d+\}|\[\d+\])$/.test(v);
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="wrap">
  <div class="bar">
    <span class="name mono" title={title}>{title}</span>
    {#if data?.class}<span class="cls">{data.class}</span>{/if}
    {#if data}
      <span class="dim"
        >{data.changes === 0 ? "no differences" : `${data.changes} change${data.changes === 1 ? "" : "s"}`}{data.rawChanges
          ? ` · ${data.rawChanges} in all data`
          : ""}</span
      >
    {/if}
    <span class="grow"></span>
    <input class="filter" placeholder="Filter properties…" bind:value={filter} spellcheck="false" />
    <label class="opt"
      ><input type="checkbox" checked={onlyChanges} onchange={(e) => setOnlyChanges(e.currentTarget.checked)} />Changes
      only</label
    >
    <button onclick={() => setAll(true)} title="Open every branch">Expand all</button>
    <button onclick={() => setAll(false)} title="Close every branch">Collapse all</button>
    <button onclick={openInUnreal} title="Diff the same two files in Unreal's own asset diff">Open in Unreal</button>
  </div>
  {#if notice}<div class="note">{notice}</div>{/if}
  {#if data?.leftError}<div class="err">{leftLabel}: {data.leftError}</div>{/if}
  {#if data?.rightError}<div class="err">{rightLabel}: {data.rightError}</div>{/if}

  {#if error}
    <div class="err mono">{error}</div>
  {:else if loading}
    <div class="pad dim">Reading both revisions…</div>
  {:else}
    {#if graph}
      <div class="gsec" style={graphOpen ? `height:${graphH}px` : ""}>
        <div class="gbar">
          <button class="fold" onclick={() => ((graphOpen = !graphOpen), (fitted = ""))} title={graphOpen ? "Hide the graph" : "Show the graph"}
            >{graphOpen ? "▾" : "▸"}</button
          >
          <select value={graphName} onchange={(e) => pickGraph(e.currentTarget.value)}>
            {#each data?.graphs ?? [] as g (g.name)}
              <option value={g.name}>{g.name}{g.changes ? ` (${g.changes})` : ""}</option>
            {/each}
          </select>
          {#if graphOpen}
            <button onclick={() => nextChange(-1)} disabled={!changed.length} title="Previous changed node">◀</button>
            <button onclick={() => nextChange(1)} disabled={!changed.length} title="Next changed node">Next change ▶</button>
            <span class="dim"
              >{changed.length
                ? `${changeAt >= 0 ? changeAt + 1 : "–"} / ${changed.length} changed node${changed.length === 1 ? "" : "s"}`
                : "no node changed"}</span
            >
            <span class="grow"></span>
            <span class="legend"
              ><i class="lg-added"></i>added <i class="lg-removed"></i>removed <i class="lg-changed"></i>changed
              <i class="lg-moved"></i>moved</span
            >
            <label class="opt"><input type="checkbox" bind:checked={dimSame} />Dim unchanged</label>
            <button onclick={fit} title="Show the whole graph">Fit</button>
          {/if}
        </div>
        {#if graphOpen}
          <div class="garea" bind:clientWidth={areaW} bind:clientHeight={areaH}>
            {#if graphOnly !== "right"}
              <AssetGraph
                side={graph.left}
                title={leftLabel}
                bind:view
                selected={selectedNode}
                dim={dimSame}
                onselect={(id) => void selectNode(id)}
              />
            {/if}
            {#if graphOnly !== "left"}
              <AssetGraph
                side={graph.right}
                title={rightLabel}
                bind:view
                selected={selectedNode}
                dim={dimSame}
                onselect={(id) => void selectNode(id)}
              />
            {/if}
          </div>
        {/if}
      </div>
      {#if graphOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="split" onpointerdown={splitDown} title="Drag to resize"></div>
      {/if}
    {/if}
    <div class="head" class:single={only}>
      <span>Property</span>
      {#if only !== "right"}
        <span class="side">
          {#if data?.leftThumb}<img src={data.leftThumb} alt="" />{/if}{leftLabel}
        </span>
      {/if}
      {#if only !== "left"}
        <span class="side">
          {#if data?.rightThumb}<img src={data.rightThumb} alt="" />{/if}{rightLabel}
        </span>
      {/if}
    </div>
    <div class="scroll" bind:this={scrollEl} bind:clientHeight={viewH} onscroll={() => (scrollTop = scrollEl?.scrollTop ?? 0)}>
      {#if !visible.length}
        <div class="pad dim">
          {filter ? "Nothing matches the filter." : onlyChanges ? "The two revisions hold the same data." : "Empty."}
        </div>
      {/if}
      <div class="rows" style="height:{visible.length * RH}px">
        {#each visible.slice(first, last) as i, k (i)}
          {@const r = rows[i]}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="row s-{r.status}"
            class:single={only}
            class:sel={i === selected}
            class:diffval={r.status === "changed" && r.left !== r.right && !isSize(r.left)}
            style="top:{(first + k) * RH}px"
            onclick={() => (selected = i)}
            ondblclick={() => r.kids && toggle(i)}
          >
            <span class="key" style="padding-left:{6 + r.depth * 16}px" title={pathOf(i)}>
              {#if r.kids}
                <button class="tw" onclick={(e) => (e.stopPropagation(), toggle(i))}>{isOpen(i) ? "▾" : "▸"}</button>
              {:else}<span class="tw"></span>{/if}<span class="mk">{MARK[r.status]}</span>{r.key}
            </span>
            {#if only !== "right"}
              <span class="val l" class:muted={isSize(r.left)} title={r.left ?? ""}>{r.left ?? ""}</span>
            {/if}
            {#if only !== "left"}
              <span class="val r" class:muted={isSize(r.right)} title={r.right ?? ""}>{r.right ?? ""}</span>
            {/if}
          </div>
        {/each}
      </div>
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
    font-weight: 600;
  }
  .cls {
    flex: none;
    font-size: 11px;
    padding: 0 6px;
    border-radius: 8px;
    border: 1px solid var(--border);
  }
  .grow {
    flex: 1;
  }
  .filter {
    width: 200px;
    font-size: 12px;
  }
  .opt {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
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
  .head,
  .row {
    display: grid;
    grid-template-columns: minmax(220px, 34%) 1fr 1fr;
  }
  .head.single,
  .row.single {
    grid-template-columns: minmax(220px, 34%) 1fr;
  }
  .head {
    flex: none;
    font-size: 12px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .head > span {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .head img {
    height: 48px;
    width: 48px;
    object-fit: contain;
    border-radius: 3px;
  }
  .scroll {
    flex: 1;
    overflow: auto;
  }
  .rows {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 22px;
    line-height: 22px;
    font-family: var(--mono);
    font-size: 12px;
    border-left: 3px solid transparent;
    cursor: default;
  }
  .row.sel {
    outline: 1px solid color-mix(in srgb, var(--accent, #5a96e6) 70%, transparent);
    outline-offset: -1px;
  }
  .row > span {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .val {
    padding: 0 8px;
    border-left: 1px solid var(--border);
  }
  .muted {
    opacity: 0.5;
  }
  .tw {
    display: inline-block;
    width: 14px;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font-size: 10px;
    cursor: pointer;
  }
  .mk {
    display: inline-block;
    width: 12px;
    font-weight: 700;
  }
  .gbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .legend {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    opacity: 0.8;
  }
  .legend i {
    display: inline-block;
    width: 12px;
    height: 8px;
    margin-left: 6px;
    border-radius: 2px;
  }
  .lg-added {
    background: #5faf5f;
  }
  .lg-removed {
    background: #d9873a;
  }
  .lg-changed {
    background: #5a96e6;
  }
  .lg-moved {
    border: 1px dashed #9aa0aa;
  }
  .garea {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .gsec {
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .split {
    flex: none;
    height: 5px;
    cursor: row-resize;
    background: var(--border);
  }
  .split:hover {
    background: #5a96e6;
  }
  .fold {
    padding: 0 6px;
  }
  .s-changed {
    background: rgba(90, 150, 230, 0.1);
    border-left-color: #5a96e6;
  }
  .s-changed .mk {
    color: #7aaef0;
  }
  .s-changed.diffval .l {
    background: rgba(217, 135, 58, 0.16);
  }
  .s-changed.diffval .r {
    background: rgba(108, 195, 108, 0.17);
  }
  .s-added {
    background: rgba(108, 195, 108, 0.12);
    border-left-color: #5faf5f;
  }
  .s-added .mk {
    color: #7cc47c;
  }
  .s-removed {
    background: rgba(217, 135, 58, 0.12);
    border-left-color: #d9873a;
  }
  .s-removed .mk {
    color: #d9873a;
  }
</style>
