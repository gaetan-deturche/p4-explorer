//! History feature store: the center History pane (file/folder revisions with
//! stale-while-revalidate caching + background paging) and the right-hand
//! changelist-details pane. Shared bits (conn, tab switch, notices) via init().

import { p4, type P4Conn, type P4Record } from "$lib/p4";
import { loadHist, saveHist, type HistEntry } from "$lib/cache";

type Hooks = {
  conn: () => P4Conn;
  showHistoryTab: () => void; // flip the center tab to History
  setNotice: (m: string, ms?: number) => void;
};

const CHUNK = 50;
const CAP = 400;

let h: Hooks | null = null;
// Monotonic token so stale center-pane loads are dropped when selection changes.
let loadSeq = 0;
let loadTimer: number | null = null;
// In-memory (session) history cache; persistent copy lives in $lib/cache.
const memCache = new Map<string, HistEntry>();

let mode = $state<"folder" | "file">("folder");
let subject = $state("");
let rows = $state<P4Record[]>([]);
let loading = $state(false);
let haveChange = $state("");
let haveRev = $state("");
let more = $state(false); // background paging of older changelists in flight
let selectedChange = $state("");
let descRows = $state<P4Record[]>([]);
let descLoading = $state(false);

async function safe<T>(fn: () => Promise<T[]>): Promise<T[]> {
  try {
    return await fn();
  } catch {
    return [];
  }
}

// Delayed loading indicator: keep the previous list visible and only show
// "Loading…" if fresh data hasn't arrived within 2s (nothing to keep → now).
function beginLoad(seq: number) {
  if (loadTimer !== null) clearTimeout(loadTimer);
  if (rows.length === 0) {
    loading = true;
    return;
  }
  loadTimer = window.setTimeout(() => {
    if (seq === loadSeq) loading = true;
  }, 2000);
}
function endLoad() {
  if (loadTimer !== null) {
    clearTimeout(loadTimer);
    loadTimer = null;
  }
  loading = false;
}

function apply(e: HistEntry) {
  mode = e.mode;
  subject = e.subject;
  if (e.mode === "folder") {
    haveChange = e.have;
    haveRev = "";
  } else {
    haveRev = e.have;
    haveChange = "";
  }
  rows = e.rows;
  more = false;
}
function cache(id: string, e: HistEntry) {
  memCache.set(id, e);
  saveHist(h!.conn().client, id, e);
}

export const history = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get mode() {
    return mode;
  },
  get subject() {
    return subject;
  },
  get rows() {
    return rows;
  },
  get loading() {
    return loading;
  },
  get haveChange() {
    return haveChange;
  },
  get haveRev() {
    return haveRev;
  },
  get more() {
    return more;
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

  /** Drop the in-memory history cache (on refresh). */
  clearMemCache() {
    memCache.clear();
  },
  /** Clear cache + all pane state (on disconnect / workspace switch). */
  reset() {
    memCache.clear();
    rows = [];
    subject = "";
    haveChange = "";
    haveRev = "";
    more = false;
    selectedChange = "";
    descRows = [];
  },

  async loadFolder(path: string, seq: number = ++loadSeq) {
    if (!h) return;
    h.showHistoryTab();
    const id = "F:" + path;

    const cached = memCache.get(id) ?? loadHist(h.conn().client, id);
    if (cached) {
      endLoad();
      apply(cached);
      this.autoSelectHave();
    } else {
      beginLoad(seq);
    }

    // Fetch the first chunk AND the synced-CL together so the list appears with
    // its greying/bold already correct — no ungreyed-then-greyed flash.
    const [firstBatch, have] = await Promise.all([
      safe(() => p4.changes(h!.conn(), path, CHUNK)),
      safe(() => p4.haveChange(h!.conn(), path)),
    ]);
    if (seq !== loadSeq) return; // selection changed; keep whatever's shown
    endLoad();
    const haveCl = have[0]?.change ?? "";

    // If we showed cached data, refresh fully into an accumulator and swap once
    // (no shrink-then-grow flicker). Otherwise paint progressively.
    let all = firstBatch;
    if (!cached) apply({ mode: "folder", subject: path, rows: all, have: haveCl });

    if (firstBatch.length === CHUNK) {
      let before = Math.min(...firstBatch.map((b) => Number(b.change))) - 1;
      while (all.length < CAP && Number.isFinite(before) && before > 0) {
        if (!cached) more = true;
        const batch = await safe(() => p4.changes(h!.conn(), path, CHUNK, before));
        if (seq !== loadSeq) return;
        if (batch.length === 0) break;
        all = [...all, ...batch];
        if (!cached) rows = all; // progressive when nothing was shown
        const min = Math.min(...batch.map((b) => Number(b.change)));
        if (batch.length < CHUNK || !Number.isFinite(min) || min <= 1) break;
        before = min - 1;
      }
    }
    more = false;
    const fresh: HistEntry = { mode: "folder", subject: path, rows: all, have: haveCl };
    apply(fresh); // single atomic swap (covers the cached-refresh case)
    this.autoSelectHave();
    cache(id, fresh);
  },

  async selectFile(depotFile: string) {
    if (!h) return;
    const seq = ++loadSeq;
    h.showHistoryTab();
    const id = "R:" + depotFile;

    const cached = memCache.get(id) ?? loadHist(h.conn().client, id);
    if (cached) {
      endLoad();
      apply(cached);
      this.autoSelectHave();
    } else {
      beginLoad(seq);
    }

    const [rev, fs] = await Promise.all([
      safe(() => p4.filelog(h!.conn(), depotFile, 200)),
      safe(() => p4.fstat(h!.conn(), depotFile)),
    ]);
    if (seq !== loadSeq) return;
    endLoad();
    const fresh: HistEntry = {
      mode: "file",
      subject: depotFile,
      rows: rev,
      have: fs[0]?.haveRev ?? "",
    };
    apply(fresh);
    this.autoSelectHave();
    cache(id, fresh);
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
    if (mode === "folder") {
      if (haveChange) this.selectChange(haveChange);
      else {
        selectedChange = "";
        descRows = [];
      }
    } else {
      const row = rows.find((r) => r.rev === haveRev);
      if (row?.change) this.selectChange(row.change);
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
      await p4.openDiff(h!.conn(), depotFile, rev);
    } catch (e) {
      h!.setNotice(String(e), 5000);
    }
  },
};
