//! Browse feature store: the left depot tree (stale-while-revalidate folder
//! cache + fuzzy search index), the Streams and Repo browser tabs, and the
//! workspace refresh. History/details loads are delegated to the history store.
//! Shared bits (conn, connected, center tab) come via init().

import { p4, idx, type P4Conn, type P4Record } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";
import {
  loadFolder,
  loadFolderAsync,
  saveFolder,
  clearClientCache,
  buildChildren,
  localChildren,
  type FolderContents,
} from "$lib/cache";
import { history } from "$lib/history.svelte";
import { pending } from "$lib/pending.svelte";
import { cacheGetSync, cacheSet } from "$lib/store";
import { loadBrowseSource, saveBrowseSource, type ViewState } from "$lib/nav";

type Tab = "history" | "pending" | "streams" | "log" | "notes";
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
let source = $state<BrowseSource>(loadBrowseSource()); // Files pane data source (persisted)
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

// Cached stream list per server (store scope `p4:streams`, key = port).
function loadStreamsCache(port: string): P4Record[] {
  if (!port) return [];
  try {
    return JSON.parse(cacheGetSync("p4:streams", port) ?? "[]") as P4Record[];
  } catch {
    return [];
  }
}
function saveStreamsCache(port: string, rows: P4Record[]): void {
  if (port) cacheSet("p4:streams", port, JSON.stringify(rows));
}

// Folder sync markers: have-change vs head-change under a folder (same signal
// the History view shows). Computed lazily per folder, throttled so a wide
// listing doesn't spawn a p4 process per subfolder at once, and cached.
type FolderSync = { status: "synced" | "stale" | "nosync"; have: string; head: string };
const folderSyncCache = new Map<string, FolderSync>();
let fsActive = 0;
const fsWaiters: (() => void)[] = [];
async function withFolderSyncSlot<T>(fn: () => Promise<T>): Promise<T> {
  if (fsActive >= 6) await new Promise<void>((r) => fsWaiters.push(r));
  fsActive++;
  try {
    return await fn();
  } finally {
    fsActive--;
    fsWaiters.shift()?.();
  }
}
/** Fill in a directory node's sync marker + have/head changelists (mutate node). */
async function computeFolderSync(node: TreeNode): Promise<void> {
  if (!h) return;
  const cacheKey = node.path.toLowerCase();
  const hit = folderSyncCache.get(cacheKey);
  if (hit) {
    node.folderSync = hit.status;
    node.haveCl = hit.have;
    node.headCl = hit.head;
    return;
  }
  const conn = h.conn();
  const q = browse.toQuery(node.path);
  const res = await withFolderSyncSlot(async () => {
    const [head, have] = await Promise.all([
      safe(() => p4.changes(conn, q, 1)),
      safe(() => p4.haveChange(conn, q)),
    ]);
    const headCl = head[0]?.change ?? "";
    const haveCl = have[0]?.change ?? "";
    const status: "synced" | "stale" | "nosync" = !haveCl
      ? "nosync"
      : haveCl === headCl
        ? "synced"
        : "stale";
    return { status, have: haveCl, head: headCl };
  });
  folderSyncCache.set(cacheKey, res);
  node.folderSync = res.status;
  node.haveCl = res.have;
  node.headCl = res.head;
}

/** A stale file's synced changelist (its head CL is already known from fstat). */
async function computeFileHaveCl(node: TreeNode): Promise<void> {
  if (!h || node.haveCl) return;
  const conn = h.conn();
  const spec = browse.toQuery(node.path) + "#have";
  const rows = await withFolderSyncSlot(() => safe(() => p4.changesExact(conn, spec)));
  node.haveCl = rows[0]?.change ?? "";
}

/** Set a file node's have/head changelists for the sync tooltip: head from the
 *  fstat record; have == head when synced, else fetched in the background. */
function applyFileCl(node: TreeNode, rec: P4Record): void {
  const have = rec.haveRev;
  const head = rec.headRev ?? "";
  node.headCl = rec.headChange ?? "";
  if (!have) return; // not synced → no have CL
  if (have === head) node.haveCl = node.headCl; // synced → same changelist
  else computeFileHaveCl(node); // stale → fetch the synced (have) CL in the background
}

// Search-index keys, one namespace per source (see the index section below).
function wsKey(): string {
  return h!.conn().client;
}
function localKey(): string {
  return "local:" + h!.conn().client;
}
function depotKey(): string {
  return "depot:" + h!.conn().port;
}
function srcKey(): string {
  return source === "depot" ? depotKey() : source === "local" ? localKey() : wsKey();
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
    saveBrowseSource(s); // persist across workspace switch + restart
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
    folderSyncCache.clear();
    rootPath = "";
    clientRoot = "";
    tree = null;
    selectedTreePath = "";
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
    queryRoot = "//" + h.conn().client; // browse through the client view
    tree = rootForSource(); // honor the user's persisted source (Local by default)

    // Restore the saved selection only for the stream-rooted sources; the Depot
    // source has no per-workspace subject (it browses //).
    if (source === "depot") {
      selectedTreePath = "";
    } else {
      const savedPath = view?.treePath && view.treePath.startsWith(stream) ? view.treePath : "";
      if (savedPath) {
        selectedTreePath = savedPath;
        if (view!.histMode === "file") history.selectFile(savedPath);
        else history.loadFolder(savedPath);
      } else {
        selectedTreePath = stream;
        history.loadFolder(stream);
      }
    }
    pending.load();
    pending.startOfflineScan(); // low-rate workspace scan for offline changes

    // A saved "repo" tab no longer exists (Depot is a Files-pane source now).
    const valid: Tab[] = ["history", "pending", "streams", "log", "notes"];
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
      for (const k of node.children) {
        if (k.isDir) computeFolderSync(k);
        else if (k.rec) applyFileCl(k, k.rec);
      }
      return;
    }

    if (source === "local") {
      // Show the on-disk listing immediately — do NOT block folder-open on the
      // server round-trip. The sync markers + ignored/uncommitted greying fill
      // in when the (async) server listing for this folder arrives.
      const local = await localChildren(clientRoot, rootPath, path);
      node.children = local ? buildChildren(local) : [];
      node.loaded = true;
      node.loading = false;
      const conn = h.conn();
      const q = browse.toQuery(path);
      void Promise.all([
        safe(() => p4.dirs(conn, q)),
        safe(() => p4.files(conn, q)),
      ]).then(([d, f]) => {
        const sDirs = new Set(d.map((r) => base(r.dir ?? "").toLowerCase()));
        const sFiles = new Map<string, P4Record>(); // basename -> fstat rec (sync marker)
        for (const r of f) if (r.depotFile) sFiles.set(base(r.depotFile).toLowerCase(), r);
        // Mutate THROUGH node.children (the reactive proxy) so the UI updates.
        for (const k of node.children) {
          const bn = base(k.path).toLowerCase();
          if (k.isDir) {
            k.untracked = !sDirs.has(bn);
            if (!k.untracked) computeFolderSync(k); // background sync marker
          } else {
            const rec = sFiles.get(bn);
            k.untracked = !rec; // on disk but not in the depot
            if (rec) {
              k.rec = rec; // gives the file its have/head sync marker
              applyFileCl(k, rec);
            }
          }
        }
      });
      return;
    }

    // workspace (server stream)
    const mem = folderCache.get(path);
    // Instant paint from the fast cache; on a miss consult the SQLite source of
    // truth (cold/evicted localStorage) before falling back to disk.
    let cached = mem ?? loadFolder(h.conn().client, path);
    if (!cached && !mem) cached = await loadFolderAsync(h.conn().client, path);
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
    // Background: subfolder sync markers; file have/head changelists for tooltips.
    for (const k of node.children) {
      if (k.isDir) computeFolderSync(k);
      else if (k.rec) applyFileCl(k, k.rec);
    }
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
  // Each source has its own index, keyed distinctly: workspace = client name,
  // local = local:<client>, depot = depot:<port> (shared per server).
  // Build (or rebuild) the workspace (server-stream) index.
  async buildIndex() {
    if (!h || !h.connected() || !h.conn().client || !rootPath || indexing) return;
    indexing = true;
    try {
      indexCount = await idx.build(h.conn(), wsKey(), rootPath);
    } catch {
      /* leave count as-is */
    } finally {
      indexing = false;
    }
  },
  // Rebuild the on-disk (Local) index in the background — disk state can change.
  async buildLocalIndex() {
    if (!h || !h.connected() || !h.conn().client || !clientRoot || !rootPath) return;
    try {
      await idx.buildLocal(localKey(), clientRoot, rootPath);
    } catch {
      /* best effort */
    }
  },
  // Build the whole-depot index (eager, on connect). Can be large on big servers.
  async buildDepotIndex() {
    if (!h || !h.connected() || !h.conn().port) return;
    try {
      await idx.buildDepot(h.conn(), depotKey());
    } catch {
      /* best effort */
    }
  },
  // Ensure the workspace index exists (build if new), and refresh the local one.
  async ensureIndex() {
    if (!h || !h.connected() || !h.conn().client) return;
    try {
      indexCount = await idx.status(wsKey());
    } catch {
      indexCount = 0;
    }
    if (indexCount === 0) browse.buildIndex();
    browse.buildLocalIndex(); // fire-and-forget refresh of the on-disk index
  },
  // Ensure the whole-depot index exists (build once per server, eager on connect).
  async ensureDepotIndex() {
    if (!h || !h.connected() || !h.conn().port) return;
    try {
      if ((await idx.status(depotKey())) === 0) browse.buildDepotIndex();
    } catch {
      /* best effort */
    }
  },
  // Per-keystroke fuzzy search over the CURRENT source's index (no p4 per key).
  async searchDepot(term: string): Promise<P4Record[]> {
    if (!h || !term.trim()) return [];
    if (source !== "depot" && !h.conn().client) return [];
    const paths = await idx.search(srcKey(), term.trim(), 200);
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
    const port = h.conn().port;
    if (streamRows.length === 0) {
      const cached = loadStreamsCache(port); // instant from cache
      if (cached.length) streamRows = cached;
      else streamsLoading = true;
    }
    const rows = await safe(() => p4.streams(h!.conn()));
    streamsLoading = false;
    if (h.conn().port !== port) return; // switched server during the fetch
    streamRows = rows;
    saveStreamsCache(port, rows);
  },

  // --- refresh ---------------------------------------------------------------
  async refresh() {
    if (!h || !h.connected() || refreshing) return;
    refreshing = true;
    folderCache.clear();
    folderSyncCache.clear();
    history.clearMemCache();
    clearClientCache(h.conn().client);
    // Re-fetch the tree + history in the BACKGROUND so a caller (e.g. a submit)
    // isn't blocked on reloading every expanded folder — the panes update when
    // their data arrives.
    void (async () => {
      try {
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
    })();
  },
};
