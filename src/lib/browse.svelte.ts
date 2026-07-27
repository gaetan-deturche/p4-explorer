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

type Tab = "history" | "pending" | "streams" | "log";
/** The file browser's data source: on-disk files, the workspace stream (server),
 *  or the whole depot (server, all depots from //). */
export type BrowseSource = "local" | "workspace" | "depot";
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
let source = $state<BrowseSource>("workspace"); // Files pane data source
let refreshing = $state(false);
let indexing = $state(false);
let indexCount = $state(0);
let streamRows = $state<P4Record[]>([]);
let streamsLoading = $state(false);

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

/** The root tree node for the current source: the `//` depots root for `depot`,
 *  else the workspace stream root (local & workspace share it; only the child
 *  listing differs). */
function rootForSource(): TreeNode {
  if (source === "depot") {
    return {
      path: "//",
      name: "Depots",
      isDir: true,
      expanded: true,
      loaded: false,
      loading: false,
      children: [],
    };
  }
  const n = makeNode(rootPath, true);
  n.expanded = true;
  return n;
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
  get source() {
    return source;
  },
  /** Switch the Files pane between on-disk / workspace-server / whole-depot and
   *  rebuild its tree. Depot needs no open workspace; the others need `rootPath`. */
  async setSource(s: BrowseSource) {
    if (!h || s === source) return;
    source = s;
    selectedTreePath = "";
    if (s !== "depot" && !rootPath) {
      tree = null;
      return;
    }
    tree = rootForSource();
    await browse.loadNode(tree);
  },

  /** Clear all browse state (on disconnect / workspace switch). */
  reset() {
    folderCache.clear();
    rootPath = "";
    clientRoot = "";
    tree = null;
    selectedTreePath = "";
    source = "workspace";
    history.reset();
    pending.clear();
    streamRows = [];
  },

  /** Point the browser at a workspace stream and load its data, restoring the
   *  workspace's saved view (tab + selection) if any; otherwise land on the
   *  stream-root folder history under `fallbackTab`. A saved path from a
   *  different stream (e.g. after a stream switch) is ignored. */
  async openWorkspace(stream: string, root: string, view: ViewState | null, fallbackTab: Tab) {
    if (!h) return;
    clientRoot = root;
    rootPath = stream;
    source = "workspace"; // entering a workspace lands on its stream (server) view
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

    // A saved "repo" tab no longer exists (Depot is a Files-pane source now).
    const valid: Tab[] = ["history", "pending", "streams", "log"];
    const tab = valid.includes(view?.tab as Tab) ? (view!.tab as Tab) : fallbackTab;
    if (tab === "streams") browse.loadStreams();
    browse.ensureIndex(); // background: build the fuzzy-search index if new
    h.setTab(tab);
    await browse.loadNode(tree); // `tree` is the reactive proxy — mutate through it
  },

  // --- file tree -------------------------------------------------------------
  // Populate a directory node's children. Dispatches on the current source:
  // depot (server, all depots), local (on-disk listing), or workspace (server
  // stream with stale-while-revalidate caching).
  async loadNode(node: TreeNode) {
    if (!h || node.loading) return;
    node.loading = true;
    const path = node.path;

    if (source === "depot") {
      if (path === "//") {
        const depots = await safe(() => p4.depots(h!.conn()));
        node.children = depots.filter((d) => d.name).map((d) => makeNode("//" + d.name, true));
      } else {
        const [d, f] = await Promise.all([
          safe(() => p4.dirs(h!.conn(), path)),
          safe(() => p4.files(h!.conn(), path)),
        ]);
        node.children = buildChildren({ dirs: d, files: f });
      }
      node.loaded = true;
      node.loading = false;
      return;
    }

    if (source === "local") {
      const local = await localChildren(clientRoot, rootPath, path);
      node.children = local ? buildChildren(local) : [];
      node.loaded = true;
      node.loading = false;
      return;
    }

    // workspace (server stream)
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

  // Single click: select. File → its server history/details (works for tracked
  // files in every source). Folder → its history in workspace/local; in the
  // depot source (no folder-history subject) it just folds/unfolds.
  selectNode(node: TreeNode) {
    selectedTreePath = node.path;
    if (node.isDir && source === "depot") {
      browse.expandNode(node);
      return;
    }
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
