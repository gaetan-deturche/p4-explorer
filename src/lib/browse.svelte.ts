//! Browse feature store: the left depot tree (stale-while-revalidate folder
//! cache + fuzzy search index), the Streams and Repo browser tabs, and the
//! workspace refresh. History/details loads are delegated to the history store.
//! Shared bits (conn, connected, center tab) come via init().

import { p4, idx, type P4Conn, type P4Record } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";
import {
  loadFolder,
  saveFolder,
  clearClientCache,
  buildChildren,
  localChildren,
  type FolderContents,
} from "$lib/cache";
import { history } from "$lib/history.svelte";
import { pending } from "$lib/pending.svelte";
import type { ViewState } from "$lib/nav";

type Tab = "history" | "pending" | "streams" | "repo" | "log";
type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  getTab: () => Tab;
  setTab: (t: Tab) => void;
};

let h: Hooks | null = null;
const folderCache = new Map<string, FolderContents>();

let rootPath = $state(""); // stream root, e.g. //Curiosity/main
let clientRoot = $state(""); // local workspace root, e.g. H:\Dev\...\Curiosity
let tree = $state<TreeNode | null>(null);
let selectedTreePath = $state("");
let refreshing = $state(false);
let indexing = $state(false);
let indexCount = $state(0);
let streamRows = $state<P4Record[]>([]);
let streamsLoading = $state(false);
let repoTree = $state<TreeNode | null>(null);
let repoSelected = $state("");

async function safe<T>(fn: () => Promise<T[]>): Promise<T[]> {
  try {
    return await fn();
  } catch {
    return [];
  }
}

// p4-query prefix (client syntax, e.g. //<client>) used for dirs/files/history
// instead of the stream depot path, so a VIRTUAL stream resolves through the
// workspace view. Display paths stay in stream-depot form (rebuilt from the
// parent path + basename), so the tree looks the same.
let queryRoot = "";
function base(p: string): string {
  return p.slice(p.lastIndexOf("/") + 1);
}

export const browse = {
  init(hooks: Hooks) {
    h = hooks;
  },
  /** Translate a display (stream-depot) path to the p4-query path (client
   *  syntax). Identity for paths outside the current stream root. */
  toQuery(path: string): string {
    return rootPath && queryRoot && path.startsWith(rootPath)
      ? queryRoot + path.slice(rootPath.length)
      : path;
  },
  get rootPath() {
    return rootPath;
  },
  get clientRoot() {
    return clientRoot;
  },
  get tree() {
    return tree;
  },
  get selectedTreePath() {
    return selectedTreePath;
  },
  get refreshing() {
    return refreshing;
  },
  get indexing() {
    return indexing;
  },
  get streamRows() {
    return streamRows;
  },
  get streamsLoading() {
    return streamsLoading;
  },
  get repoTree() {
    return repoTree;
  },
  get repoSelected() {
    return repoSelected;
  },

  /** Clear all browse state (on disconnect / workspace switch). */
  reset() {
    folderCache.clear();
    rootPath = "";
    clientRoot = "";
    tree = null;
    selectedTreePath = "";
    history.reset();
    pending.clear();
    streamRows = [];
    repoTree = null;
    repoSelected = "";
  },

  /** Point the browser at a workspace stream and load its data, restoring the
   *  workspace's saved view (tab + selection) if any; otherwise land on the
   *  stream-root folder history under `fallbackTab`. A saved path from a
   *  different stream (e.g. after a stream switch) is ignored. */
  async openWorkspace(stream: string, root: string, view: ViewState | null, fallbackTab: Tab) {
    if (!h) return;
    clientRoot = root;
    rootPath = stream;
    queryRoot = "//" + h.conn().client; // browse through the client view
    tree = makeNode(stream, true);
    tree.expanded = true;

    const savedPath = view?.treePath && view.treePath.startsWith(stream) ? view.treePath : "";
    if (savedPath) {
      selectedTreePath = savedPath;
      if (view!.histMode === "file") history.selectFile(savedPath);
      else history.loadFolder(savedPath);
    } else {
      selectedTreePath = stream;
      history.loadFolder(stream);
    }
    pending.load();

    const tab = view?.tab ?? fallbackTab;
    if (tab === "streams") browse.loadStreams();
    else if (tab === "repo") browse.openRepo();
    browse.ensureIndex(); // background: build the fuzzy-search index if new
    h.setTab(tab);
    await browse.loadNode(tree); // `tree` is the reactive proxy — mutate through it
  },

  // --- depot tree ------------------------------------------------------------
  // Populate a directory node's children: instant from cache/local disk, then
  // the authoritative p4 listing replaces it (stale-while-revalidate).
  async loadNode(node: TreeNode) {
    if (!h || node.loading) return;
    node.loading = true;
    const path = node.path;

    const mem = folderCache.get(path);
    const cached = mem ?? loadFolder(h.conn().client, path);
    if (cached) {
      node.children = buildChildren(cached);
    } else {
      const local = await localChildren(clientRoot, rootPath, path);
      if (local && node.children.length === 0) node.children = buildChildren(local);
    }

    // Refresh unless we already have it fresh in memory this session. Query via
    // the client view, but rebuild each child's path from the display parent +
    // basename so the tree stays in stream-depot form (and virtual streams show).
    if (!mem) {
      const q = browse.toQuery(path);
      const [d, f] = await Promise.all([
        safe(() => p4.dirs(h!.conn(), q)),
        safe(() => p4.files(h!.conn(), q)),
      ]);
      const c = {
        dirs: d.map((r) => (r.dir ? { ...r, dir: `${path}/${base(r.dir)}` } : r)),
        files: f.map((r) => (r.depotFile ? { ...r, depotFile: `${path}/${base(r.depotFile)}` } : r)),
      };
      node.children = buildChildren(c);
      folderCache.set(path, c);
      saveFolder(h.conn().client, path, c);
    }
    node.loaded = true;
    node.loading = false;
  },

  // Single click: select (dir → history, file → details) — does NOT fold.
  selectNode(node: TreeNode) {
    selectedTreePath = node.path;
    h?.setTab("history"); // explicit user navigation → show History
    if (node.isDir) history.loadFolder(node.path);
    else history.selectFile(node.path);
  },

  // Triangle / double click: toggle fold state, loading children on first open.
  expandNode(node: TreeNode) {
    node.expanded = !node.expanded;
    if (node.expanded && !node.loaded) browse.loadNode(node);
  },

  // Re-fetch an expanded node's children, preserving which descendants were open.
  async reloadNode(node: TreeNode) {
    if (!node.isDir || !node.expanded) return;
    const openPaths = new Set(node.children.filter((c) => c.isDir && c.expanded).map((c) => c.path));
    node.loaded = false;
    await browse.loadNode(node);
    for (const child of node.children) {
      if (openPaths.has(child.path)) {
        child.expanded = true;
        await browse.reloadNode(child);
      }
    }
  },

  // --- fuzzy search index ----------------------------------------------------
  // Build (or rebuild) the local fuzzy-search index for the current workspace.
  async buildIndex() {
    if (!h || !h.connected() || !h.conn().client || !rootPath || indexing) return;
    indexing = true;
    try {
      indexCount = await idx.build(h.conn(), h.conn().client, rootPath);
    } catch {
      /* leave count as-is */
    } finally {
      indexing = false;
    }
  },
  // Ensure an index exists (build in the background if this workspace is new).
  async ensureIndex() {
    if (!h || !h.connected() || !h.conn().client) return;
    try {
      indexCount = await idx.status(h.conn().client);
    } catch {
      indexCount = 0;
    }
    if (indexCount === 0) browse.buildIndex();
  },
  // Per-keystroke fuzzy search over the local index (case-insensitive, no p4).
  async searchDepot(term: string): Promise<P4Record[]> {
    if (!h || !term.trim() || !h.conn().client) return [];
    const paths = await idx.search(h.conn().client, term.trim(), 200);
    return paths.map((p) => ({ depotFile: p }) as P4Record);
  },
  openResult(depotFile: string) {
    selectedTreePath = depotFile;
    h?.setTab("history"); // explicit user navigation → show History
    history.selectFile(depotFile);
  },

  // --- Streams tab -----------------------------------------------------------
  async loadStreams() {
    if (!h) return;
    h.setTab("streams");
    if (!h.connected()) return;
    if (streamRows.length === 0) streamsLoading = true;
    const rows = await safe(() => p4.streams(h!.conn()));
    streamsLoading = false;
    streamRows = rows;
  },

  // --- Repo (all-depots) browser tab -----------------------------------------
  openRepo() {
    if (!h) return;
    h.setTab("repo");
    if (!h.connected()) return;
    if (!repoTree) {
      repoTree = {
        path: "//",
        name: "Depots",
        isDir: true,
        expanded: true,
        loaded: false,
        loading: false,
        children: [],
      };
      browse.loadRepoNode(repoTree);
    }
  },
  async loadRepoNode(node: TreeNode) {
    if (!h || node.loading) return;
    node.loading = true;
    if (node.path === "//") {
      const depots = await safe(() => p4.depots(h!.conn()));
      node.children = depots.filter((d) => d.name).map((d) => makeNode("//" + d.name, true));
    } else {
      const [d, f] = await Promise.all([
        safe(() => p4.dirs(h!.conn(), node.path)),
        safe(() => p4.files(h!.conn(), node.path)),
      ]);
      node.children = buildChildren({ dirs: d, files: f });
    }
    node.loaded = true;
    node.loading = false;
  },
  repoExpand(node: TreeNode) {
    node.expanded = !node.expanded;
    if (node.expanded && !node.loaded) browse.loadRepoNode(node);
  },
  repoSelect(node: TreeNode) {
    if (!h) return;
    repoSelected = node.path;
    if (node.isDir) browse.repoExpand(node);
    else {
      history.selectFile(node.path);
      h.setTab("history"); // jump to History showing this file's revisions
    }
  },

  // --- refresh ---------------------------------------------------------------
  async refresh() {
    if (!h || !h.connected() || refreshing) return;
    refreshing = true;
    try {
      folderCache.clear();
      history.clearMemCache();
      clearClientCache(h.conn().client);
      if (tree) {
        tree.expanded = true;
        await browse.reloadNode(tree);
      }
      if (selectedTreePath && history.mode === "file") await history.selectFile(selectedTreePath);
      else if (selectedTreePath) await history.loadFolder(selectedTreePath);
      browse.buildIndex(); // rebuild the fuzzy-search index in the background
    } finally {
      refreshing = false;
    }
  },
};
