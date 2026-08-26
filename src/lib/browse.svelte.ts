//! Browse feature store: the left depot tree, the Streams tab, and the workspace
//! refresh. History/details loads are delegated to the history store. Shared bits
//! (conn, connected, center tab) come via init().
//!
//! The tree is DERIVED from the store: the only view state is which folders are
//! expanded (per source) + the selection; children, sync markers, and loaded/
//! loading states are computed from the store scopes + marker caches on every
//! change. Commands (expand, refresh, source switch) never build UI state — they
//! fetch from p4/disk and WRITE the store; the derived tree re-renders. One data
//! path: p4 → store → view.

import { p4, idx, type P4Conn, type P4Record, type SearchHits } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";
import { localChildren, type FolderContents } from "$lib/cache";
import { history } from "$lib/history.svelte";
import { pending } from "$lib/pending.svelte";
import {
  cacheGet,
  cacheSet,
  storeGet,
  storeSet,
  storeSetMem,
  hydrate,
} from "$lib/store.svelte";
import {
  loadBrowseSource,
  saveBrowseSource,
  loadShowDeleted,
  saveShowDeleted,
  type ViewState,
} from "$lib/nav";

type Tab = "history" | "pending" | "reviews" | "streams" | "log" | "notes";
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

// --- tree view state (ALL of it) ---------------------------------------------
// Which folders are expanded, per source — survives source switches. Plain Sets;
// `treeVer` bumps re-run the derived tree (the store's own $state map re-triggers
// it for data changes; the bump covers these plain structures).
const expandedBySource = new Map<BrowseSource, Set<string>>();
// Folder fetches in flight (`${source}:${path}`) — drives the per-node "…".
const loadingPaths = new Set<string>();
let treeVer = $state(0);

function expandedSet(): Set<string> {
  let s = expandedBySource.get(source);
  if (!s) {
    s = new Set();
    expandedBySource.set(source, s);
  }
  return s;
}

// Folder-contents store scopes, keyed by path. The derived tree ALWAYS builds
// children from these; the p4 fetch only writes them (ensureFolder). `treeScope`
// MUST stay in sync with the history/refresh clears.
const treeScope = (client: string) => `p4tree:${client}`; // workspace stream folder contents
const depotScope = (port: string) => `p4depot:${port}`; // whole-depot folder contents
const localScope = (client: string) => `p4local:${client}`; // on-disk structure
const markScope = (client: string) => `p4localmark:${client}`; // server listing → local sync markers

function parseJson<T>(s: string | undefined | null): T | null {
  if (s === undefined || s === null) return null;
  try {
    return JSON.parse(s) as T;
  } catch {
    return null;
  }
}

let rootPath = $state(""); // stream root, e.g. //Curiosity/main
// The workspace the pane currently DESCRIBES. Between `conn.client` changing and
// openWorkspace finishing, the tree and history still hold the previous
// workspace's state, and anything that persists "the current view" in that gap
// files the old selection under the new workspace.
let viewClient = $state("");
let clientRoot = $state(""); // local workspace root, e.g. H:\Dev\...\Curiosity
let selectedTreePath = $state("");
let source = $state<BrowseSource>(loadBrowseSource()); // Files pane data source (persisted)
// Show files that are deleted at head (`p4 files` reports them). Off by default:
// they can't be synced and would look like sync failures; on, they're rendered
// struck-through so they're unmistakable. Persisted globally.
let showDeleted = $state(loadShowDeleted());
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

/** Is this listing record a file that no longer exists at head? `p4 files`
 *  reports deleted revisions too, so without this the tree shows files that
 *  aren't in the depot anymore — they can never sync ("up-to-date", no have
 *  revision, nothing on disk), which reads exactly like a broken sync. */
function isDeletedAtHead(rec: P4Record): boolean {
  const a = (rec.headAction ?? rec.action ?? "").toLowerCase();
  return a.includes("delete") || a === "purge" || a === "archive";
}

// The stream list for the current server, read from the store (scope `p4:streams`,
// key = port). `void streamsVer` subscribes to (re)loads so the getter re-runs
// after loadStreams() even if the hooks weren't wired on the first evaluation.
function currentStreamRows(): P4Record[] {
  void streamsVer;
  if (!h || !h.connected()) return [];
  const port = h.conn().port;
  if (!port) return [];
  return parseJson<P4Record[]>(storeGet("p4:streams", port)) ?? [];
}
// Loading = connected to a server whose stream list isn't in the store yet.
function streamsAreLoading(): boolean {
  void streamsVer;
  if (!h || !h.connected()) return false;
  const port = h.conn().port;
  return !!port && storeGet("p4:streams", port) === undefined;
}

// --- sync markers (command side: fetch → cache; the derive only READS) --------
// Folder markers: have-change vs head-change under a folder (same signal the
// History view shows). Computed lazily per folder, throttled so a wide listing
// doesn't spawn a p4 process per subfolder at once, cached in memory + store.
type FolderSync = { status: "synced" | "stale" | "nosync"; have: string; head: string };
const folderSyncCache = new Map<string, FolderSync>();
// A stale file's have-CL, keyed by path.toLowerCase().
const fileHaveClCache = new Map<string, string>();
const markersInflight = new Set<string>(); // paths being fetched (either kind)
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

/** Fetch a folder's sync marker into the caches (memory + store) and bump the
 *  tree. No-op if cached or already in flight; instant from the persisted store
 *  on repeat visits (Refresh clears it so it recomputes). */
async function computeFolderSync(path: string): Promise<void> {
  if (!h) return;
  const cacheKey = path.toLowerCase();
  if (folderSyncCache.has(cacheKey) || markersInflight.has(cacheKey)) return;
  // Deliberately NOT read from the store. A marker persisted across sessions
  // says what was true when it was computed, and nothing invalidates it when the
  // depot moves on — so a workspace hundreds of changelists behind kept showing
  // green folders while the History tab said "synced @199718" against a head of
  // @200333. In-memory only: still instant while browsing, never a lie from
  // yesterday. It is written for Refresh's benefit and read by nothing.
  markersInflight.add(cacheKey);
  try {
    const conn = h.conn();
    const q = browse.toQuery(path);
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
    treeVer++;
  } finally {
    markersInflight.delete(cacheKey);
  }
}

/** Fetch a stale file's synced (have) changelist into the cache. */
async function computeFileHaveCl(path: string): Promise<void> {
  if (!h) return;
  const key = path.toLowerCase();
  if (fileHaveClCache.has(key) || markersInflight.has(key)) return;
  markersInflight.add(key);
  try {
    const conn = h.conn();
    const spec = browse.toQuery(path) + "#have";
    const rows = await withFolderSyncSlot(() => safe(() => p4.changesExact(conn, spec)));
    fileHaveClCache.set(key, rows[0]?.change ?? "");
    treeVer++;
  } finally {
    markersInflight.delete(key);
  }
}

/** Kick the marker fetches a folder's children need (dir sync markers; stale
 *  files' have-CLs). Called from the command side whenever a folder's contents
 *  are (re)available — never from the derive. */
function kickMarkers(path: string): void {
  if (!h) return;
  const c = readFolder(path);
  if (!c) return;
  if (source === "local") {
    const mark = parseJson<{ dirs: string[]; files: P4Record[] }>(
      storeGet(markScope(h.conn().client), path),
    );
    if (!mark) return; // markers arrive with the server listing; kicked again then
    const sDirs = new Set(mark.dirs.map((x) => x.toLowerCase()));
    for (const d of c.dirs) {
      if (d.dir && sDirs.has(base(d.dir).toLowerCase())) computeFolderSync(d.dir);
    }
    for (const r of mark.files) {
      if (r.depotFile && r.haveRev && r.haveRev !== (r.headRev ?? "")) computeFileHaveCl(r.depotFile);
    }
    return;
  }
  for (const d of c.dirs) if (d.dir) computeFolderSync(d.dir);
  for (const f of c.files) {
    if (f.depotFile && f.haveRev && f.haveRev !== (f.headRev ?? "")) computeFileHaveCl(f.depotFile);
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

// --- the derived tree ---------------------------------------------------------

/** The current source's root path ("//" for depot, else the stream root). */
function rootPathForSource(): string {
  return source === "depot" ? "//" : rootPath;
}

/** Read a folder's contents from the store for the current source (reactive). */
function readFolder(path: string): FolderContents | null {
  if (!h) return null;
  const scope =
    source === "depot"
      ? depotScope(h.conn().port)
      : source === "local"
        ? localScope(h.conn().client)
        : treeScope(h.conn().client);
  return parseJson<FolderContents>(storeGet(scope, path));
}

/** Build one node of the derived tree: state from the view sets, children from
 *  the store, markers from the caches. Recurses into expanded directories. */
function buildDirNode(path: string, name: string): TreeNode {
  const node = makeNode(path, true);
  if (name) node.name = name;
  const exp = expandedSet().has(path);
  node.expanded = exp;
  node.loading = loadingPaths.has(`${source}:${path}`);
  const c = readFolder(path);
  node.loaded = c !== null;
  // Folder sync marker (memory cache; hydrated from the store on compute).
  const fs = folderSyncCache.get(path.toLowerCase());
  if (fs) {
    node.folderSync = fs.status;
    node.haveCl = fs.have;
    node.headCl = fs.head;
  }
  if (!exp || !c) return node; // collapsed (or not yet loaded) — children unrendered
  // Local source: the server listing marks dirs/files present in the depot;
  // anything on disk but absent is `untracked` (ignored / uncommitted).
  let mark: { dirs: string[]; files: P4Record[] } | null = null;
  if (source === "local") {
    mark = parseJson<{ dirs: string[]; files: P4Record[] }>(
      storeGet(markScope(h!.conn().client), path),
    );
  }
  const sDirs = mark ? new Set(mark.dirs.map((x) => x.toLowerCase())) : null;
  const sFiles = mark ? new Map<string, P4Record>() : null;
  if (mark && sFiles) {
    for (const r of mark.files) if (r.depotFile) sFiles.set(base(r.depotFile).toLowerCase(), r);
  }
  const kids: TreeNode[] = [];
  for (const d of c.dirs) {
    if (!d.dir) continue;
    const k = buildDirNode(d.dir, "");
    if (sDirs) k.untracked = !sDirs.has(base(d.dir).toLowerCase());
    kids.push(k);
  }
  for (const f of c.files) {
    if (!f.depotFile) continue;
    const gone = isDeletedAtHead(f);
    if (gone && !showDeleted) continue; // hidden unless the user opts in
    const k = makeNode(f.depotFile, false, f);
    k.deleted = gone; // rendered struck-through; never counted as unsynced
    if (sFiles) {
      const rec = sFiles.get(base(f.depotFile).toLowerCase());
      k.untracked = !rec;
      if (rec) k.rec = rec;
    }
    // have/head changelists for the sync tooltip: head from fstat; have == head
    // when synced, else from the async cache (computeFileHaveCl fills it).
    const rec = k.rec;
    if (rec) {
      k.headCl = rec.headChange ?? "";
      if (rec.haveRev) {
        k.haveCl =
          rec.haveRev === (rec.headRev ?? "") ? k.headCl : fileHaveClCache.get(f.depotFile.toLowerCase());
      }
    }
    kids.push(k);
  }
  node.children = kids;
  return node;
}

// The whole visible tree, derived from view state + store + marker caches.
// `void treeVer` is read FIRST so the derive always subscribes (never sticks on
// an early null); the storeGet reads subscribe it to every visible folder's data.
const tree: TreeNode | null = $derived.by(() => {
  void treeVer;
  if (!h) return null;
  const rootP = rootPathForSource();
  if (!rootP) return null;
  const root = buildDirNode(rootP, source === "depot" ? "Depots" : "");
  return root;
});

// --- commands: fetch p4/disk → write the store ---------------------------------

/** Fetch a folder's contents from the server/disk and WRITE them to the store —
 *  the only place tree data is populated. The store write re-runs the derived
 *  tree; markers are kicked after each write. */
async function ensureFolder(path: string): Promise<void> {
  if (!h) return;
  const src = source; // snapshot: ignore results if the source changed mid-fetch

  if (src === "depot") {
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
    if (source === src) kickMarkers(path);
    return;
  }

  if (src === "local") {
    const client = h.conn().client;
    // Disk structure first → show the files fast; the sync dots follow.
    const local = await localChildren(clientRoot, rootPath, path);
    if (local) storeSet(localScope(client), path, JSON.stringify(local));
    // Server listing → sync/ignored markers.
    const q = browse.toQuery(path);
    const [d, f] = await Promise.all([
      safe(() => p4.dirs(h!.conn(), q)),
      safe(() => p4.files(h!.conn(), q)),
    ]);
    const dirs = d.map((r) => base(r.dir ?? "")).filter(Boolean);
    // Deleted-at-head files aren't in the depot: a leftover local copy is
    // untracked, not "synced" (see isDeletedAtHead).
    const files = f.filter((r) => r.depotFile && !isDeletedAtHead(r));
    storeSet(markScope(client), path, JSON.stringify({ dirs, files }));
    if (source === src) kickMarkers(path);
    return;
  }

  // workspace (server stream). On a cold cache, seed a provisional on-disk listing
  // (memory only — not persisted, the server result replaces it) so the folder
  // isn't blank while p4 responds.
  const client = h.conn().client;
  if (storeGet(treeScope(client), path) === undefined) {
    const local = await localChildren(clientRoot, rootPath, path);
    if (local) storeSetMem(treeScope(client), path, JSON.stringify(local));
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
  if (source === src) kickMarkers(path);
}

/** Open a folder: hydrate the persisted layers (instant cached paint via the
 *  derive), then refresh from the server — Local always (disk changes), the
 *  server sources once per session. The spinner shows only while nothing is in
 *  the store to render. */
async function openFolder(path: string): Promise<void> {
  if (!h) return;
  const client = h.conn().client;
  const key = `${source}:${path}`;

  if (source === "local") {
    hydrate(localScope(client), path);
    hydrate(markScope(client), path);
  } else if (source === "depot") {
    hydrate(depotScope(h.conn().port), path);
  } else {
    hydrate(treeScope(client), path);
  }
  treeVer++; // re-derive with whatever hydration brought in
  kickMarkers(path); // markers for the cached children (no-ops when unavailable)

  if (source === "local") {
    // Always re-read disk + markers, in the BACKGROUND — the store writes
    // re-derive the tree; no spinner (its server listing is slow on the root).
    void ensureFolder(path);
    return;
  }
  if (sessionFetched.has(key)) return; // fresh this session — the store is current
  sessionFetched.add(key); // claimed before the await so a re-expand can't double-fetch
  loadingPaths.add(key);
  treeVer++;
  try {
    await ensureFolder(path);
  } finally {
    loadingPaths.delete(key);
    treeVer++;
  }
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
  /** The workspace the tree and history currently describe. */
  get viewClient() {
    return viewClient;
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
  get showDeleted() {
    return showDeleted;
  },
  /** Toggle listing deleted-at-head files; re-projects the open tree so the
   *  change is immediate (the store already holds the deleted records). */
  setShowDeleted(v: boolean) {
    if (v === showDeleted) return;
    showDeleted = v;
    saveShowDeleted(v);
    treeVer++; // the tree is derived — re-run the projection
  },
  /** The search index the tree is currently querying — one namespace per source
   *  and workspace. The tree watches it so a filter is re-run when the ground
   *  under it changes. */
  get searchScope() {
    void treeVer;
    return h ? srcKey() : "";
  },
  get source() {
    return source;
  },
  /** Switch the Files pane between on-disk / workspace-server / whole-depot. The
   *  tree re-derives for the new source; its expansion set (per source) is intact
   *  from the last visit, so switching back restores the open folders. */
  async setSource(s: BrowseSource) {
    if (!h || s === source) return;
    const was = selectedTreePath;
    const wasFile = history.mode === "file";
    source = s;
    saveBrowseSource(s); // persist across workspace switch + restart
    if (s === "local") void browse.refreshLocalIndexIfStale();
    selectedTreePath = "";
    const rootP = rootPathForSource();
    // The History tab describes what the Files pane has selected, so a source
    // switch has to settle it: follow the same path into the new source when it
    // is addressable there (a stream path is a depot path, so workspace and depot
    // usually agree), and otherwise stop describing anything. Leaving it alone was
    // the bug — switching to Depot showed the depot tree beside the previous
    // source's history.
    const addressable = !!was && !!rootP && (rootP === "//" || was.startsWith(rootP));
    if (addressable) {
      selectedTreePath = was;
      if (wasFile) void history.selectFile(was);
      else void history.loadFolder(was);
    } else {
      history.reset(); // the path does not exist in this source: describe nothing
    }
    if (!rootP) return; // no workspace open (derive renders nothing)
    const exp = expandedSet();
    if (exp.size === 0) exp.add(rootP); // first visit: expand the root
    treeVer++;
    // Load every expanded folder (instant from the store on a revisit; fetches on
    // a cold first visit). Parents first so provisional listings nest correctly.
    const open = [...exp].sort((a, b) => a.length - b.length);
    for (const p of open) void openFolder(p);
  },

  /** Clear all browse state (on disconnect / workspace switch). */
  reset() {
    sessionFetched.clear();
    expandedBySource.clear();
    loadingPaths.clear();
    folderSyncCache.clear();
    fileHaveClCache.clear();
    rootPath = "";
    clientRoot = "";
    viewClient = ""; // the pane describes no workspace until openWorkspace says so
    selectedTreePath = "";
    treeVer++;
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
    viewClient = h.conn().client;
    queryRoot = "//" + h.conn().client; // browse through the client view

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
    const valid: Tab[] = ["history", "pending", "reviews", "streams", "log", "notes"];
    const tab = valid.includes(view?.tab as Tab) ? (view!.tab as Tab) : fallbackTab;
    if (tab === "streams") browse.loadStreams();
    browse.ensureIndex(); // background: build the fuzzy-search index if new
    h.setTab(tab);

    // Open the root (expanded by default); the derive renders as data lands.
    const rootP = rootPathForSource();
    expandedSet().add(rootP);
    treeVer++;
    await openFolder(rootP);
  },

  // --- file tree -------------------------------------------------------------
  // Single click: select, and show what was selected. A file gives its server
  // history and details; a folder gives its changelists. The same in all three
  // sources — the depot source used to fold/unfold on a click instead, for no
  // reason that survives contact with p4: `changes //depot/path/...` answers for
  // any depot, workspace or not. Only the synced-CL marker is missing there,
  // since `#have` says nothing about a path outside the workspace view.
  // Expanding is the triangle's job (or a double click), everywhere.
  selectNode(node: TreeNode) {
    selectedTreePath = node.path;
    h?.setTab("history"); // explicit user navigation → show History
    if (node.isDir) history.loadFolder(node.path);
    else history.selectFile(node.path);
  },

  // Triangle / double click: toggle fold state. Expanding loads the folder
  // (instant from the store when cached; fetches otherwise).
  expandNode(node: TreeNode) {
    if (!node.isDir) return;
    const exp = expandedSet();
    if (exp.has(node.path)) {
      exp.delete(node.path);
      treeVer++;
      return;
    }
    exp.add(node.path);
    treeVer++;
    void openFolder(node.path);
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
      cacheSet("nav", `idxlocalbuilt:${localKey()}`, String(Date.now()));
    } catch {
      /* best effort */
    }
  },
  /** Refresh the Local index only when it has gone stale. Rebuilding it on
   *  EVERY boot walked ~600k files and rewrote as many rows — a >100MB WAL
   *  transaction per restart, which is what bloated the database and starved
   *  its checkpoints. Half an hour of staleness is fine for a fuzzy-search
   *  index; a truly fresh view is one Refresh away. */
  async refreshLocalIndexIfStale() {
    if (!h || !h.connected() || !h.conn().client) return;
    const key = `idxlocalbuilt:${localKey()}`;
    const at = Number((await cacheGet("nav", key)) ?? 0);
    if (Date.now() - at < 30 * 60 * 1000) return;
    void browse.buildLocalIndex();
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
    void browse.refreshLocalIndexIfStale(); // NOT unconditional — see the method
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
  // Per-keystroke search over the CURRENT source's index (no p4 per key). The
  // view filters on `contains`; `fuzzy` feeds the suggestion list.
  async searchDepot(term: string): Promise<SearchHits> {
    const none: SearchHits = { contains: [], fuzzy: [] };
    if (!h || !term.trim()) return none;
    if (source !== "depot" && !h.conn().client) return none;
    return await idx.search(srcKey(), term.trim(), 200);
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
  /** Re-fetch everything visible. Stale-while-revalidate: the tree scopes are NOT
   *  cleared (the current view stays up while fresh data replaces it store write
   *  by store write); the sync-marker caches ARE cleared so they recompute. */
  async refresh() {
    if (!h || !h.connected() || refreshing) return;
    refreshing = true;
    sessionFetched.clear();
    folderSyncCache.clear();
    fileHaveClCache.clear();
    folderSyncCache.clear(); // markers live for the session; Refresh recomputes
    history.clearMemCache();
    treeVer++; // drop the marker dots until they recompute
    // Re-fetch the expanded folders + history in the BACKGROUND so a caller (e.g.
    // a submit) isn't blocked — the derived tree updates as each write lands.
    void (async () => {
      try {
        const open = [...expandedSet()].sort((a, b) => a.length - b.length);
        await Promise.all(open.map((p) => ensureFolder(p)));
        if (selectedTreePath && history.mode === "file") await history.selectFile(selectedTreePath);
        else if (selectedTreePath) await history.loadFolder(selectedTreePath);
        browse.buildIndex(); // rebuild the fuzzy-search index in the background
      } finally {
        refreshing = false;
      }
    })();
  },
};
