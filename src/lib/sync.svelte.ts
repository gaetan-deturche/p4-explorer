//! Sync feature store: streaming global sync, "update to changelist", reconcile,
//! and the per-file error report + fixes. Owns its own progress/error state;
//! shared bits (conn, busy flags, notices, refresh/reload) come via `init()`.

import { listen } from "@tauri-apps/api/event";
import { p4, type P4Conn, type SyncBlocker } from "$lib/p4";

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  busy: () => boolean; // syncing || reconciling in flight
  setSyncing: (v: boolean) => void;
  setReconciling: (v: boolean) => void;
  setNotice: (m: string, ms?: number) => void;
  setError: (m: string) => void;
  askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
  refresh: () => Promise<void>;
  loadPending: () => void;
  rootPath: () => string;
  histSubject: () => string;
  histMode: () => "folder" | "file";
};

type Progress = {
  title: string;
  count: number;
  current: string;
  issues: number;
  issueLine: string;
  phase: "running" | "error";
  message: string;
};
type ErrItem = {
  line: string;
  file: string | null;
  /** What p4's refusal actually means for this file, asked for after the fact
   *  (see p4_sync_blockers). "Can't clobber writable file" says nothing about
   *  whether the local copy is precious, tracked, or merely writable. */
  why?: string;
  kind?: string;
  /** Set when the entry is not a sync FAILURE but a file p4 synced and then
   *  scheduled a resolve on. It is reported, never retried or forced. */
  resolve?: boolean;
  /** Settled since the report opened — resolved, or synced by a retry/force.
   *  It stays listed, struck through and green: a row that disappears reads the
   *  same as one that was never there, and half a dozen files fixed one at a
   *  time is exactly when you want to see what is left. */
  done?: boolean;
};
// `specs` = what the failed run targeted; the retry falls back to these when
// no per-file paths could be parsed out of the error lines. NEVER widen it to
// the whole workspace — a retry must not sync more than the run it retries.
type ErrReport = { title: string; items: ErrItem[]; specs: string[] };

/** A sync/unsync/reconcile target: a depot path and whether it's a folder. */
export type SyncTarget = { path: string; isDir: boolean };

/** Depot specs for the targets (folders get the `/...` wildcard). */
function toSpecs(targets: SyncTarget[]): string[] {
  return targets.map((t) => (t.isDir ? t.path.replace(/\/+$/, "") + "/..." : t.path));
}
/** Short label for a target set: the single item's name, else "N items". */
function targetLabel(targets: SyncTarget[]): string {
  if (targets.length === 1) {
    const p = targets[0].path.replace(/\/+$/, "");
    return p.split("/").pop() || p;
  }
  return `${targets.length} items`;
}

let h: Hooks | null = null;
let cancelled = false;
let errorItems: ErrItem[] = [];

/** Files p4 is holding a resolve on, as report entries.
 *
 *  A sync that lands on a file you have open does not fail: p4 syncs it,
 *  schedules a resolve and says so as an ordinary RESULT ("is opened and not
 *  being changed", "must resolve #6,#9 before submitting"), which never reaches
 *  the issue stream — so the run used to end with "Synced N files." and no sign
 *  that anything needed a decision. Asking p4 outright is also the only
 *  locale-proof way to know it: `fstat -Ru` is filtered server-side over opened
 *  files, so it is one cheap call per sync. */
async function resolveItems(): Promise<ErrItem[]> {
  if (!h || !h.connected() || !h.conn().client) return [];
  const conn = h.conn();
  const files = await p4.resolveNeeded(conn, "").catch(() => [] as string[]);
  if (h.conn().client !== conn.client) return []; // workspace switched mid-fetch
  return files.map((f) => ({
    file: f,
    line: `${f} — must be resolved before it can be submitted`,
    resolve: true,
  }));
}

/** Flag the entries a fix has settled, leaving the rest of the report alone. */
function mark(items: ErrItem[], settled: (i: ErrItem) => boolean): ErrItem[] {
  return items.map((i) => (settled(i) ? { ...i, done: true } : i));
}

let progress = $state<Progress | null>(null);
let errors = $state<ErrReport | null>(null);
/** Files a sync could not write, with the state behind each refusal. Kept after
 *  the dialog closes so the Offline section can list them: an untracked file
 *  blocking a sync is invisible to the offline scan (`reconcile -e -d` reports
 *  edits and deletes of files you hold; this is neither), and the only moment
 *  anything notices is the sync that trips over it. */
let blockers = $state<SyncBlocker[]>([]);
let busyFile = $state<string | null>(null); // file being fixed ("*" = all)

export const sync = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get progress() {
    return progress;
  },
  /** Files that blocked the last sync, for the Offline section. */
  get blockers() {
    return blockers;
  },
  /** Forget a blocker (it was force-synced, moved aside, or reported gone). */
  clearBlocker(file: string) {
    blockers = blockers.filter((b) => b.depotFile !== file && b.clientFile !== file && b.file !== file);
  },
  /** Drop the resolve entries p4 is no longer holding — a merge was saved. */
  async recheckResolves() {
    if (!h || !errors || !errors.items.some((i) => i.resolve)) return;
    const files = await p4.resolveNeeded(h.conn(), "").catch(() => null);
    if (!files || !errors) return; // on doubt keep the entry; a stale row is not a lie
    const still = new Set(files);
    errors = {
      ...errors,
      items: mark(errors.items, (i) => !!i.resolve && !!i.file && !still.has(i.file)),
    };
  },
  /** Re-ask p4 about the recorded blockers and drop the ones that are settled. */
  async recheckBlockers() {
    if (!h || !blockers.length) return;
    const files = blockers.map((b) => b.depotFile || b.file);
    const fresh = await p4.syncBlockers(h.conn(), files).catch(() => null);
    if (!fresh) return;
    blockers = fresh.filter((b) => b.kind !== "gone" && b.kind !== "unknown");
  },
  get errors() {
    return errors;
  },
  /** Fill in the "why" for the failures on screen, and remember the ones that
   *  are workspace state rather than a passing hiccup. */
  async explainErrors() {
    if (!h || !errors) return;
    // Only the refusals have a "why" to ask about: a resolve is p4 working as
    // intended, and syncBlockers would report the file as merely writable.
    const files = errors.items
      .filter((i) => !i.resolve)
      .map((i) => i.file)
      .filter((f): f is string => !!f);
    if (!files.length) return;
    const infos = await p4.syncBlockers(h.conn(), files).catch(() => null);
    if (!infos || !errors) return;
    const by = new Map(infos.map((i) => [i.file, i]));
    errors = {
      ...errors,
      items: errors.items.map((it) => {
        const info = it.resolve || !it.file ? undefined : by.get(it.file);
        return info ? { ...it, why: info.reason, kind: info.kind } : it;
      }),
    };
    // Only the states that persist are worth listing outside the dialog.
    blockers = infos.filter((i) => i.kind === "untracked" || i.kind === "writable" || i.kind === "modified");
  },
  get busyFile() {
    return busyFile;
  },
  dismissProgress() {
    progress = null;
  },
  dismissErrors() {
    errors = null;
  },
  cancel() {
    cancelled = true;
    p4.syncCancel().catch(() => {});
  },

  /** Run a streaming sync with the live progress dialog; open the error report
   *  afterwards if any files failed. Returns files synced (null on cancel/error). */
  async run(title: string, specs: string[] = []): Promise<number | null> {
    if (!h) return null;
    cancelled = false;
    errorItems = [];
    progress = { title, count: 0, current: "", issues: 0, issueLine: "", phase: "running", message: "" };
    const un1 = await listen<{ count: number; line: string }>("sync-progress", (e) => {
      if (progress) {
        progress.count = e.payload.count;
        progress.current = e.payload.line;
      }
    });
    const un2 = await listen<{ count: number; line: string; file: string | null }>("sync-issue", (e) => {
      errorItems.push({ line: e.payload.line, file: e.payload.file });
      if (progress) {
        progress.issues = e.payload.count;
        progress.issueLine = e.payload.line;
      }
    });
    try {
      const n = await p4.syncStream(h.conn(), specs);
      progress = null;
      // What p4 refused to write, and what it wrote but left needing a decision.
      const items = [...errorItems, ...(await resolveItems())];
      if (items.length > 0) {
        errors = { title, items, specs };
        void sync.explainErrors(); // fills in the "why" a moment later
      }
      else h.setNotice(n > 0 ? `Synced ${n} file${n === 1 ? "" : "s"}.` : "Already up to date.");
      return n;
    } catch (e) {
      if (cancelled) {
        progress = null;
        h.setNotice("Sync cancelled.");
        return null;
      }
      if (progress) {
        progress.phase = "error";
        progress.message = String(e);
      }
      return null;
    } finally {
      un1();
      un2();
    }
  },

  async globalSync() {
    if (!h || !h.connected() || h.busy()) return;
    if (
      !(await h.askConfirm(
        "Sync the entire workspace to the latest revision?\nThis may download a lot of files.",
        "Sync workspace",
        "Sync",
      ))
    ) {
      return;
    }
    // Refresh AFTER the busy flag clears: the offline scan refuses to run while a
    // sync is in flight (it would see half-written files), so scanning inside the
    // try would simply be skipped.
    let n: number | null = null;
    h.setSyncing(true);
    try {
      n = await this.run("Sync workspace");
    } finally {
      h.setSyncing(false);
    }
    if (n !== null) {
      await h.refresh();
      h.loadPending(); // offline state changed — refresh list + rescan
    }
  },

  /** Sync the currently-viewed path to a changelist (forward or backward). */
  async updateToChange(change: string) {
    if (!h || !h.connected() || h.busy()) return;
    const subject = h.histSubject();
    if (!subject) return;
    const spec = h.histMode() === "file" ? `${subject}@${change}` : `${subject}/...@${change}`;
    const label = h.histMode() === "file" ? subject : `${subject}/...`;
    if (
      !(await h.askConfirm(
        `${label}\n\nFiles will be synced to their state at @${change} (this can move backward).`,
        `Update to changelist @${change}`,
        "Update",
      ))
    ) {
      return;
    }
    let n: number | null = null;
    h.setSyncing(true);
    try {
      n = await this.run(`Update to @${change}`, [spec]);
    } finally {
      h.setSyncing(false);
    }
    if (n !== null) {
      await h.refresh(); // see globalSync: refresh once the sync is no longer busy
      h.loadPending();
    }
  },

  /** Sync depot paths (files and/or folders) to the latest revision. */
  async syncPath(targets: SyncTarget[]) {
    if (!h || !h.connected() || h.busy() || !targets.length) return;
    let n: number | null = null;
    h.setSyncing(true);
    try {
      n = await this.run(`Sync ${targetLabel(targets)}`, toSpecs(targets));
    } finally {
      h.setSyncing(false);
    }
    if (n !== null) {
      await h.refresh(); // see globalSync: refresh once the sync is no longer busy
      h.loadPending();
    }
  },

  /** "Unsync" a path: `p4 sync <spec>#none` removes the local copy AND updates
   *  the have records, so it's the true inverse of a sync — nothing is opened or
   *  marked for delete in the depot, and a later sync brings it back. p4 keeps
   *  files that are open or writable (noclobber), so local work isn't lost. */
  async unsyncPath(targets: SyncTarget[]) {
    if (!h || !h.connected() || h.busy() || !targets.length) return;
    const specs = toSpecs(targets).map((sp) => `${sp}#none`);
    const what = targets.length > 1 ? `these ${targets.length} paths` : "this path";
    if (
      !(await h.askConfirm(
        `${specs.join("\n")}\n\nRemove the local copy of ${what}? The files are deleted from disk and the workspace records them as not synced — nothing is marked for delete in the depot, and syncing again restores them. Open or modified files are kept.`,
        `Unsync ${targetLabel(targets)}`,
        "Unsync",
      ))
    ) {
      return;
    }
    let n: number | null = null;
    h.setSyncing(true);
    try {
      n = await this.run(`Unsync ${targetLabel(targets)}`, specs);
    } finally {
      h.setSyncing(false);
    }
    if (n !== null) {
      await h.refresh(); // see globalSync: refresh once the sync is no longer busy
      h.loadPending();
    }
  },

  /** Reconcile offline work under depot paths (files and/or folders). */
  async reconcilePath(targets: SyncTarget[]) {
    if (!h || !h.connected() || h.busy() || !targets.length) return;
    const specs = toSpecs(targets);
    const what = targets.length > 1 ? `these ${targets.length} paths` : "this path";
    if (
      !(await h.askConfirm(
        `${specs.join("\n")}\n\nReconcile offline work under ${what}? This opens files changed, added, or deleted outside Perforce into the default changelist.`,
        "Reconcile offline work",
        "Reconcile",
      ))
    ) {
      return;
    }
    h.setReconciling(true);
    try {
      // Exact specs (not a single path) — reconcileFiles takes a list.
      const rows = await p4.reconcileFiles(h.conn(), specs);
      const n = rows.length;
      h.setNotice(
        n > 0 ? `Reconciled ${n} file${n === 1 ? "" : "s"} into the default changelist.` : "Nothing to reconcile.",
        5000,
      );
      await h.refresh();
      h.loadPending();
    } catch (e) {
      h.setError(String(e));
    } finally {
      h.setReconciling(false);
    }
  },

  /** Reconcile offline work under the stream root into the default changelist. */
  async reconcile() {
    if (!h || !h.connected() || h.busy() || !h.rootPath()) return;
    const root = h.rootPath();
    if (
      !(await h.askConfirm(
        `${root}\n\nReconcile offline work? This opens files changed, added, or deleted outside Perforce into the default changelist.`,
        "Reconcile offline work",
        "Reconcile",
      ))
    ) {
      return;
    }
    h.setReconciling(true);
    try {
      const rows = await p4.reconcile(h.conn(), root);
      const n = rows.length;
      h.setNotice(
        n > 0 ? `Reconciled ${n} file${n === 1 ? "" : "s"} into the default changelist.` : "Nothing to reconcile.",
        5000,
      );
      await h.refresh();
      h.loadPending();
    } catch (e) {
      h.setError(String(e));
    } finally {
      h.setReconciling(false);
    }
  },

  // --- error report fixes ---------------------------------------------------
  targets(): string[] {
    if (!errors) return [];
    // A file waiting on a resolve is not a sync failure: retrying does nothing
    // and forcing would overwrite the very work the resolve exists to settle.
    // It is listed and never acted on in bulk.
    const retryable = errors.items.filter((i) => !i.resolve && !i.done);
    if (!retryable.length) return [];
    const files = Array.from(new Set(retryable.map((i) => i.file).filter((f): f is string => !!f)));
    if (files.length) return files;
    // No parseable per-file paths: retry exactly what this run targeted. An
    // empty spec list means it WAS a whole-workspace sync.
    if (errors.specs.length) return errors.specs;
    return [h && h.rootPath() ? `${h.rootPath()}/...` : "..."];
  },
  /** Dismiss one entry without touching the file (e.g. keep offline changes and
   *  skip this update). Closes the dialog when it was the last entry. */
  ignoreFile(file: string) {
    if (!errors) return;
    const rest = errors.items.filter((i) => i.file !== file);
    errors = rest.length ? { ...errors, items: rest } : null;
  },
  async fixFile(file: string, force: boolean) {
    if (!h || !errors || busyFile) return;
    if (
      force &&
      !(await h.askConfirm(
        `${file}\n\nForce-overwrite with the depot version? Local changes will be DISCARDED.`,
        "Force overwrite",
        "Overwrite",
      ))
    ) {
      return;
    }
    busyFile = file;
    try {
      const rows = await p4.resync(h.conn(), [file], force);
      sync.clearBlocker(file); // it either synced or the recheck will bring it back
      // A non-force retry refuses files with offline modifications, and a sync
      // that still fails comes back as a "failed" record (see p4_resync) — in
      // both cases keep the item listed with the fresh reason.
      const failed = rows.find((r) => r.action === "failed");
      if (rows.some((r) => r.action === "protected")) {
        errors = {
          ...errors,
          items: errors.items.map((i) =>
            i.file === file
              ? { ...i, line: `${file} — kept: it has offline changes (use Force to overwrite)` }
              : i,
          ),
        };
        h.setNotice("File kept — it has offline changes. Use Force to overwrite.", 6000);
      } else if (failed) {
        errors = {
          ...errors,
          items: errors.items.map((i) =>
            i.file === file ? { ...i, line: failed.message || `${file} — still failing` } : i,
          ),
        };
        h.setNotice("File still failing — see the updated error.", 6000);
      } else {
        errors = { ...errors, items: mark(errors.items, (i) => i.file === file) };
      }
      await h.refresh();
      h.loadPending();
    } catch (e) {
      h.setError(String(e));
    } finally {
      busyFile = null;
    }
  },
  async fixAll(force: boolean) {
    if (!h || !errors || busyFile) return;
    if (
      force &&
      !(await h.askConfirm(
        "Force-overwrite ALL affected files with the depot version?\nLocal changes will be DISCARDED. Conflicts must be resolved separately (p4 resolve / P4V).",
        "Force overwrite all",
        "Overwrite all",
      ))
    ) {
      return;
    }
    const targets = this.targets();
    if (!targets.length) return; // nothing here to re-sync (all of it needs resolving)
    busyFile = "*";
    try {
      const rows = await p4.resync(h.conn(), targets, force);
      // Keep listing what is STILL broken: files excluded for offline changes
      // ("protected", non-force only) and files whose sync failed again
      // ("failed") — only genuinely synced files leave the dialog.
      const still: ErrItem[] = [];
      for (const r of rows) {
        if (r.action === "protected") {
          still.push({
            file: r.depotFile,
            line: `${r.depotFile} — kept: it has offline changes (use Force to overwrite)`,
          });
        } else if (r.action === "failed") {
          still.push({ file: r.depotFile || null, line: r.message || "sync failed" });
        }
      }
      // Everything that was retried and is not back in `still` is settled; the
      // rest of the report (resolves, entries already green) stays as it was.
      const broken = new Map<string | null, ErrItem>();
      for (const s of still) broken.set(s.file, s);
      const retried = new Set(targets);
      const items = errors.items.map((i) => {
        const bad = i.file ? broken.get(i.file) : undefined;
        if (bad) return { ...i, line: bad.line, done: false };
        return i.resolve || i.done || !i.file || !retried.has(i.file) ? i : { ...i, done: true };
      });
      // A failure with no parseable path cannot be matched to a row; list it.
      for (const s of still) if (!s.file) items.push(s);
      errors = { ...errors, items };
      if (still.length) {
        void sync.explainErrors();
        const nKept = still.filter((i) => i.line.includes("offline changes")).length;
        h.setNotice(
          nKept === still.length
            ? `Re-synced; ${nKept} file${nKept === 1 ? "" : "s"} with offline changes kept.`
            : `Re-synced; ${still.length} file${still.length === 1 ? "" : "s"} still need attention.`,
        );
      } else {
        blockers = []; // whatever blocked the sync has been dealt with
        h.setNotice(force ? "Force re-synced the affected files." : "Re-synced the affected files.");
      }
      await h.refresh();
      h.loadPending();
    } catch (e) {
      h.setError(String(e));
    } finally {
      busyFile = null;
    }
  },
};
