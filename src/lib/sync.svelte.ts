//! Sync feature store: streaming global sync, "update to changelist", reconcile,
//! and the per-file error report + fixes. Owns its own progress/error state;
//! shared bits (conn, busy flags, notices, refresh/reload) come via `init()`.

import { listen } from "@tauri-apps/api/event";
import { p4, type P4Conn } from "$lib/p4";

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
type ErrItem = { line: string; file: string | null };
type ErrReport = { title: string; items: ErrItem[]; path: string | undefined };

let h: Hooks | null = null;
let cancelled = false;
let errorItems: ErrItem[] = [];

let progress = $state<Progress | null>(null);
let errors = $state<ErrReport | null>(null);
let busyFile = $state<string | null>(null); // file being fixed ("*" = all)

export const sync = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get progress() {
    return progress;
  },
  get errors() {
    return errors;
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
  async run(title: string, path: string | undefined): Promise<number | null> {
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
      const n = await p4.syncStream(h.conn(), path);
      progress = null;
      if (errorItems.length > 0) errors = { title, items: [...errorItems], path };
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
    h.setSyncing(true);
    try {
      const n = await this.run("Sync workspace", undefined);
      if (n !== null) {
        await h.refresh();
        h.loadPending(); // offline state changed — refresh list + rescan
      }
    } finally {
      h.setSyncing(false);
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
    h.setSyncing(true);
    try {
      const n = await this.run(`Update to @${change}`, spec);
      if (n !== null) {
        await h.refresh();
        h.loadPending();
      }
    } finally {
      h.setSyncing(false);
    }
  },

  /** Sync a single depot path (file or folder) to the latest revision. */
  async syncPath(path: string, isDir: boolean) {
    if (!h || !h.connected() || h.busy()) return;
    const spec = isDir ? `${path}/...` : path;
    const name = path.replace(/\/+$/, "").split("/").pop() || path;
    h.setSyncing(true);
    try {
      const n = await this.run(`Sync ${name}`, spec);
      if (n !== null) {
        await h.refresh();
        h.loadPending();
      }
    } finally {
      h.setSyncing(false);
    }
  },

  /** Reconcile offline work under a single depot path (file or folder). */
  async reconcilePath(path: string, isDir: boolean) {
    if (!h || !h.connected() || h.busy()) return;
    const spec = isDir ? `${path}/...` : path;
    if (
      !(await h.askConfirm(
        `${spec}\n\nReconcile offline work under this path? This opens files changed, added, or deleted outside Perforce into the default changelist.`,
        "Reconcile offline work",
        "Reconcile",
      ))
    ) {
      return;
    }
    h.setReconciling(true);
    try {
      const rows = await p4.reconcile(h.conn(), spec);
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
    const files = Array.from(new Set(errors.items.map((i) => i.file).filter((f): f is string => !!f)));
    return files.length ? files : [errors.path ?? (h && h.rootPath() ? `${h!.rootPath()}/...` : "...")];
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
        const rest = errors.items.filter((i) => i.file !== file);
        errors = rest.length ? { ...errors, items: rest } : null;
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
      if (still.length) {
        errors = { ...errors, items: still };
        const nKept = still.filter((i) => i.line.includes("offline changes")).length;
        h.setNotice(
          nKept === still.length
            ? `Re-synced; ${nKept} file${nKept === 1 ? "" : "s"} with offline changes kept.`
            : `Re-synced; ${still.length} file${still.length === 1 ? "" : "s"} still need attention.`,
        );
      } else {
        errors = null;
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
