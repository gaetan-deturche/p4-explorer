//! Browse feature store: the left depot tree (stale-while-revalidate folder
//! cache + fuzzy search index), the Streams and Repo browser tabs, and the
//! workspace refresh. History/details loads are delegated to the history store.
//! Shared bits (conn, connected, center tab) come via init().

import { p4, idx, type P4Conn, type P4Record } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";
import { clearClientCache, buildChildren, localChildren, type FolderContents } from "$lib/cache";
import { history } from "$lib/history.svelte";
import { pending } from "$lib/pending.svelte";
import {
  cacheGetSync,
  cacheSet,
  cacheClearScope,
  storeGet,
  storeSet,
  storeSetMem,
  hydrate,
} from "$lib/store.svelte";
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
// Folders whose contents we've already refreshed from the server THIS session, so
// a re-expand doesn't re-fetch (the store still holds them). Keyed `${source}:${path}`.
const sessionFetched = new Set<string>();

// Folder-contents store scopes, keyed by path. Children are ALWAYS built from
// these (projectChildren); the p4 fetch only writes them (ensureFolder) — one
// data path: p4 → store → view. `treeScope` MUST match cache.ts (Refresh's
// clearClientCache drops it).
const treeScope = (client: string) => `p4tree:${client}`; // workspace stream folder contents
const depotScope = (port: string) => `p4depot:${port}`; // whole-depot folder contents
const localScope = (client: string) => `p4local:${client}`; // on-disk structure
const markScope = (client: string) => `p4localmark:${client}`; // server listing → local sync markers

function parseFolder(s: string | undefined): FolderContents | null {
  if (s === undefined) return null;
  try {
    return JSON.parse(s) as FolderContents;
  } catch {
    return null;
  }
}

let rootPath = $state(""); // stream root, e.g. //Curiosity/main
let clientRoot = $state(""); // local workspace root, e.g. H:\Dev\...\Curiosity
let tree = $state<TreeNode | null>(null);
let selectedTreePath = $state("");
let source = $state<BrowseSource>(loadBrowseSource()); // Files pane data source (persisted)
let refreshing = $state(false);
let indexing = $state(false);
let indexCount = $state(0);
// Streams are DERIVED from the store (scope `p4:streams`, key = port) — the same
// single-path rule as the pending list. `loadStreams()` only writes the store;
// `streamsVer` bumps so the getters re-run even before the hooks were wired.
let streamsVer = $state(0);

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

// The stream list for the current server, read from the store (scope `p4:streams`,
// key = port). `void streamsVer` subscribes to (re)loads so the getter re-runs
// after loadStreams() even if the hooks weren't wired on the first evaluation.
function currentStreamRows(): P4Record[] {
  void streamsVer;
  if (!h || !h.connected()) return [];
  const port = h.conn().port;
  if (!port) return [];
  const json = storeGet("p4:streams", port);
  if (json === undefined) return [];
  try {
    return JSON.parse(json) as P4Record[];
  } catch {
    return [];
  }
}
// Loading = connected to a server whose stream list isn't in the store yet.
function streamsAreLoading(): boolean {
  void streamsVer;
  if (!h || !h.connected()) return false;
  const port = h.conn().port;
  return !!port && storeGet("p4:streams", port) === undefined;
}

// Folder sync markers: have-change vs head-change under a folder (same signal
// the History view shows). Computed lazily per folder, throttled so a wide
// listing doesn't spawn a p4 process per subfolder at once, and cached.
type FolderSync = { status: "synced" | "stale" | "nosync"; have: string; head: string };
const folderSyncCache = new Map<string, FolderSync>();
// A stale file's have-CL, keyed by path.toLowerCase(). Since projectChildren
// rebuilds child nodes and can re-run on each store write, this stops the
// per-stale-file `changesExact` query from re-firing on every re-projection.
const fileHaveClCache = new Map<string, string>();
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
/** Fill in a directory node's sync marker + have/head changelists (mutate node).
 *  Instant from the in-memory or persisted (store) cache; only queries p4 on a
 *  cold miss. Refresh clears the store scope so it recomputes. */
async function computeFolderSync(node: TreeNode): Promise<void> {
  if (!h) return;
  const cacheKey = node.path.toLowerCase();
  const apply = (r: FolderSync) => {
    node.folderSync = r.status;
    node.haveCl = r.have;
    node.headCl = r.head;
  };
  const hit = folderSyncCache.get(cacheKey);
  if (hit) {
    apply(hit);
    return;
  }
  const client = h.conn().client;
  const scope = `p4foldersync:${client}`;
  try {
    const s = cacheGetSync(scope, node.path);
    if (s) {
      const stored = JSON.parse(s) as FolderSync;
      folderSyncCache.set(cacheKey, stored);
      apply(stored); // instant on repeat visits — no p4 (refreshed via Refresh)
      return;
    }
  } catch {
    /* corrupt */
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
  cacheSet(scope, node.path, JSON.stringify(res));
  apply(res);
}

/** A stale file's synced changelist (its head CL is already known from fstat). */
async function computeFileHaveCl(node: TreeNode): Promise<void> {
  if (!h || node.haveCl) return;
  const key = node.path.toLowerCase();
  const hit = fileHaveClCache.get(key);
  if (hit !== undefined) {
    node.haveCl = hit;
    return;
  }
  const conn = h.conn();
  const spec = browse.toQuery(node.path) + "#have";
  const rows = await withFolderSyncSlot(() => safe(() => p4.changesExact(conn, spec)));
  const cl = rows[0]?.change ?? "";
  fileHaveClCache.set(key, cl);
  node.haveCl = cl;
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

/** Apply the Local-source sync/ignored markers to a folder's children from a
 *  server listing: `dirs` = subfolder basenames present in the depot, `files` =
 *  fstat records (have/head). Anything on disk but absent is `untracked`. */
function applyLocalMarkers(children: TreeNode[], dirs: string[], files: P4Record[]): void {
  const sDirs = new Set(dirs.map((x) => x.toLowerCase()));
  const sFiles = new Map<string, P4Record>();
  for (const r of files) if (r.depotFile) sFiles.set(base(r.depotFile).toLowerCase(), r);
  for (const k of children) {
    const bn = base(k.path).toLowerCase();
    if (k.isDir) {
      k.untracked = !sDirs.has(bn);
      if (!k.untracked) computeFolderSync(k);
    } else {
      const rec = sFiles.get(bn);
      k.untracked = !rec;
      if (rec) {
        k.rec = rec;
        applyFileCl(k, rec);
      }
    }
  }
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

// Reconcile a node's children against a freshly-built desired list: reuse the
// existing node for a matching path so its VIEW state (expanded / loaded / its own
// children / already-computed markers) survives a re-projection; only the data
// (name, fstat rec) is refreshed. New paths are added, gone paths dropped, order
// follows `desired`. Without this, re-projecting would collapse expanded subtrees.
function reconcileChildren(existing: TreeNode[], desired: TreeNode[]): TreeNode[] {
  if (existing.length === 0) return desired;
  const byPath = new Map(existing.map((n) => [n.path, n]));
  return desired.map((d) => {
    const cur = byPath.get(d.path);
    if (cur && cur.isDir === d.isDir) {
      cur.name = d.name;
      if (d.rec) cur.rec = d.rec; // fresh fstat when the source carries one
      return cur;
    }
    return d;
  });
}

// Build node.children from the store ONLY (the single source), reconciled against
// the current children so expansion is preserved. For Local, the disk structure
// and the server-listing markers each come from their store scope; for
// workspace/depot, the folder contents come from the store and the sync markers
// are filled in the background (themselves store-backed via p4foldersync). Called
// for the instant paint and again after ensureFolder writes fresh data.
function projectChildren(node: TreeNode): void {
  if (!h) return;
  const path = node.path;
  if (source === "local") {
    const client = h.conn().client;
    const local = parseFolder(storeGet(localScope(client), path));
    const desired = local ? buildChildren(local) : [];
    node.children = reconcileChildren(node.children, desired);
    let mark: { dirs: string[]; files: P4Record[] } | null = null;
    try {
      const s = storeGet(markScope(client), path);
      if (s) mark = JSON.parse(s) as { dirs: string[]; files: P4Record[] };
    } catch {
      /* corrupt */
    }
    // Apply markers to the reconciled children (in place, so reused nodes keep
    // their expansion). Only reapplied when the server listing is present.
    if (mark) applyLocalMarkers(node.children, mark.dirs, mark.files);
    return;
  }
  const scope = source === "depot" ? depotScope(h.conn().port) : treeScope(h.conn().client);
  const c = parseFolder(storeGet(scope, path));
  const desired = c ? buildChildren(c) : [];
  node.children = reconcileChildren(node.children, desired);
  // Background: subfolder sync markers; file have/head changelists for tooltips.
  for (const k of node.children) {
    if (k.isDir) computeFolderSync(k);
    else if (k.rec) applyFileCl(k, k.rec);
  }
}

// Fetch a folder's contents from the server and WRITE them to the store — the
// only place the tree data is populated. Re-projects the node after each write so
// the view updates progressively (matches the old cached-then-fresh paint) while
// still flowing exclusively through the store. Dispatches on source.
async function ensureFolder(node: TreeNode): Promise<void> {
  if (!h) return;
  const path = node.path;

  if (source === "depot") {
    let c: FolderContents;
    if (path === "//") {
      const depots = await safe(() => p4.depots(h!.conn()));
      c = { dirs: depots.filter((d) => d.name).map((d) => ({ dir: "//" + d.name }) as P4Record), files: [] };
    } else {
      const [d, f] = await Promise.all([
        safe(() => p4.dirs(h!.conn(), path)),
        safe(() => p4.files(h!.conn(), path)),
      ]);
      c = { dirs: d, files: f };
    }
    storeSet(depotScope(h.conn().port), path, JSON.stringify(c));
    projectChildren(node);
    return;
  }

  if (source === "local") {
    const client = h.conn().client;
    // Disk structure first → show the files fast; the sync dots follow.
    const local = await localChildren(clientRoot, rootPath, path);
    if (local) {
      storeSet(localScope(client), path, JSON.stringify(local));
      projectChildren(node);
    }
    // Server listing → sync/ignored markers.
    const q = browse.toQuery(path);
    const [d, f] = await Promise.all([
      safe(() => p4.dirs(h!.conn(), q)),
      safe(() => p4.files(h!.conn(), q)),
    ]);
    const dirs = d.map((r) => base(r.dir ?? "")).filter(Boolean);
    const files = f.filter((r) => r.depotFile);
    storeSet(markScope(client), path, JSON.stringify({ dirs, files }));
    projectChildren(node);
    return;
  }

  // workspace (server stream). On a cold cache, seed a provisional on-disk listing
  // (memory only — not persisted, the server result replaces it) so the folder
  // isn't blank while p4 responds.
  const client = h.conn().client;
  if (storeGet(treeScope(client), path) === undefined) {
    const local = await localChildren(clientRoot, rootPath, path);
    if (local) {
      storeSetMem(treeScope(client), path, JSON.stringify(local));
      projectChildren(node);
    }
  }
  // Query via the client view, but rebuild each child's path from the display
  // parent + basename so the tree stays in stream-depot form (virtual streams too).
  const q = browse.toQuery(path);
  const [d, f] = await Promise.all([
    safe(() => p4.dirs(h!.conn(), q)),
    safe(() => p4.files(h!.conn(), q)),
  ]);
  const c: FolderContents = {
    dirs: d.map((r) => (r.dir ? { ...r, dir: `${path}/${base(r.dir)}` } : r)),
    files: f.map((r) => (r.depotFile ? { ...r, depotFile: `${path}/${base(r.depotFile)}` } : r)),
  };
  storeSet(treeScope(client), path, JSON.stringify(c));
  projectChildren(node);
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
    return currentStreamRows();
  },
  get streamsLoading() {
    return streamsAreLoading();
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
    sessionFetched.clear();
    folderSyncCache.clear();
    fileHaveClCache.clear();
    rootPath = "";
    clientRoot = "";
    tree = null;
    selectedTreePath = "";
    history.reset();
    pending.clear();
    streamsVer++; // re-run the streams getters (clears them on disconnect)
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
  // Populate a directory node's children. The children are DERIVED from the store
  // (projectChildren); the server fetch only WRITES the store (ensureFolder) — so
  // there is one data path, p4 → store → view, for both the instant (cached) paint
  // and the refresh. Dispatches on the current source inside those two helpers.
  async loadNode(node: TreeNode) {
    if (!h || node.loading) return;
    node.loading = true;
    const path = node.path;
    const client = h.conn().client;
    const port = h.conn().port;
    const key = `${source}:${path}`;

    // 1) Instant paint: hydrate the persisted layers into the reactive map, then
    //    project whatever the store already holds.
    if (source === "local") {
      hydrate(localScope(client), path);
      hydrate(markScope(client), path);
    } else if (source === "depot") {
      hydrate(depotScope(port), path);
    } else {
      hydrate(treeScope(client), path);
    }
    projectChildren(node);
    if (node.children.length > 0) node.loaded = true; // painted (stale-ok) content

    // 2) Refresh the store from the server, re-projecting as fresh data lands
    //    (ensureFolder does that). One path: p4 → store → view.
    if (source === "local") {
      // Local always re-reads disk + markers, but in the BACKGROUND — it re-projects
      // itself, so don't hold the spinner (its server listing is slow on the root).
      node.loaded = true;
      node.loading = false;
      void ensureFolder(node);
      return;
    }
    // Stream/depot: one awaited fetch per session (nothing to show until it returns,
    // so the spinner is warranted). Claimed before the await to avoid a double-fetch.
    if (!sessionFetched.has(key)) {
      sessionFetched.add(key);
      await ensureFolder(node);
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
    hydrate("p4:streams", port); // fill the store from localStorage/SQLite (instant paint)
    streamsVer++;
    const rows = await safe(() => p4.streams(h!.conn()));
    if (h.conn().port !== port) return; // switched server during the fetch
    storeSet("p4:streams", port, JSON.stringify(rows)); // ONE write → the getters re-derive
    streamsVer++;
  },

  // --- refresh ---------------------------------------------------------------
  async refresh() {
    if (!h || !h.connected() || refreshing) return;
    refreshing = true;
    sessionFetched.clear();
    folderSyncCache.clear();
    fileHaveClCache.clear();
    cacheClearScope(`p4foldersync:${h.conn().client}`); // persisted folder-sync markers
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
