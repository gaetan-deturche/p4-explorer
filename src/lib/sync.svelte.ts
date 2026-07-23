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
        "Global sync",
        "Sync",
      ))
    ) {
      return;
    }
    h.setSyncing(true);
    try {
      const n = await this.run("Global sync", undefined);
      if (n !== null) await h.refresh();
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
      if (n !== null) await h.refresh();
    } finally {
      h.setSyncing(false);
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
      await p4.resync(h.conn(), [file], force);
      const rest = errors.items.filter((i) => i.file !== file);
      errors = rest.length ? { ...errors, items: rest } : null;
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
      await p4.resync(h.conn(), targets, force);
      errors = null;
      h.setNotice(force ? "Force re-synced the affected files." : "Re-synced the affected files.");
      await h.refresh();
      h.loadPending();
    } catch (e) {
      h.setError(String(e));
    } finally {
      busyFile = null;
    }
  },
};
