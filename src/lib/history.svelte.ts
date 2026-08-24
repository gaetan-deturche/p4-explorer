//! History feature store: the center History pane (file/folder revisions) and
//! the right-hand changelist-details pane. The revision list is DERIVED from the
//! store (scope p4hist:<client>, key = subject id) — the single source the UI
//! reads; loadFolder/selectFile only WRITE that store (progressively, for the
//! background paging of older changelists). The details pane fetches once per
//! selection (no competing cache), so it stays plain $state. Shared bits (conn,
//! path translation, notices) come via init().

import { p4, type P4Conn, type P4Record } from "$lib/p4";
import { openDiff } from "$lib/opendiff";
import type { HistEntry } from "$lib/cache";
import {
  storeGet,
  storeSet,
  storeSetMem,
  hydrate,
  storeClearScope,
} from "$lib/store.svelte";

type Hooks = {
  conn: () => P4Conn;
  setNotice: (m: string, ms?: number) => void;
  // Translate a display path to the p4-query path (client view; for virtual streams).
  toQuery: (path: string) => string;
};

const CHUNK = 50;
const CAP = 400;
const PERSIST = 100; // rows written to disk; the live (in-memory) list may hold up to CAP

let h: Hooks | null = null;
// Monotonic token so stale center-pane loads are dropped when selection changes.
let loadSeq = 0;

// The revision list lives under `p4hist:<client>` — this MUST match cache.ts's
// histScope so Refresh's clearClientCache drops it. Keyed by subject id:
// "F:<path>" (folder history) or "R:<file>" (file history).
const histScope = (client: string) => `p4hist:${client}`;

let currentId = $state(""); // selected subject id — the key the view reads
let histVer = $state(0); // bumps on every (re)load so the derived getters re-run
let more = $state(false); // background paging of older changelists in flight
let deepening = $state(false); // an explicit "load older history" fetch is running
let selectedChange = $state(""); // details-pane selection
let descRows = $state<P4Record[]>([]);
let descLoading = $state(false);

async function safe<T>(fn: () => Promise<T[]>): Promise<T[]> {
  try {
    return await fn();
  } catch {
    return [];
  }
}

const curMode = (): "folder" | "file" => (currentId.startsWith("R:") ? "file" : "folder");

// Servers whose `filelog` genuinely FAILED this session (see selectFile), so the
// timeout is paid once rather than on every file click.
//
// It used to be persisted per server and cleared only by forgetting the server —
// a permanent verdict from a single bad answer. On this depot the flag was set
// and stuck while `filelog` answers in 0.1s, so every file history took the
// fallback and came back empty. Session-only now: a restart re-tries, and the
// cost of being wrong is one slow request instead of a feature that stays broken.
const filelogFailed = new Set<string>();
const serverKey = () => h?.conn().port ?? "";
function filelogUnusable(): boolean {
  return !!h && filelogFailed.has(serverKey());
}
function markFilelogUnusable(): void {
  if (h) filelogFailed.add(serverKey());
}

// The stored entry for the current subject, read from the reactive map; null
// until loaded. Memoized so the (up-to-CAP-row) parse runs once per change, not
// per getter read. `void histVer` is read first so it ALWAYS subscribes — the
// derived can never memoize-stick on an early `return null` (the bootstrap the
// pending/streams views use, safe here because it precedes every early return).
const entry: HistEntry | null = $derived.by(() => {
  void histVer;
  if (!h || !currentId) return null;
  const client = h.conn().client;
  if (!client) return null;
  const json = storeGet(histScope(client), currentId);
  if (json === undefined) return null;
  try {
    return JSON.parse(json) as HistEntry;
  } catch {
    return null;
  }
});

// Write a subject's revisions: full list to the reactive map, a bounded slice to
// disk. `memOnly` skips persistence for progressive paging chunks (persisted
// once at the end). Always bumps histVer so the getters re-derive.
function writeHist(client: string, id: string, e: HistEntry, memOnly = false): void {
  const scope = histScope(client);
  const full = JSON.stringify(e);
  if (memOnly) {
    storeSetMem(scope, id, full);
  } else {
    storeSet(scope, id, full, JSON.stringify({ ...e, rows: e.rows.slice(0, PERSIST) }));
  }
  histVer++;
}

export const history = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get mode() {
    void histVer;
    return curMode();
  },
  get subject() {
    void histVer;
    return currentId ? currentId.slice(2) : "";
  },
  get rows() {
    return entry?.rows ?? [];
  },
  get loading() {
    // No data in the store for the current subject yet → the allowed placeholder.
    void histVer;
    if (!h || !currentId) return false;
    const client = h.conn().client;
    if (!client) return false;
    return storeGet(histScope(client), currentId) === undefined;
  },
  get haveChange() {
    return curMode() === "folder" ? (entry?.have ?? "") : "";
  },
  get haveRev() {
    return curMode() === "file" ? (entry?.have ?? "") : "";
  },
  get more() {
    return more;
  },
  get deepening() {
    return deepening;
  },
  get selectedChange() {
    return selectedChange;
  },
  get descRows() {
    return descRows;
  },
  get descLoading() {
    return descLoading;
  },

  /** Extend the current subject's history further into the past (for searching
   *  beyond the default cap): folder mode pages older changelists from the
   *  oldest loaded one; file mode refetches the filelog with a larger max. Adds
   *  up to `extra` rows per call, progressively visible. */
  async deepen(extra = 1000) {
    if (!h || !currentId || deepening) return;
    const e = entry;
    if (!e) return;
    const seq = loadSeq; // invalidated by any selection change
    const client = h.conn().client;
    const id = currentId;
    deepening = true;
    try {
      if (e.mode === "file") {
        const rev = await safe(() => p4.filelog(h!.conn(), h!.toQuery(e.subject), e.rows.length + extra));
        if (seq !== loadSeq) return;
        if (rev.length > e.rows.length) writeHist(client, id, { ...e, rows: rev });
        return;
      }
      const q = h.toQuery(e.subject);
      let all = e.rows;
      const target = all.length + extra;
      let before = Math.min(...all.map((b) => Number(b.change))) - 1;
      while (all.length < target && Number.isFinite(before) && before > 0) {
        const batch = await safe(() => p4.changes(h!.conn(), q, CHUNK, before));
        if (seq !== loadSeq) return;
        if (batch.length === 0) break;
        all = [...all, ...batch];
        writeHist(client, id, { ...e, rows: all }, true); // progressive (mem only)
        const min = Math.min(...batch.map((b) => Number(b.change)));
        if (batch.length < CHUNK || !Number.isFinite(min) || min <= 1) break;
        before = min - 1;
      }
      writeHist(client, id, { ...e, rows: all }); // persist once at the end
    } finally {
      deepening = false;
    }
  },

  /** Drop the persisted + in-memory history cache for the client (on Refresh). */
  clearMemCache() {
    if (h) storeClearScope(histScope(h.conn().client));
  },
  /** Clear pane state (on disconnect / workspace switch). The old client's store
   *  entries stay cached (keyed by client) — harmless, and instant on return. */
  reset() {
    currentId = "";
    selectedChange = "";
    descRows = [];
    descLoading = false;
    more = false;
    histVer++;
  },

  /** A file's history from `p4 changes <file>` — the fallback when filelog cannot
   *  answer. It queries the FILE: loadFolder appends `/...`, which matches
   *  nothing for a file and is why the fallback returned an empty history. */
  async loadFileChanges(depotFile: string) {
    if (!h) return;
    const seq = ++loadSeq;
    const client = h.conn().client;
    const id = "R:" + depotFile;
    currentId = id;
    hydrate(histScope(client), id);
    histVer++;
    const q = h.toQuery(depotFile);
    const [rows, fs] = await Promise.all([
      safe(() => p4.changesExact(h!.conn(), q, CAP)),
      safe(() => p4.fstat(h!.conn(), q)),
    ]);
    if (seq !== loadSeq) return;
    // No per-row revision here (changes knows changelists, not revisions), so the
    // rows carry what they have; the view falls back to changelist mode.
    writeHist(client, id, {
      mode: "file",
      subject: depotFile,
      rows,
      have: fs[0]?.haveRev ?? "",
    });
    history.autoSelectHave();
  },

  async loadFolder(path: string) {
    if (!h) return;
    const seq = ++loadSeq;
    const client = h.conn().client;
    const id = "F:" + path;
    currentId = id; // select immediately: the header + any cached rows paint at once
    hydrate(histScope(client), id); // instant cached paint (localStorage / SQLite)
    histVer++;
    const hadCache = storeGet(histScope(client), id) !== undefined;
    if (hadCache) history.autoSelectHave();

    // Fetch the first chunk AND the synced-CL together so the list appears with
    // its greying/bold already correct — no ungreyed-then-greyed flash.
    const q = h.toQuery(path);
    const [firstBatch, have] = await Promise.all([
      safe(() => p4.changes(h!.conn(), q, CHUNK)),
      safe(() => p4.haveChange(h!.conn(), q)),
    ]);
    if (seq !== loadSeq) return; // selection changed — keep whatever's shown
    const haveCl = have[0]?.change ?? "";

    // Nothing cached → paint progressively as pages arrive; cache shown → keep it
    // and accumulate silently, swapping once at the end (no shrink-then-grow).
    let all = firstBatch;
    if (!hadCache) {
      writeHist(client, id, { mode: "folder", subject: path, rows: all, have: haveCl }, true);
    }

    if (firstBatch.length === CHUNK) {
      let before = Math.min(...firstBatch.map((b) => Number(b.change))) - 1;
      while (all.length < CAP && Number.isFinite(before) && before > 0) {
        if (!hadCache) more = true;
        const batch = await safe(() => p4.changes(h!.conn(), q, CHUNK, before));
        if (seq !== loadSeq) return;
        if (batch.length === 0) break;
        all = [...all, ...batch];
        if (!hadCache) {
          writeHist(client, id, { mode: "folder", subject: path, rows: all, have: haveCl }, true);
        }
        const min = Math.min(...batch.map((b) => Number(b.change)));
        if (batch.length < CHUNK || !Number.isFinite(min) || min <= 1) break;
        before = min - 1;
      }
    }
    more = false;
    // Persist + atomic swap (also covers the cached-refresh case).
    writeHist(client, id, { mode: "folder", subject: path, rows: all, have: haveCl });
    history.autoSelectHave();
  },

  async selectFile(depotFile: string) {
    if (!h) return;
    const seq = ++loadSeq;
    const client = h.conn().client;
    const id = "R:" + depotFile;
    currentId = id;
    hydrate(histScope(client), id);
    histVer++;
    if (storeGet(histScope(client), id) !== undefined) history.autoSelectHave();

    // Some servers can't answer `filelog` for a file at all: it walks integration
    // history, which on a heavily-branched depot either blows the server's
    // maxscanrows (~35s, then an error) or is refused outright when the
    // integrations reach depots the account can't read (Epic's licensee server
    // does both, depending on path syntax). `p4 changes <file>` needs none of
    // that, so fall back to the changelists that touched the file — and remember
    // per server, so later clicks don't pay the timeout again.
    const q = h.toQuery(depotFile);
    if (filelogUnusable()) {
      await history.loadFileChanges(depotFile);
      return;
    }
    // filelog is awaited WITHOUT `safe`, so a thrown error (the timeout this
    // fallback exists for) is told apart from an empty answer. An empty answer
    // is about this one file; only a failure says anything about the server.
    let rev: P4Record[] = [];
    let threw = false;
    const fstatP = safe(() => p4.fstat(h!.conn(), q));
    try {
      rev = await p4.filelog(h!.conn(), q, 200);
    } catch {
      threw = true;
    }
    const fs = await fstatP;
    if (seq !== loadSeq) return;
    if (threw) {
      markFilelogUnusable();
      await history.loadFileChanges(depotFile);
      return;
    }
    if (rev.length === 0 && fs.length > 0) {
      // The file exists but filelog said nothing about it — fall back for THIS
      // file without condemning the server.
      await history.loadFileChanges(depotFile);
      return;
    }
    writeHist(client, id, { mode: "file", subject: depotFile, rows: rev, have: fs[0]?.haveRev ?? "" });
    history.autoSelectHave();
  },

  async selectChange(change: string) {
    if (!h || !change || change === selectedChange) return;
    selectedChange = change;
    descLoading = true;
    descRows = await safe(() => p4.describe(h!.conn(), change));
    descLoading = false;
  },

  // Auto-select the changelist the workspace is currently synced to.
  autoSelectHave() {
    const e = entry;
    if (!e) return;
    if (curMode() === "folder") {
      if (e.have) history.selectChange(e.have);
      else {
        selectedChange = "";
        descRows = [];
      }
    } else {
      const row = e.rows.find((r) => r.rev === e.have);
      if (row?.change) history.selectChange(row.change);
      else {
        selectedChange = "";
        descRows = [];
      }
    }
  },

  // Changelist file diffs.
  fileDiff(depotFile: string, rev: number): Promise<string> {
    return p4.diff2(h!.conn(), depotFile, rev);
  },
  openFileDiff(depotFile: string, rev: number) {
    return openDiff(h!.conn(), { kind: "rev", file: depotFile, rev }, h!.setNotice);
  },
};
