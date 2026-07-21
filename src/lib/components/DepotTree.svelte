<script lang="ts">
  import type { P4Record } from "$lib/p4";
  import type { TreeNode } from "$lib/tree";

  let {
    root,
    selectedPath,
    indexing = false,
    onSelect,
    onExpand,
    onSearch,
    onOpenResult,
  }: {
    root: TreeNode | null;
    selectedPath: string;
    indexing?: boolean;
    onSelect: (node: TreeNode) => void; // single click: dir → history, file → details
    onExpand: (node: TreeNode) => void; // triangle / double-click: toggle + load
    onSearch?: (term: string) => Promise<P4Record[]>; // fuzzy index search (optional)
    onOpenResult?: (depotFile: string) => void; // click a search result
  } = $props();

  let query = $state("");
  let results = $state<P4Record[] | null>(null); // null = show tree
  let searching = $state(false);
  let debounce: number | null = null;
  let seq = 0; // drop out-of-order search responses

  // Collapsed folders in the result tree (default = expanded).
  let collapsed = $state<Record<string, boolean>>({});
  function toggleCollapse(path: string) {
    collapsed[path] = !collapsed[path];
  }

  function onInput() {
    if (debounce !== null) clearTimeout(debounce);
    const term = query.trim();
    if (!term) {
      seq++; // invalidate any in-flight search so a late response can't repopulate
      results = null;
      searching = false;
      return;
    }
    searching = true;
    debounce = window.setTimeout(() => runSearch(term), 90);
  }

  async function runSearch(term: string) {
    if (!onSearch) return;
    const mine = ++seq;
    const r = await onSearch(term);
    if (mine !== seq) return; // superseded by a newer keystroke / cleared
    results = r;
    searching = false;
  }

  function clearSearch() {
    seq++; // drop any in-flight search
    query = "";
    results = null;
    searching = false;
    collapsed = {};
  }

  // Build a folder hierarchy from the flat search-result paths, rooted at the
  // workspace root, so results keep their directory structure.
  type RNode = { name: string; path: string; isDir: boolean; children: RNode[] };
  function sortTree(n: RNode) {
    n.children.sort((a, b) =>
      a.isDir === b.isDir ? a.name.localeCompare(b.name) : a.isDir ? -1 : 1,
    );
    n.children.forEach(sortTree);
  }
  const resultTree = $derived.by<RNode | null>(() => {
    if (!results) return null;
    const base = root?.path ?? "";
    const rootNode: RNode = { name: base, path: base, isDir: true, children: [] };
    for (const rec of results) {
      const p = rec.depotFile;
      if (!p) continue;
      const rel = base && p.startsWith(base + "/") ? p.slice(base.length + 1) : p.replace(/^\/+/, "");
      const segs = rel.split("/");
      let cur = rootNode;
      let acc = base;
      for (let i = 0; i < segs.length; i++) {
        acc = acc + "/" + segs[i];
        const isDir = i < segs.length - 1;
        let child = cur.children.find((c) => c.name === segs[i] && c.isDir === isDir);
        if (!child) {
          child = { name: segs[i], path: acc, isDir, children: [] };
          cur.children.push(child);
        }
        cur = child;
      }
    }
    sortTree(rootNode);
    return rootNode;
  });

  function sync(rec?: P4Record): { cls: string; label: string; title: string } {
    if (!rec) return { cls: "prov", label: "", title: "local (not yet confirmed)" };
    const have = rec.haveRev;
    const head = rec.headRev ?? "";
    if (!have) return { cls: "nosync", label: "", title: "not synced" };
    if (have === head) return { cls: "synced", label: "●", title: `synced #${have}` };
    return { cls: "stale", label: `#${have}/${head}`, title: `have #${have}, head #${head}` };
  }
</script>

<div class="panel">
  {#if onSearch}
    <div class="search">
      <input
        placeholder={indexing ? "Building index…" : "Search files (fuzzy)"}
        bind:value={query}
        oninput={onInput}
        spellcheck="false"
      />
      {#if query}
        <button class="clear" title="Clear" onclick={clearSearch}>✕</button>
      {/if}
    </div>
  {/if}

  <div class="scroll body">
    {#if results !== null || query.trim()}
      <!-- Search mode -->
      {#if indexing && results === null}
        <div class="msg dim">Building search index…</div>
      {:else if searching && results === null}
        <div class="msg dim">Searching…</div>
      {:else if results && results.length === 0}
        <div class="msg dim">No files matching “{query.trim()}”.</div>
      {:else if results}
        <div class="reshdr dim">
          {results.length} result{results.length === 1 ? "" : "s"}{results.length >= 200 ? "+" : ""}
        </div>
        {#if resultTree}
          {@render resultNodes(resultTree, 0)}
        {/if}
      {/if}
    {:else if !root}
      <div class="msg dim">Select a workspace to browse.</div>
    {:else}
      {@render nodeRow(root, 0)}
    {/if}
  </div>
</div>

{#snippet resultNodes(node: RNode, depth: number)}
  {#each node.children as c (c.path)}
    {@const open = !collapsed[c.path]}
    <div class="row" class:selected={!c.isDir && c.path === selectedPath} style="padding-left:{depth * 14 + 4}px">
      {#if c.isDir}
        <button class="tw" title="Expand / collapse" onclick={() => toggleCollapse(c.path)}>
          {open ? "▾" : "▸"}
        </button>
        <button class="main mono" onclick={() => toggleCollapse(c.path)}>
          <span class="ic">📁</span><span class="name">{c.name}</span>
        </button>
      {:else}
        <span class="tw-sp"></span>
        <button class="main mono" title={c.path} onclick={() => onOpenResult?.(c.path)}>
          <span class="ic">📄</span><span class="name">{c.name}</span>
        </button>
      {/if}
    </div>
    {#if c.isDir && open}
      {@render resultNodes(c, depth + 1)}
    {/if}
  {/each}
{/snippet}

{#snippet nodeRow(node: TreeNode, depth: number)}
  <div class="row" class:selected={node.path === selectedPath} style="padding-left:{depth * 14 + 4}px">
    {#if node.isDir}
      <button class="tw" title="Expand / collapse" onclick={() => onExpand(node)}>
        {node.expanded ? "▾" : "▸"}
      </button>
    {:else}
      <span class="tw-sp"></span>
    {/if}
    <button
      class="main mono"
      onclick={() => onSelect(node)}
      ondblclick={() => node.isDir && onExpand(node)}
    >
      <span class="ic">{node.isDir ? "📁" : "📄"}</span>
      <span class="name">{node.name}</span>
      {#if node.loading}<span class="dim sp">…</span>{/if}
      {#if !node.isDir}
        {@const s = sync(node.rec)}
        <span class="sync {s.cls}" title={s.title}>{s.label}</span>
      {/if}
    </button>
  </div>
  {#if node.isDir && node.expanded}
    {#if node.loaded && node.children.length === 0 && !node.loading}
      <div class="empty dim" style="padding-left:{(depth + 1) * 14 + 24}px">empty</div>
    {:else}
      {#each node.children as child (child.path)}
        {@render nodeRow(child, depth + 1)}
      {/each}
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
  .search {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  .search input {
    flex: 1;
    min-width: 0;
    font-size: 12px;
  }
  .clear {
    border: none;
    background: none;
    border-radius: 4px;
    padding: 2px 6px;
    color: var(--text-dim);
    cursor: pointer;
  }
  .clear:hover {
    background: var(--bg-hover);
  }
  .body {
    flex: 1;
    padding: 4px 0;
  }
  .row {
    display: flex;
    align-items: center;
  }
  .row.selected {
    background: var(--bg-sel);
  }
  .row:hover:not(.selected) {
    background: var(--bg-hover);
  }
  .tw,
  .tw-sp {
    width: 16px;
    flex: none;
  }
  .tw {
    border: none;
    background: none;
    border-radius: 0;
    padding: 0;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 11px;
    line-height: 1;
  }
  .tw:hover {
    color: var(--text);
  }
  .main {
    display: flex;
    align-items: center;
    gap: 5px;
    flex: 1;
    min-width: 0;
    text-align: left;
    border: none;
    background: none;
    border-radius: 0;
    padding: 2px 10px 2px 0;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
    cursor: pointer;
  }
  .ic {
    flex: none;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sp {
    flex: none;
  }
  .sync {
    flex: none;
    font-size: 10px;
  }
  .sync.synced {
    color: var(--have);
  }
  .sync.stale {
    color: #d08a1d;
  }
  .empty {
    font-size: 11px;
    font-style: italic;
    padding-top: 1px;
    padding-bottom: 2px;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
  .reshdr {
    padding: 4px 10px;
    font-size: 11px;
    border-bottom: 1px solid var(--border);
  }
</style>
