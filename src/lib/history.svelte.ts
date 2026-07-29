//! History feature store: the center History pane (file/folder revisions) and
//! the right-hand changelist-details pane. The revision list is DERIVED from the
//! store (scope p4hist:<client>, key = subject id) — the single source the UI
//! reads; loadFolder/selectFile only WRITE that store (progressively, for the
//! background paging of older changelists). The details pane fetches once per
//! selection (no competing cache), so it stays plain $state. Shared bits (conn,
//! path translation, notices) come via init().

import { p4, openDiffWindow, type P4Conn, type P4Record } from "$lib/p4";
import { editor, isUnrealAsset, unrealAssetName } from "$lib/editor.svelte";
import type { HistEntry } from "$lib/cache";
import {
  storeGet,
  storeSet,
  storeSetMem,
  hydrate,
  storeClearScope,
  cacheGetSync,
  cacheSet,
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

// Servers whose `filelog` can't be used (see selectFile). Remembered per server
// so the failure is paid once, not on every file click; persisted so it survives
// a restart. Cleared only by forgetting the server.
const filelogKey = () => `filelog-unusable:${h?.conn().port ?? ""}`;
function filelogUnusable(): boolean {
  return !!h && cacheGetSync("nav", filelogKey()) === "1";
}
function markFilelogUnusable(): void {
  if (h) cacheSet("nav", filelogKey(), "1");
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
      await history.loadFolder(depotFile); // changelists for this file
      return;
    }
    const [rev, fs] = await Promise.all([
      safe(() => p4.filelog(h!.conn(), q, 200)),
      safe(() => p4.fstat(h!.conn(), q)),
    ]);
    if (seq !== loadSeq) return;
    if (rev.length === 0 && fs.length > 0) {
      // fstat sees the file but filelog returned nothing → filelog is the problem.
      markFilelogUnusable();
      await history.loadFolder(depotFile);
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
  async openFileDiff(depotFile: string, rev: number) {
    try {
      // UE assets → Unreal's asset-diff tool; text → the in-app diff window or
      // the external P4DIFF tool, per Options → Editor.
      if (isUnrealAsset(depotFile)) {
        h!.setNotice("Opening Unreal diff…", 15000); // instant feedback; replaced on completion
        const pair = await p4.diffPairRev(h!.conn(), depotFile, rev);
        const mode = await p4.openUnrealDiff(
          h!.conn(), pair.left, pair.right, unrealAssetName(depotFile), pair.leftLabel, pair.rightLabel,
        );
        h!.setNotice(
          mode === "remote"
            ? "Diff opened in the running Unreal Editor."
            : "Launching Unreal Editor for the diff — this takes a moment…",
          8000,
        );
      } else if (editor.diffTool === "inapp") {
        await openDiffWindow(await p4.diffPairRev(h!.conn(), depotFile, rev));
      } else {
        await p4.openDiff(h!.conn(), depotFile, rev);
      }
    } catch (e) {
      h!.setNotice(String(e), 5000);
    }
  },
};
