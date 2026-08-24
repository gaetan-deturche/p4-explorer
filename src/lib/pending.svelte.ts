//! Pending-changelists feature store: the pending list + all changelist and
//! file actions (submit, review, rename, revert, reopen, new CL) and the
//! file-content providers for PendingList. Shared bits come via `init()`.

import { openUrl } from "@tauri-apps/plugin-opener";
import {
  p4,
  type P4Conn,
  type P4Record,
  type ReviewInfo,
  type UndoResult,
  type UnshelveResult,
  type OpenResult,
} from "$lib/p4";
import { openDiff } from "$lib/opendiff";
import { cacheGet, cacheGetSync, cacheSet, storeGet, hydrate, storeSet, storeSetMem } from "$lib/store.svelte";

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  syncing: () => boolean; // shared busy guard
  setSyncing: (v: boolean) => void;
  setNotice: (m: string, ms?: number) => void;
  setError: (m: string) => void;
  askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
  // A confirmation carrying one tick-box (unshelve's "leave the files offline").
  askOption: (
    msg: string,
    title: string,
    ok: string,
    optionLabel: string,
    optionChecked?: boolean,
  ) => Promise<{ ok: boolean; option: boolean }>;
  refresh: () => Promise<void>;
};

let h: Hooks | null = null;
let swarmBase = "";
let version = $state(0); // bumps on every (re)load so views refetch file lists
let reviews = $state<Record<string, ReviewInfo | null>>({}); // change → Swarm review status
// Open-file count per changelist ("default" included). Undefined until the
// first `p4 opened` lands — an unknown count must not read as "empty", or an
// action gated on emptiness gets offered on a changelist full of work.
let openCounts = $state<Record<string, number> | null>(null);
// Pending changelists that hold shelved files. One cheap `changes -s shelved`
// per load answers it for every CL at once — describing each CL instead would
// cost one command per row.
let shelved = $state<Set<string>>(new Set());
// The last unshelve's result, handed from the action callback to the notice that
// reports it — `mutate` can only take a message, not the outcome it describes.
let lastUnshelve: UnshelveResult | null = null;
// Depot paths p4 is holding a resolve on. `p4 opened` says nothing about resolve
// state, so a conflicted file is indistinguishable from a plain edit without this.
let unresolved = $state<Set<string>>(new Set());
// Files open for edit whose content matches the depot: nothing to submit. Empty
// until the first answer, so a row is only ever marked on evidence.
let unchanged = $state<Set<string>>(new Set());
// Fingerprint of `p4 opened` from the last poll; null = no baseline yet.
let openedFingerprint: string | null = null;

// The pending list is DERIVED from the store — the single source the UI reads
// (see the `rows`/`loading` getters). `load()` only writes the p4 result to the
// store; the getters re-read it, so components tracking them re-render.
function currentPendingRows(): P4Record[] {
  void version; // subscribe to reloads: forces a re-run after load() even if `h` was
  // still null on the first evaluation (before init()), so the getter reaches the
  // storeGet() read below and thereafter tracks the store key directly.
  if (!h || !h.connected()) return [];
  const conn = h.conn();
  if (!conn.client) return [];
  const def = { change: "default", desc: "", user: conn.user, time: "" } as P4Record;
  const json = storeGet("p4:pending", conn.client); // reactive read
  if (json === undefined) return [def];
  try {
    return [def, ...(JSON.parse(json) as P4Record[])];
  } catch {
    return [def];
  }
}

// Offline changes (files modified/added/deleted on disk but not open in any
// changelist). Scanned on its own low-rate timer — a workspace-wide p4 scan is
// slow, so it's decoupled from the fast pending refresh and eased off further
// when the app is in the background.
// Offline list is DERIVED from the store (scope `p4:offline`, key = client); the
// scan only writes it. `offlineVer` bumps to re-run the getter (bootstrap + after
// a scan). `offlineScanning` is transient status, not data — it stays $state.
let offlineVer = $state(0);
let offlineScanning = $state(false);
let offlineScannedAt = $state<number | null>(null); // last completed scan (freshness stamp)
let offlineTimer: number | null = null;
let offlineStopped = true;
let offlineFocused = true;
const OFFLINE_MS_FOCUS = 300_000; // 5 min (the scan itself is ~30s)
const OFFLINE_MS_BG = 1_800_000; // 30 min in the background

// Persist the last offline result per workspace (store scope `p4:offline`) so
// switching back shows it instantly; a fresh scan then refreshes it. Durable in
// SQLite, mirrored in localStorage for the instant read.
function currentOffline(): P4Record[] {
  void offlineVer;
  if (!h || !h.connected()) return [];
  const client = h.conn().client;
  if (!client) return [];
  const json = storeGet("p4:offline", client);
  if (json === undefined) return [];
  try {
    return JSON.parse(json) as P4Record[];
  } catch {
    return [];
  }
}
function saveOfflineCache(client: string, recs: P4Record[]): void {
  if (client) cacheSet("p4:offline", client, JSON.stringify(recs));
}

// Cached opened/shelved files per changelist (store scopes p4:clfiles:<client> /
// p4:clshelved:<client>, key = change). `undefined` means NEVER fetched (show a
// loading state); `[]` means fetched and genuinely empty (show nothing, no flash)
// — the distinction the UI needs so an empty CL doesn't flicker "loading".
function loadClFilesCache(client: string, change: string): P4Record[] | undefined {
  if (!client) return undefined;
  const json = cacheGetSync(`p4:clfiles:${client}`, change);
  if (json === null) return undefined;
  try {
    return JSON.parse(json) as P4Record[];
  } catch {
    return undefined;
  }
}
/** Like the sync variants below, but falling through to SQLite. localStorage is
 *  only a bounded mirror — after enough caching it evicts, and a fresh boot then
 *  found nothing synchronously and sat on "Loading files…" until p4 answered,
 *  even though the list was a millisecond away on disk. */
async function loadClCacheDeep(scope: string, change: string): Promise<P4Record[] | undefined> {
  const client = h?.conn().client;
  if (!client) return undefined;
  const json = await cacheGet(`${scope}:${client}`, change);
  if (json === null) return undefined;
  try {
    return JSON.parse(json) as P4Record[];
  } catch {
    return undefined;
  }
}

function saveClFilesCache(client: string, change: string, recs: P4Record[]): void {
  if (client) cacheSet(`p4:clfiles:${client}`, change, JSON.stringify(recs));
}
function saveShelvedCache(client: string, change: string, recs: P4Record[]): void {
  if (client) cacheSet(`p4:clshelved:${client}`, change, JSON.stringify(recs));
}

// Self-scheduling loop: the next scan is armed only after the current one
// finishes, so a long (~30s) scan never overlaps or piles up behind a timer.
async function runOfflineLoop() {
  if (offlineStopped) return;
  const ran = await pending.scanOffline();
  if (offlineStopped) return;

  // If the scan was skipped (another scan held the lock — e.g. right after a
  // workspace switch), retry soon instead of waiting the full interval.
  const delay = ran ? (offlineFocused ? OFFLINE_MS_FOCUS : OFFLINE_MS_BG) : 5_000;
  offlineTimer = window.setTimeout(runOfflineLoop, delay);
}

/** Apply a change to the cached lists straight away and return its undo.
 *
 *  p4 can take seconds on a large file, and waiting for the command and the reload
 *  before touching the UI makes the app look dead. These write only the reactive
 *  map, never the persisted layers: the view updates on the next frame, the reload
 *  that follows replaces the guess with the truth, and a failure rolls it back.
 */
/** Tally `p4 opened` records per changelist. Files with no `change` field sit
 *  in the default changelist. */
function countOpen(recs: P4Record[]): void {
  const next: Record<string, number> = {};
  for (const r of recs) {
    if (!r.depotFile) continue;
    const c = String(r.change ?? "default");
    next[c] = (next[c] ?? 0) + 1;
  }
  openCounts = next;
}

function forgetFiles(files: string[]): () => void {
  const client = h?.conn().client;
  if (!client || !files.length) return () => {};
  const gone = new Set(files);
  const undo: (() => void)[] = [];

  for (const row of currentPendingRows()) {
    const change = String(row.change);
    const cached = loadClFilesCache(client, change);
    if (!cached) continue;
    const kept = cached.filter((f) => !gone.has(String(f.depotFile)));
    if (kept.length === cached.length) continue;
    const scope = `p4:clfiles:${client}`;
    storeSetMem(scope, change, JSON.stringify(kept));
    undo.push(() => storeSetMem(scope, change, JSON.stringify(cached)));
  }

  const offline = currentOffline();
  const keptOffline = offline.filter((o) => !gone.has(String(o.depotFile)));
  if (keptOffline.length !== offline.length) {
    storeSetMem("p4:offline", client, JSON.stringify(keptOffline));
    offlineVer++;
    undo.push(() => {
      storeSetMem("p4:offline", client, JSON.stringify(offline));
      offlineVer++;
    });
  }
  return () => {
    for (const f of undo) f();
  };
}

/** As `forgetFiles`, and also list it under Offline - what `revert -k` produces. */
function makeOfflineNow(file: string, record: P4Record): () => void {
  const client = h?.conn().client;
  const undoForget = forgetFiles([file]);
  if (!client) return undoForget;
  const offline = currentOffline();
  storeSetMem(
    "p4:offline",
    client,
    JSON.stringify([...offline.filter((o) => String(o.depotFile) !== file), record]),
  );
  offlineVer++;
  return () => {
    undoForget();
    storeSetMem("p4:offline", client, JSON.stringify(offline));
    offlineVer++;
  };
}

/** Which changelist a file is cached under, "" when it is in none. */
function currentChangeOf(file: string): string {
  const client = h?.conn().client;
  if (!client) return "";
  for (const row of currentPendingRows()) {
    const change = String(row.change);
    const cached = loadClFilesCache(client, change);
    if (cached?.some((f) => String(f.depotFile) === file)) return change;
  }
  return "";
}

/** Move a file between two changelists' cached lists. */
function moveFileNow(file: string, from: string, to: string): () => void {
  const client = h?.conn().client;
  if (!client) return () => {};
  const scope = `p4:clfiles:${client}`;
  const src = loadClFilesCache(client, from);
  const dst = loadClFilesCache(client, to);
  const rec = src?.find((f) => String(f.depotFile) === file);
  if (!src || !rec) return () => {};
  storeSetMem(scope, from, JSON.stringify(src.filter((f) => f !== rec)));
  if (dst) storeSetMem(scope, to, JSON.stringify([...dst, rec]));
  return () => {
    storeSetMem(scope, from, JSON.stringify(src));
    if (dst) storeSetMem(scope, to, JSON.stringify(dst));
  };
}


export const pending = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get rows() {
    return currentPendingRows(); // reads the store — reactive to components
  },
  get loading() {
    void version; // same bootstrap as currentPendingRows (re-run after load())
    // Connected to a workspace whose pending list isn't in the store yet.
    if (!h || !h.connected()) return false;
    const client = h.conn().client;
    return !!client && storeGet("p4:pending", client) === undefined;
  },
  get version() {
    return version;
  },
  get reviews() {
    return reviews;
  },
  get offline() {
    return currentOffline();
  },
  get offlineScanning() {
    return offlineScanning;
  },
  get offlineScannedAt() {
    return offlineScannedAt;
  },
  /** Has this workspace ever been scanned? `undefined` in the store means never
   *  (→ show "scanning"), while an empty list is a real cached result: zero
   *  offline files, which must NOT read as "still loading". */
  get offlineCached() {
    void offlineVer;
    if (!h || !h.connected()) return false;
    const client = h.conn().client;
    return !!client && storeGet("p4:offline", client) !== undefined;
  },

  /** Drop transient state (on disconnect / workspace switch). rows/loading are
   *  derived from the store, so they follow the client automatically. */
  clear() {
    reviews = {};
    offlineVer++; // re-run the offline getter (clears it on disconnect/switch)
    offlineScannedAt = null; // the stamp belongs to the previous workspace
    pending.stopOfflineScan();
  },

  /** Scan the whole workspace for offline changes (read-only p4 preview). The
   *  in-flight guard lives on `window`, so it serializes even across stray
   *  duplicate scan loops (e.g. dev-HMR module reloads) — a workspace-wide p4
   *  scan must never run more than once at a time. */
  async scanOffline(): Promise<boolean> {
    if (!h || !h.connected() || !h.conn().client) {
      offlineVer++;
      return true;
    }
    // Never scan while a sync is writing files: mid-write they look modified and
    // not-yet-written ones look deleted, so the scan would cache a list of
    // phantom "offline changes" that stays until the next scan. Returning false
    // makes the scan loop retry shortly (see runOfflineLoop).
    if (h.syncing()) return false;
    const w = window as unknown as { __p4guiOfflineBusy?: boolean };
    if (w.__p4guiOfflineBusy) return false; // a scan is already running — skipped
    w.__p4guiOfflineBusy = true;
    offlineScanning = true;
    const conn = h.conn();
    const client = conn.client; // snapshot: the workspace THIS scan is for
    try {
      let recs: P4Record[];
      try {
        recs = await p4.status(conn);
      } catch {
        return true; // cancelled (by an interactive write) or failed — keep the list
      }
      // Keep only real change entries (an action + a file).
      const result = recs.filter((r) => (r.action ?? r.status) && (r.clientFile || r.depotFile));
      saveOfflineCache(client, result); // store write, keyed by the scanned workspace
      // No "still on that workspace?" guard needed: the getter reads the CURRENT
      // client's store entry, so a stale scan writing another client's key can't
      // leak into the view — it just populates that workspace's cache for later.
      offlineVer++;
      if (h && h.conn().client === client) offlineScannedAt = Date.now(); // freshness stamp
      return true;
    } finally {
      offlineScanning = false;
      w.__p4guiOfflineBusy = false;
    }
  },
  /** Start the low-rate offline-change scan: show the cached result immediately,
   *  then run a fresh scan (and keep polling) to update it. Idempotent — if a
   *  loop is already running, just refresh the cached display. */
  startOfflineScan() {
    if (!h) return;
    hydrate("p4:offline", h.conn().client); // instant: last known set for this workspace
    offlineVer++;
    if (!offlineStopped) return; // a loop is already active — don't spawn another
    offlineStopped = false;
    runOfflineLoop();
  },
  stopOfflineScan() {
    offlineStopped = true;
    if (offlineTimer !== null) clearTimeout(offlineTimer);
    offlineTimer = null;
  },
  /** Focus-aware pacing: slow the offline scan right down in the background, and
   *  pull the next scan in soon when focus returns. */
  setFocused(v: boolean) {
    if (v === offlineFocused) return;
    offlineFocused = v;
    if (!offlineStopped && v && offlineTimer !== null) {
      clearTimeout(offlineTimer);
      offlineTimer = window.setTimeout(runOfflineLoop, 2000);
    }
  },

  /** Refresh the pending changelists: hydrate the store from persistence (so the
   *  cached list shows at once), then write the fresh p4 result to the store —
   *  the derived `rows` re-renders. One path: the UI only reads the store. */
  async load() {
    if (!h) return;
    const conn = h.conn();
    if (!h.connected() || !conn.client) {
      reviews = {};
      unresolved = new Set();
      unchanged = new Set();
      version++; // re-run the derived getters (clears the list on disconnect)
      return;
    }
    const client = conn.client;
    hydrate("p4:pending", client); // fill the store from localStorage/SQLite (reactive)
    version++; // paint the cached list now, before the server round-trip
    const r = await p4.pending(conn, 100).catch(() => [] as P4Record[]);
    if (h.conn().client !== client) return; // switched workspace during the fetch
    storeSet("p4:pending", client, JSON.stringify(r)); // ONE write → rows re-derive
    openedFingerprint = null; // next poll re-baselines instead of re-reloading
    version++; // fresh list in; also signals open CLs to refetch their file lists
    pending.loadReviews(); // fire-and-forget: populate Swarm review badges
    pending.loadUnresolved();
    pending.loadShelved();
    pending.loadOpenCounts();
    pending.loadUnchanged();
  },

  /** Cheap change-detection poll, run at keepalive rate: ONE `p4 opened` over
   *  the whole client, fingerprinted. Only an actual change (a checkout made in
   *  the editor or p4v, a revert elsewhere...) triggers the full pending reload
   *  — so external changes show within seconds without per-tick churn. */
  async checkExternalChanges() {
    if (!h || !h.connected() || h.syncing() || !h.conn().client) return;
    const conn = h.conn();
    const recs = await p4.opened(conn, "").catch(() => null);
    if (recs === null || h.conn().client !== conn.client) return; // error / switched
    countOpen(recs); // same records the fingerprint uses — no extra command
    const fp = recs
      .filter((r) => r.depotFile)
      .map((r) => `${r.depotFile}|${r.action}|${r.change}`)
      .sort()
      .join("\n");
    // Review status, shelves and resolve state move WITHOUT `p4 opened`
    // changing — somebody approves a review, Swarm re-shelves, a colleague
    // submits over a file we hold. None of that touched the fingerprint, so the
    // badges sat stale until a manual refresh. They are refreshed every tick
    // now: one batched Swarm request, one server-filtered `changes -s shelved`
    // (0.09s for the whole client) and one server-filtered `fstat -Ru`.
    void pending.loadReviews();
    void pending.loadShelved();
    void pending.loadUnresolved();
    void pending.loadUnchanged();

    if (fp === openedFingerprint) return;
    const first = openedFingerprint === null;
    openedFingerprint = fp;
    if (!first) pending.load(); // baseline tick just records; changes reload
  },

  /** One client-wide `p4 opened` → how many files each changelist holds. Also
   *  refreshed by the change-detection poll, which fetches the same records. */
  async loadOpenCounts() {
    if (!h || !h.connected() || !h.conn().client) return;
    const conn = h.conn();
    const recs = await p4.opened(conn, "").catch(() => null);
    if (recs === null || h.conn().client !== conn.client) return;
    countOpen(recs);
  },
  /** Files this changelist holds open; undefined while still unknown. */
  openCount(change: string): number | undefined {
    return openCounts ? (openCounts[change] ?? 0) : undefined;
  },
  /** Can `p4 change -d` succeed on this changelist? Only when p4 has nothing
   *  left to refuse over: no open files, no shelf, and not the default CL
   *  (which cannot be deleted at all). Undefined counts mean "not yet known",
   *  and answer false — better a missing entry than one that lies. */
  canDelete(change: string): boolean {
    return change !== "default" && pending.openCount(change) === 0 && !shelved.has(change);
  },

  /** Which opened files still need resolving (server-side filtered, so cheap). */
  async loadUnresolved() {
    if (!h || !h.connected() || !h.conn().client) return;
    const conn = h.conn();
    try {
      const files = await p4.resolveNeeded(conn, "");
      if (h.conn().client !== conn.client) return; // workspace switched mid-fetch
      unresolved = new Set(files);
    } catch {
      unresolved = new Set(); // no badge rather than a stale one
    }
  },
  /** Which pending changelists have a shelf. */
  async loadShelved() {
    if (!h || !h.connected() || !h.conn().client) return;
    const conn = h.conn();
    try {
      const recs = await p4.shelvedChanges(conn);
      if (h.conn().client !== conn.client) return; // switched workspace mid-fetch
      shelved = new Set(recs.map((r) => String(r.change)));
    } catch {
      shelved = new Set(); // no entry rather than a stale one
    }
  },
  /** True when this changelist holds shelved files. */
  hasShelf(change: string): boolean {
    return shelved.has(change);
  },

  /** Revert every file in a changelist. The count in the prompt comes from the
   *  cached file list, so the user is told how much they are discarding rather
   *  than agreeing to "the files, whatever they are". */
  revertChangelist(change: string) {
    const client = h?.conn().client ?? "";
    const files = (loadClFilesCache(client, change) ?? []).map((f) => String(f.depotFile));
    const what = change === "default" ? "the default changelist" : `@${change}`;
    const count = files.length
      ? `${files.length} file${files.length === 1 ? "" : "s"} in ${what}`
      : `every file in ${what}`;
    pending.action(
      () => p4.revertChange(h!.conn(), change),
      `Revert ${count}?\n\nTheir local changes are discarded and cannot be recovered. The changelist itself stays (delete it separately).`,
      "Revert changelist",
      "Revert all",
      "Changelist reverted.",
      { optimistic: () => forgetFiles(files) },
    );
  },

  /** Delete an empty pending changelist. p4 refuses while it still holds opened
   *  or shelved files and says so — deleting those for the user would throw away
   *  work they never asked to lose. */
  deleteChangelist(change: string) {
    pending.action(
      () => p4.deleteChange(h!.conn(), change),
      `Delete changelist @${change}?\n\nIt holds no files; only its description goes away.`,
      "Delete changelist",
      "Delete",
      `Changelist @${change} deleted.`,
      { refresh: false }, // no synced content changes
    );
  },

  /** True when p4 is holding a resolve on this depot file. */
  needsResolve(depotFile: string): boolean {
    return unresolved.has(depotFile);
  },

  /** Which opened files are byte-identical to the depot — one `p4 diff -sr` for
   *  the whole workspace (measured: 0.21s over 21 files, binaries included). */
  async loadUnchanged() {
    if (!h || !h.connected() || !h.conn().client) return;
    const conn = h.conn();
    try {
      const files = await p4.unchangedOpen(conn);
      if (h.conn().client !== conn.client) return; // workspace switched mid-fetch
      unchanged = new Set(files);
    } catch {
      unchanged = new Set(); // no marker rather than a stale one
    }
  },
  /** True when this file is open for edit but identical to the depot revision —
   *  a checkout with nothing in it. Only `edit`/`integrate` files can qualify:
   *  an `add` has no depot revision to match, a `delete` is meant to be missing,
   *  and p4 lists neither, so nothing else is ever marked. */
  isUnchanged(depotFile: string): boolean {
    return unchanged.has(depotFile);
  },
  /** How many of a changelist's files have nothing in them. */
  unchangedCount(files: { depotFile?: unknown }[]): number {
    return files.filter((f) => unchanged.has(String(f.depotFile))).length;
  },

  /** Fetch the Swarm review status for every numbered changelist (the review is
   *  linked by change, so we can't pre-filter on a description marker). Builds a
   *  fresh map so statuses for removed changelists are pruned. */
  async loadReviews() {
    if (!h || !h.connected() || !h.conn().client) return;
    const conn = h.conn();
    const targets = currentPendingRows()
      .filter((r) => r.change !== "default")
      .map((r) => String(r.change));
    if (!targets.length) {
      reviews = {};
      return;
    }
    // ONE request for all of them: Swarm's change[] filter takes a list, and each
    // returned review names the changelists it covers. It used to be a request
    // per changelist, which is why polling this was not affordable — and so the
    // badges only moved on a manual refresh.
    try {
      const rows = await p4.swarmReviewsFor(conn, targets);
      if (h.conn().client !== conn.client) return; // switched workspace mid-flight
      const next: Record<string, ReviewInfo | null> = {};
      for (const r of rows) {
        next[String(r.change)] = {
          id: Number(r.id ?? 0),
          state: String(r.state ?? ""),
          stateLabel: String(r.stateLabel ?? ""),
        };
      }
      reviews = next;
    } catch {
      /* unreachable Swarm → leave the badges as they were */
    }
  },

  /** Run a workspace-mutating action, then reload. `refresh` (default true) also
   *  reloads the depot tree + history; skip it for CL-only moves that change no
   *  synced content. Pending is always reloaded in `finally`, so an optimistic UI
   *  update reconciles with the truth on success AND rolls back on error. */
  async mutate(
    runFn: () => Promise<unknown>,
    // A function when the message can only be written once the action has run
    // (an undo names the changelist it just created).
    okNotice: string | (() => string),
    opts?: { refresh?: boolean; optimistic?: () => () => void },
  ) {
    if (!h || !h.connected() || h.syncing()) return;
    // Guess the outcome first so the list reacts immediately; the reload in
    // `finally` reconciles it, and `rollback` undoes the guess if p4 refuses.
    const rollback = opts?.optimistic?.();
    h.setSyncing(true);
    try {
      await p4.cancelOfflineScan().catch(() => {}); // free its server locks before writing
      await runFn();
      h.setNotice(typeof okNotice === "string" ? okNotice : okNotice());
      if (opts?.refresh !== false) await h.refresh();
    } catch (e) {
      rollback?.();
      h.setError(String(e));
    } finally {
      pending.load();
      h.setSyncing(false);
    }
  },
  /** As `mutate`, but confirm first. */
  async action(
    runFn: () => Promise<unknown>,
    msg: string,
    title: string,
    ok: string,
    // A function when only the completed action can describe itself (see mutate).
    note: string | (() => string),
    opts?: { refresh?: boolean; optimistic?: () => () => void },
  ) {
    if (!h || !h.connected() || h.syncing()) return;
    if (!(await h.askConfirm(msg, title, ok))) return;
    await pending.mutate(runFn, note, opts);
  },

  async submit(change: string) {
    if (!h || !h.connected() || h.syncing()) return;
    const what = change === "default" ? "the default changelist" : `changelist @${change}`;
    const go = await h.askConfirm(
      `Submit ${what}?\nThis commits the files to the depot and cannot be undone.`,
      "Submit changelist",
      "Submit",
    );
    if (go) await pending.doSubmit(change);
  },
  /** Submit, reporting the result of the SUBMIT command itself — a later refresh
   *  failure must not look like a failed submit (#3). If the submit is blocked by
   *  shelved files, offer to delete the shelf and submit (#1). */
  async doSubmit(change: string) {
    if (!h || h.syncing()) return;
    const label = change === "default" ? "The default changelist" : `Changelist @${change}`;
    h.setSyncing(true);
    try {
      await p4.cancelOfflineScan().catch(() => {}); // free its server locks before submitting
      try {
        await p4.submit(h.conn(), change);
      } catch (e) {
        if (!/shelved/i.test(String(e))) throw e;
        // Blocked by shelved files — offer to remove the shelf and submit.
        h.setSyncing(false);
        const yes = await h.askConfirm(
          `${label} has shelved files that block the submit.\nRemove the shelf and submit?`,
          "Shelved files",
          "Remove shelf & submit",
        );
        if (!yes) return;
        h.setSyncing(true);
        await p4.shelveDelete(h.conn(), change);
        await p4.submit(h.conn(), change); // may still throw → outer catch
      }
      h.setNotice("Changelist submitted.");
      // Refresh separately: the submit already succeeded, so a refresh error is
      // not a submit error (don't surface it as one).
      try {
        await h.refresh();
      } catch {
        /* submit succeeded regardless */
      }
    } catch (e) {
      h.setError(String(e));
    } finally {
      pending.load();
      h.setSyncing(false);
    }
  },
  requestReview(change: string) {
    pending.action(
      () => p4.requestReview(h!.conn(), change),
      `Request a Swarm review for @${change}?\nThis adds #review to the description and shelves the files.`,
      "Request review",
      "Request",
      "Review requested.",
      { refresh: false }, // shelving changes no synced content
    );
  },
  updateReview(change: string) {
    pending.action(
      () => p4.shelveUpdate(h!.conn(), change),
      `Update the review for @${change} by re-shelving its files?`,
      "Update review",
      "Update",
      "Review updated.",
      { refresh: false }, // shelving changes no synced content
    );
  },
  /** Copy this changelist's open files to the server shelf. The files stay open
   *  and the workspace is untouched — shelving is a server-side copy, so this
   *  needs no confirmation. `-f` replaces an existing shelf. */
  shelveChangelist(change: string) {
    const replacing = shelved.has(change);
    pending.mutate(
      () => p4.shelveUpdate(h!.conn(), change),
      replacing ? `Shelf of @${change} replaced.` : `Files of @${change} shelved.`,
      { refresh: false }, // shelving changes no synced content
    );
  },

  /** Shelve just these files of a changelist. Additive: it adds them to
   *  whatever the shelf already holds rather than trimming the shelf to this
   *  set, which is p4's behaviour and the useful one — you can build a shelf up
   *  a file at a time. */
  shelveSome(change: string, files: string[]) {
    if (!files.length) return;
    const n = files.length;
    pending.mutate(
      () => p4.shelveUpdate(h!.conn(), change, files),
      `Shelved ${n} file${n === 1 ? "" : "s"} of @${change}.`,
      { refresh: false }, // a server-side copy; the workspace is untouched
    );
  },

  /** Take files back out of a changelist's shelf, leaving the rest shelved. */
  unshelveSome(change: string, files: string[]) {
    if (!files.length) return;
    const n = files.length;
    pending.action(
      () => p4.shelveDelete(h!.conn(), change, files),
      `Remove ${n === 1 ? "this file" : `these ${n} files`} from the shelf of @${change}?\n\nThe shelved copy goes; the file stays open with your edits.`,
      "Remove from shelf",
      "Remove",
      `Removed ${n} file${n === 1 ? "" : "s"} from the shelf.`,
      { refresh: false },
    );
  },

  /** Restore this changelist's shelf into the workspace.
   *
   *  Two ways, the same choice the patch dialog offers and defaulting the same
   *  way. OFFLINE writes the shelved content to disk and opens nothing, so no
   *  file is checked out and no `+l` asset is exclusive-locked — it is
   *  `review_copy_files`, which exists for exactly this and handles adds and
   *  deletes. CHECKED OUT is `p4 unshelve`, which opens every file it restores;
   *  that also brings the pending integration history a later submit records as a
   *  merge, which the offline copy cannot.
   *
   *  p4 can only merge a shelf onto files that are still open, leaving them to
   *  resolve, so the dialog says so when that applies. */
  async unshelveChangelist(change: string, only: string[] = []) {
    if (!h || !h.connected() || h.syncing()) return;
    // Only the files being restored matter for the merge warning: unshelving one
    // file onto a changelist where a DIFFERENT file is open costs nothing.
    const cached = loadClFilesCache(h.conn().client, change) ?? [];
    const openPaths = new Set(cached.map((f) => String(f.depotFile)));
    const open = only.length
      ? only.filter((f) => openPaths.has(f)).length
      : (pending.openCount(change) ?? 0);
    const what = only.length
      ? only.length === 1
        ? (only[0].split("/").pop() ?? "this file")
        : `${only.length} files of @${change}`
      : `the files of @${change}`;
    const answer = await h.askOption(
      `Unshelve ${what} into your workspace?\n\n` +
        (open
          ? `${open === 1 ? "It is" : `${open} of them are`} still open. Checked out, p4 can only ` +
            `merge the shelf onto ${open === 1 ? "it" : "those"}, which leaves ` +
            `${open === 1 ? "it" : "them"} needing a resolve.\n\n`
          : "") +
        (only.length
          ? "The rest of the shelf is untouched, and the shelved copy of these stays too."
          : "The shelf stays on the server — delete it separately when you no longer need it."),
      only.length ? "Unshelve files" : "Unshelve changelist",
      "Unshelve",
      "Leave the files offline (write to disk only, no checkout)",
      true,
    );
    if (!answer.ok) return;

    if (!answer.option) {
      lastUnshelve = null;
      await pending.mutate(
        async () => {
          lastUnshelve = await p4.unshelve(h!.conn(), change, only);
        },
        // A resolve left behind must not be silent, and only the finished command
        // knows whether there is one.
        () =>
          lastUnshelve
            ? `Restored ${lastUnshelve.restored} file${lastUnshelve.restored === 1 ? "" : "s"} from the shelf of @${change}` +
              (lastUnshelve.needsResolve ? " — some need a resolve before they can be submitted." : ".")
            : `Unshelved @${change}.`,
      );
      return;
    }

    const files = only.length
      ? only
      : (await pending.shelvedFiles(change)).map((f) => String(f.depotFile));
    if (!files.length) {
      h.setError(`@${change} has no shelved files.`);
      return;
    }
    let copied = 0;
    let failed: string[] = [];
    await pending.mutate(
      async () => {
        const res = await p4.reviewCopyFiles(h!.conn(), change, files, "offline");
        copied = res.filter((r) => r.status === "copied").length;
        failed = res.filter((r) => r.status === "failed" || r.status === "skipped").map((r) => r.message);
      },
      () =>
        `Wrote ${copied} file${copied === 1 ? "" : "s"} from the shelf of @${change} to disk` +
        (failed.length ? `, ${failed.length} skipped: ${failed[0]}` : " — nothing is checked out."),
    );
  },

  deleteShelf(change: string) {
    // Removing a shelf changes no synced content — skip the full tree/index
    // refresh (it was making this feel slow); just reload the pending list.
    pending.action(
      () => p4.shelveDelete(h!.conn(), change),
      `Delete the shelved files of @${change}?`,
      "Delete shelf",
      "Delete",
      "Shelf deleted.",
      { refresh: false },
    );
  },
  async openReview(change: string) {
    if (!h) return;
    try {
      if (!swarmBase) swarmBase = await p4.swarmUrl(h.conn()).catch(() => "");
      if (!swarmBase) {
        h.setError("Swarm URL is not configured on the server.");
        return;
      }
      await openUrl(`${swarmBase.replace(/\/$/, "")}/changes/${change}`);
    } catch (e) {
      h.setError(String(e));
    }
  },
  /** Check out, mark for add, or mark for delete. Reports what p4 actually did
   *  per file: a refusal on an exclusive file names whoever holds it, which is
   *  the whole reason to attempt the checkout in the first place. */
  async openFiles(
    verb: "edit" | "add" | "delete",
    files: string[],
    change = "",
    newDesc = "",
  ) {
    if (!h || !h.connected() || h.syncing() || !files.length) return;
    const what =
      verb === "edit" ? "Checked out" : verb === "add" ? "Marked for add" : "Marked for delete";
    if (verb === "delete") {
      const list =
        files.slice(0, 8).join("\n") +
        (files.length > 8 ? `\n…and ${files.length - 8} more` : "");
      const go = await h.askConfirm(
        `${list}\n\nMark ${files.length === 1 ? "this file" : `these ${files.length} files`} ` +
          `for delete? The local cop${files.length === 1 ? "y" : "ies"} will be removed; ` +
          `nothing leaves the depot until you submit.`,
        "Mark for delete",
        "Mark for delete",
      );
      if (!go) return;
    }
    let done = 0;
    let failed: OpenResult[] = [];
    await pending.mutate(
      async () => {
        // A named changelist is created first, so the files never pass through
        // Default on their way to it.
        const into = newDesc ? await p4.newChangelist(h!.conn(), newDesc) : change;
        const res = await p4.openFiles(h!.conn(), verb, files, into);
        done = res.filter((r) => r.ok).length;
        failed = res.filter((r) => !r.ok);
      },
      () =>
        `${what} ${done} file${done === 1 ? "" : "s"}` +
        (failed.length ? ` · ${failed.length} refused: ${failed[0].message}` : "."),
    );
    // A refusal is the interesting outcome, so it gets the error line too — the
    // notice above is transient and a blocked checkout is worth reading twice.
    if (failed.length && !done) h.setError(`${failed[0].file}\n${failed[0].message}`);
  },

  /** Rename or move a file, keeping its history. */
  async moveFile(from: string, to: string) {
    if (!h || !h.connected() || h.syncing()) return;
    const out: { res: OpenResult | null } = { res: null };
    await pending.mutate(
      async () => {
        out.res = await p4.moveFile(h!.conn(), from, to);
      },
      () =>
        out.res?.ok
          ? `Moved to ${to.split("/").pop()} — submit the change to make it stick.`
          : `Move refused: ${out.res?.message ?? "unknown reason"}`,
    );
    if (out.res && !out.res.ok) h.setError(`${from}\n${out.res.message}`);
  },

  /** Undo a SUBMITTED change, or just some of its files, into a new pending
   *  changelist. Nothing reaches the depot: the user reviews the result, trims
   *  it if they only wanted part of it, and submits.
   *
   *  Confirms against a real `p4 undo -n`, not against an intention — a file
   *  that is already open elsewhere or outside this workspace is listed as such
   *  before anything happens. Returns the result so the caller can take the user
   *  to it; null when it was refused or cancelled. */
  async undoSubmitted(change: string, files: string[] = []): Promise<UndoResult | null> {
    if (!h || !h.connected() || h.syncing()) return null;
    let preview: P4Record[] = [];
    try {
      preview = await p4.undoPreview(h.conn(), change, files);
    } catch (e) {
      h.setError(String(e));
      return null;
    }
    const can = preview.filter((r) => r.ok);
    const blocked = preview.filter((r) => !r.ok);
    if (!can.length) {
      h.setError(
        blocked[0]?.message
          ? `Nothing of @${change} can be undone here: ${blocked[0].message}`
          : `Nothing of @${change} can be undone in this workspace.`,
      );
      return null;
    }
    const name = (f: string) => String(f).split("/").pop() ?? String(f);
    const show = can.slice(0, 12).map((r) => "  " + name(String(r.depotFile)));
    if (can.length > show.length) show.push(`  …and ${can.length - show.length} more`);
    const what = files.length === 1 ? name(files[0]) : `@${change}`;
    const msg =
      `Undo ${files.length === 1 ? `${what} from @${change}` : `all of @${change}`}?\n\n` +
      `${can.length} file${can.length === 1 ? "" : "s"} will be opened in a NEW changelist at the ` +
      `revision before this change:\n${show.join("\n")}\n\n` +
      (blocked.length
        ? `${blocked.length} cannot be undone here and will be skipped:\n` +
          blocked
            .slice(0, 5)
            .map((r) => `  ${name(String(r.depotFile))} — ${r.message}`)
            .join("\n") +
          "\n\n"
        : "") +
      "Nothing is submitted — review the result and submit it yourself. You can keep part of the " +
      "change by editing the files before submitting.";
    if (!(await h.askConfirm(msg, "Undo submitted change", "Undo"))) return null;

    let res: UndoResult | null = null;
    await pending.mutate(
      async () => {
        res = await p4.undoChange(h!.conn(), change, files);
      },
      () =>
        res
          ? `Undo of @${change} opened in @${res.change} — ${res.undone} file${res.undone === 1 ? "" : "s"}` +
            (res.failed ? `, ${res.failed} skipped` : "") +
            (res.needsResolve
              ? ". Some files need a resolve before submitting."
              : ". Nothing is submitted yet.")
          : `Undo of @${change} done.`,
    );
    return res;
  },

  revert(file: string) {
    pending.action(
      () => p4.revert(h!.conn(), file),
      `${file}\n\nRevert this file? Your local changes will be discarded.`,
      "Revert file",
      "Revert",
      "File reverted.",
      { optimistic: () => forgetFiles([file]) },
    );
  },
  /** `p4 revert -k`: un-open the file but leave the edited copy on disk — i.e.
   *  turn it into an offline change, which is where it then shows up. Named for
   *  that outcome ("Make offline") rather than the mechanism, since "remove from
   *  changelist" reads like the edits go away. */
  revertKeep(file: string) {
    pending.action(
      () => p4.revertKeep(h!.conn(), file),
      `${file}\n\nMake this an offline change? The file stops being checked out (it leaves the changelist, nothing is submitted) but your edited copy stays on disk — it will show up under Offline.`,
      "Make offline",
      "Make offline",
      "File is now an offline change (your edits are still on disk).",
      {
        optimistic: () => {
          const client = h!.conn().client;
          const rec = currentPendingRows()
            .flatMap((row) => loadClFilesCache(client, String(row.change)) ?? [])
            .find((f) => String(f.depotFile) === file);
          return makeOfflineNow(file, { ...(rec ?? {}), depotFile: file } as P4Record);
        },
      },
    );
  },
  /** Move files into `change`. One p4 command whatever the count, so a dragged
   *  selection of twenty is one move — and one optimistic update. */
  reopen(files: string[], change: string) {
    if (!files.length) return;
    const label = change === "default" ? "Default" : "@" + change;
    const n = files.length;
    // CL move only — no synced content changes, so skip the tree/history refresh.
    pending.mutate(
      () => p4.reopen(h!.conn(), files, change),
      n === 1 ? `Moved to ${label}.` : `Moved ${n} files to ${label}.`,
      {
        refresh: false,
        optimistic: () => {
          const undos = files.map((f) => moveFileNow(f, currentChangeOf(f), change));
          return () => undos.forEach((u) => u());
        },
      },
    );
  },
  moveToNew(files: string[], desc: string) {
    pending.mutate(
      async () => {
        const ch = await p4.newChangelist(h!.conn(), desc);
        await p4.reopen(h!.conn(), files, ch);
      },
      files.length === 1 ? "Moved to a new changelist." : `Moved ${files.length} files to a new changelist.`,
      { refresh: false },
    );
  },
  rename(change: string, desc: string) {
    pending.mutate(() => p4.setDescription(h!.conn(), change, desc), "Changelist renamed.");
  },
  /** Export a .patch from a changelist (files=[]) or an explicit file set. */
  async generatePatch(change: string, files: string[]) {
    if (!h) return;
    const base =
      files.length === 1
        ? files[0].split("/").pop() || "file"
        : files.length > 1
          ? "selected"
          : change && change !== "default"
            ? `change-${change}`
            : "workspace";
    try {
      const path = await p4.exportPatch(h.conn(), change, files, `${base}.patch`);
      if (path) h.setNotice(`Patch saved to ${path}`, 6000);
    } catch (e) {
      h.setError(String(e));
    }
  },

  // --- file-content providers for PendingList (no `this`; safe as callbacks) --
  /** The cached opened files of a changelist (sync) — for instant paint.
   *  `undefined` = never fetched (loading); `[]` = fetched, empty (no flash). */
  localFilesCached(change: string): Promise<P4Record[] | undefined> {
    return loadClCacheDeep("p4:clfiles", change);
  },
  async localFiles(change: string): Promise<P4Record[]> {
    const conn = h!.conn();
    const recs = await p4.opened(conn, change).catch(() => [] as P4Record[]);
    const files = recs.filter((r) => r.depotFile); // drop any non-file header record
    saveClFilesCache(conn.client, change, files); // cache the FILTERED list
    return files;
  },
  /** The cached shelved files of a changelist (sync) — for instant paint.
   *  `undefined` = never fetched (loading); `[]` = fetched, empty (no flash).
   *  The default changelist can never have a shelf, so it's known-empty. */
  async shelvedFilesCached(change: string): Promise<P4Record[] | undefined> {
    if (change === "default") return [];
    return loadClCacheDeep("p4:clshelved", change);
  },
  async shelvedFiles(change: string): Promise<P4Record[]> {
    if (change === "default") return [];
    const conn = h!.conn();
    const recs = await p4.describeShelved(conn, change).catch(() => [] as P4Record[]);
    // `describe -S` returns a header record (the change itself, no depotFile)
    // even with no shelf — keep only the actual shelved files, or the cache
    // (and count) would show a phantom "Shelved (1)".
    const files = recs.filter((r) => r.depotFile);
    saveShelvedCache(conn.client, change, files);
    return files;
  },
  localDiff(file: string): Promise<string> {
    return p4.diffLocal(h!.conn(), file);
  },
  offlineDiff(file: string): Promise<string> {
    return p4.diffOffline(h!.conn(), file); // -f: diff an unopened (offline) file
  },
  /** Repair a have/disk desync (`p4 flush #head` — record only, disk untouched),
   *  then rescan so the ghost offline entry clears promptly. */
  async repairDesync(file: string) {
    await pending.mutate(
      () => p4.flush(h!.conn(), [file]),
      "Sync record repaired (have = head, file untouched).",
      { refresh: false },
    );
    void pending.scanOffline();
  },
  /** Check out offline-modified files into `change` ("" = the default
   *  changelist, or a description to create a new one for them).
   *
   *  The rows are dropped from Offline immediately. They used to linger there
   *  until the next workspace-wide offline scan finished — a ~30s job — so a
   *  freshly checked-out file sat in a changelist AND in Offline at the same
   *  time. The reload that follows brings the truth either way; the scan is
   *  still kicked off, it just no longer decides when the UI stops lying. */
  async checkoutOffline(files: string[], target: { change?: string; newDesc?: string } = {}) {
    if (!files.length) return;
    const n = files.length;
    let where = "the default changelist";
    if (target.change && target.change !== "default") where = "@" + target.change;
    if (target.newDesc) where = "a new changelist";
    await pending.mutate(
      async () => {
        const change = target.newDesc
          ? await p4.newChangelist(h!.conn(), target.newDesc)
          : (target.change ?? "");
        await p4.reconcileFiles(h!.conn(), files, change);
      },
      `Checked out ${n} file${n === 1 ? "" : "s"} into ${where}.`,
      {
        refresh: false, // no synced content changes — just opened
        optimistic: () => forgetFiles(files), // leaves Offline now, not in 30s
      },
    );
    void pending.scanOffline(); // reconciles the guess with a real scan
  },
  /** Revert a selection that may mix OPENED and OFFLINE files: opened files are
   *  `p4 revert`ed, offline ones restored to depot state (`p4 clean`) — one
   *  confirmation for the lot. Destructive for the local edits. */
  async revertMixed(files: string[]) {
    if (!files.length) return;
    const offline = new Set(currentOffline().map((o) => o.depotFile));
    const off = files.filter((f) => offline.has(f));
    const opened = files.filter((f) => !offline.has(f));
    const n = files.length;
    const list = n <= 5 ? files.join("\n") : `${n} files`;
    await pending.action(
      async () => {
        for (const f of opened) await p4.revert(h!.conn(), f);
        if (off.length) await p4.clean(h!.conn(), off);
      },
      `${list}\n\nRevert? Opened files are reverted, offline files restored to their depot state — your local changes are DISCARDED.`,
      "Revert files",
      "Revert",
      `Reverted ${n} file${n === 1 ? "" : "s"}.`,
      { optimistic: () => forgetFiles(files) },
    );
    void pending.scanOffline();
  },
  shelvedDiff(file: string, rev: number, change: string): Promise<string> {
    return p4.diffShelved(h!.conn(), file, rev, change);
  },
  // Double-click diff: the opened file against #have, the shelved one against
  // its base revision (see $lib/opendiff for which tool each ends up in).
  openLocalDiff(file: string) {
    return openDiff(h!.conn(), { kind: "local", file }, h!.setNotice);
  },
  openShelvedDiff(file: string, rev: number, change: string) {
    return openDiff(h!.conn(), { kind: "shelved", file, rev, change }, h!.setNotice);
  },
};
