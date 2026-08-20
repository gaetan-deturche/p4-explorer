<script lang="ts">
  import type { SearchHits } from "$lib/p4";
  import type { TreeNode } from "$lib/tree";

  let {
    root,
    selectedPath,
    indexing = false,
    onSelect,
    onExpand,
    onSearch,
    onOpenResult,
    onContext,
  }: {
    root: TreeNode | null;
    selectedPath: string;
    indexing?: boolean;
    onSelect: (node: TreeNode) => void; // single click: dir → history, file → details
    onExpand: (node: TreeNode) => void; // triangle / double-click: toggle + load
    // Index search: literal matches filter the view, fuzzy ones are suggestions.
    onSearch?: (term: string) => Promise<SearchHits>;
    onOpenResult?: (depotFile: string) => void; // click a search result
    // right-click a node → (node, event, selected nodes incl. this one)
    onContext?: (node: TreeNode, e: MouseEvent, selection: TreeNode[]) => void;
  } = $props();

  // Multi-selection (Ctrl/Shift click), keyed by depot path. A plain click still
  // navigates (onSelect) and resets the selection to that node, so the common
  // case is unchanged; Ctrl/Shift only build a set for the context actions.
  let selected = $state<Set<string>>(new Set());
  let anchor: string | null = null;
  /** Visible rows in render order — the range Shift+click spans. */
  const visibleNodes = $derived.by(() => {
    const out: TreeNode[] = [];
    const walk = (n: TreeNode) => {
      out.push(n);
      if (n.isDir && n.expanded) for (const c of n.children) walk(c);
    };
    if (root) walk(root);
    return out;
  });
  function clickNode(node: TreeNode, e: MouseEvent) {
    if (e.shiftKey && anchor) {
      const paths = visibleNodes.map((n) => n.path);
      const a = paths.indexOf(anchor);
      const b = paths.indexOf(node.path);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        selected = new Set(paths.slice(lo, hi + 1));
        return; // range select only — don't navigate
      }
    }
    if (e.ctrlKey || e.metaKey) {
      const n = new Set(selected);
      if (n.has(node.path)) n.delete(node.path);
      else n.add(node.path);
      selected = n;
      anchor = node.path;
      return; // toggle only — don't navigate
    }
    selected = new Set([node.path]);
    anchor = node.path;
    onSelect(node);
  }
  /** The nodes the context menu acts on: the selection when the clicked node is
   *  part of it, else just that node (which becomes the new selection). */
  function contextSelection(node: TreeNode): TreeNode[] {
    if (!selected.has(node.path)) {
      selected = new Set([node.path]);
      anchor = node.path;
      return [node];
    }
    return visibleNodes.filter((n) => selected.has(n.path));
  }

  let query = $state("");
  let hits = $state<SearchHits | null>(null); // null = show the tree
  let suggestOpen = $state(false); // fuzzy droplist under the search box
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
      hits = null;
      suggestOpen = false;
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
    hits = r;
    suggestOpen = true;
    sugSel = -1;
    searching = false;
  }

  // Suggestions are the RANKED best matches — the closest file names first, so a
  // real hit (a file actually called what you typed) leads the list. They are not
  // filtered against the view's literal matches: doing that removed exactly the
  // matches worth suggesting and left the low-scoring tail.
  let sugSel = $state(-1);
  const suggestions = $derived.by(() => {
    // Never show a previous query's answer while a newer search is still running,
    // and nothing for one or two characters (any ranking there is noise).
    if (!hits || searching || query.trim().length < 3) return [];
    return hits.fuzzy.slice(0, 10);
  });

  function openSuggestion(p: string) {
    suggestOpen = false;
    onOpenResult?.(p);
  }
  function onSearchKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      if (suggestOpen) suggestOpen = false;
      else clearSearch();
      return;
    }
    if (!suggestOpen || !suggestions.length) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      sugSel = (sugSel + 1) % suggestions.length;
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      sugSel = (sugSel - 1 + suggestions.length) % suggestions.length;
    } else if (e.key === "Enter" && sugSel >= 0) {
      e.preventDefault();
      openSuggestion(suggestions[sugSel]);
    }
  }

  function clearSearch() {
    seq++; // drop any in-flight search
    query = "";
    hits = null;
    suggestOpen = false;
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
    if (!hits) return null;
    const base = root?.path ?? "";
    const rootNode: RNode = { name: base, path: base, isDir: true, children: [] };
    for (const p of hits.contains) {
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

  // File sync marker. Tooltip is changelist-based (synced CL, or have/head CL
  // when behind); the have CL fills in asynchronously for stale files.
  function sync(node: TreeNode): { cls: string; label: string; title: string } {
    const rec = node.rec;
    if (!rec) return { cls: "prov", label: "", title: "local (not yet confirmed)" };
    const have = rec.haveRev;
    const head = rec.headRev ?? "";
    if (!have) return { cls: "nosync", label: "", title: "not synced" };
    if (have === head) {
      return { cls: "synced", label: "●", title: node.headCl ? `synced (CL ${node.headCl})` : "synced" };
    }
    const title = node.headCl
      ? `have CL ${node.haveCl ?? "…"} / head CL ${node.headCl}`
      : `have #${have}, head #${head}`;
    return { cls: "stale", label: `#${have}/${head}`, title };
  }
  // Folder sync marker: have-change vs head-change under the folder.
  function folderSync(node: TreeNode): { cls: string; label: string; title: string } {
    const s = node.folderSync;
    if (s === "synced") {
      return { cls: "synced", label: "●", title: node.headCl ? `synced (CL ${node.headCl})` : "all files synced" };
    }
    if (s === "stale") {
      const title = node.headCl
        ? `have CL ${node.haveCl ?? "…"} / head CL ${node.headCl}`
        : "some files behind head";
      return { cls: "stale", label: "●", title };
    }
    return { cls: "nosync", label: "", title: "" };
  }
</script>

<div class="panel">
  {#if onSearch}
    <div class="search">
      <input
        data-role="search"
        placeholder={indexing ? "Building index…" : "Search files"}
        bind:value={query}
        oninput={onInput}
        onkeydown={onSearchKey}
        onblur={() => setTimeout(() => (suggestOpen = false), 120)}
        onfocus={() => hits && (suggestOpen = true)}
        spellcheck="false"
      />
      {#if query}
        <button class="clear" title="Clear" onclick={clearSearch}>✕</button>
      {/if}
      <!-- Fuzzy suggestions: near-misses the literal filter below won't show
           (↑/↓ to pick, Enter to open, Esc to dismiss). -->
      {#if suggestOpen && suggestions.length}
        <div class="suggest">
          {#each suggestions as p, i (p)}
            <button
              class="sug mono"
              class:sel={i === sugSel}
              title={p}
              onmousedown={(e) => {
                e.preventDefault(); // keep focus so blur can't close first
                openSuggestion(p);
              }}
            >
              <span class="sname">{p.split("/").pop()}</span>
              <span class="sdir dim">{p.slice(0, p.lastIndexOf("/") + 1)}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <div class="scroll body">
    {#if hits !== null || query.trim()}
      <!-- Search mode: the view lists LITERAL (case-insensitive substring)
           matches, so what you typed is what you see; fuzzy near-misses are
           offered in the droplist above instead. -->
      {#if indexing && hits === null}
        <div class="msg dim">Building search index…</div>
      {:else if searching && hits === null}
        <div class="msg dim">Searching…</div>
      {:else if hits && hits.contains.length === 0}
        <div class="msg dim">
          No file name contains “{query.trim()}”.
          {#if suggestions.length}Closest names are suggested above.{/if}
        </div>
      {:else if hits}
        <div class="reshdr dim">
          {hits.contains.length} match{hits.contains.length === 1 ? "" : "es"}{hits.contains.length >=
          200
            ? "+"
            : ""}
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
  <div
    class="row"
    class:selected={node.path === selectedPath}
    class:multisel={selected.size > 1 && selected.has(node.path)}
    style="padding-left:{depth * 14 + 4}px"
  >
    {#if node.isDir}
      <button class="tw" title="Expand / collapse" onclick={() => onExpand(node)}>
        {node.expanded ? "▾" : "▸"}
      </button>
    {:else}
      <span class="tw-sp"></span>
    {/if}
    <button
      class="main mono"
      class:untracked={node.untracked}
      class:deleted={node.deleted}
      title={node.untracked ? "Not in the depot (ignored / uncommitted)" : node.path}
      onclick={(e) => clickNode(node, e)}
      ondblclick={() => node.isDir && onExpand(node)}
      oncontextmenu={(e) => {
        if (onContext) {
          e.preventDefault();
          onContext(node, e, contextSelection(node));
        }
      }}
    >
      <span class="ic">{node.isDir ? "📁" : "📄"}</span>
      <span class="name">{node.name}</span>
      {#if node.loading}<span class="dim sp">…</span>{/if}
      {#if node.deleted}
        <!-- Not a sync state: it's gone from the depot, so no dot. The row's own
             tooltip stays the path (like any file); the explanation lives here. -->
        <span
          class="delmark"
          title="Deleted at head — no longer in the depot, so it can't be synced; its history is still browsable"
        >
          deleted
        </span>
      {:else if !node.isDir}
        {@const s = sync(node)}
        <span class="sync {s.cls}" title={s.title}>{s.label}</span>
      {:else if node.folderSync}
        {@const s = folderSync(node)}
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
    /* flex:1 (take the space left by the Files header + source buttons above),
       NOT height:100% — that ignored those siblings and overflowed the column,
       pushing the scroll body's bottom under the status bar. min-height:0 lets the
       .scroll body actually scroll instead of growing to its content. */
    flex: 1;
    min-height: 0;
    background: var(--bg-panel);
  }
  .search {
    position: relative; /* anchors the suggestion droplist */
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  /* Fuzzy suggestions, overlaid so opening them doesn't shift the file list. */
  .suggest {
    position: absolute;
    top: 100%;
    left: 6px;
    right: 6px;
    z-index: 20;
    max-height: 45vh;
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.4);
    padding: 3px;
    display: flex;
    flex-direction: column;
  }
  .sug {
    display: flex;
    align-items: baseline;
    gap: 6px;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 4px;
    padding: 3px 6px;
    font-size: 11px;
    color: var(--text);
    cursor: pointer;
    white-space: nowrap;
  }
  .sug:hover,
  .sug.sel {
    background: var(--bg-hover);
  }
  .sname {
    flex: none;
  }
  .sdir {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: right;
    font-size: 10px;
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
  /* Multi-selected rows (Ctrl/Shift click) — distinct from the navigated row. */
  .row.multisel {
    background: var(--bg-hover);
    outline: 1px solid var(--accent);
    outline-offset: -1px;
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
  .main.untracked {
    color: var(--text-dim);
    opacity: 0.65;
  }
  /* Deleted at head: struck through so it can't be mistaken for a live file. */
  .main.deleted .name {
    text-decoration: line-through;
  }
  .main.deleted {
    color: var(--text-dim);
    opacity: 0.7;
  }
  /* A state, not an error — dim like the struck-through name, not alarm-red. */
  .delmark {
    flex: none;
    font-size: 10px;
    font-style: italic;
    color: var(--text-dim);
  }
  .main.untracked .ic {
    opacity: 0.5;
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
