//! Pending-changelists feature store: the pending list + all changelist and
//! file actions (submit, review, rename, revert, reopen, new CL) and the
//! file-content providers for PendingList. Shared bits come via `init()`.

import { openUrl } from "@tauri-apps/plugin-opener";
import { p4, type P4Conn, type P4Record, type ReviewInfo } from "$lib/p4";

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  syncing: () => boolean; // shared busy guard
  setSyncing: (v: boolean) => void;
  setNotice: (m: string, ms?: number) => void;
  setError: (m: string) => void;
  askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
  refresh: () => Promise<void>;
};

let h: Hooks | null = null;
let swarmBase = "";
let rows = $state<P4Record[]>([]);
let loading = $state(false);
let version = $state(0); // bumps on every (re)load so views refetch file lists
let reviews = $state<Record<string, ReviewInfo | null>>({}); // change → Swarm review status

export const pending = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get rows() {
    return rows;
  },
  get loading() {
    return loading;
  },
  get version() {
    return version;
  },
  get reviews() {
    return reviews;
  },

  /** Drop the current list (on disconnect / workspace switch). */
  clear() {
    rows = [];
    loading = false;
    reviews = {};
  },

  /** (Re)load the client's pending changelists (Default prepended). */
  async load() {
    if (!h) return;
    if (!h.connected() || !h.conn().client) {
      rows = [];
      loading = false;
      reviews = {};
      return;
    }
    if (rows.length === 0) loading = true; // keep previous list otherwise
    const r = await p4.pending(h.conn(), 100).catch(() => [] as P4Record[]);
    loading = false;
    const def = { change: "default", desc: "", user: h.conn().user, time: "" } as P4Record;
    rows = [def, ...r];
    version++; // signal open changelists to refetch their (now-stale) file lists
    pending.loadReviews(); // fire-and-forget: populate Swarm review badges
  },

  /** Fetch the Swarm review status for every numbered changelist (the review is
   *  linked by change, so we can't pre-filter on a description marker). Builds a
   *  fresh map so statuses for removed changelists are pruned. */
  async loadReviews() {
    if (!h || !h.connected() || !h.conn().client) return;
    const conn = h.conn();
    const targets = rows.filter((r) => r.change !== "default");
    const next: Record<string, ReviewInfo | null> = {};
    await Promise.all(
      targets.map(async (r) => {
        try {
          next[r.change] = await p4.swarmReview(conn, r.change);
        } catch {
          /* leave this CL without a badge */
        }
      }),
    );
    reviews = next;
  },

  /** Run a workspace-mutating action, then reload. `refresh` (default true) also
   *  reloads the depot tree + history; skip it for CL-only moves that change no
   *  synced content. Pending is always reloaded in `finally`, so an optimistic UI
   *  update reconciles with the truth on success AND rolls back on error. */
  async mutate(runFn: () => Promise<unknown>, okNotice: string, opts?: { refresh?: boolean }) {
    if (!h || !h.connected() || h.syncing()) return;
    h.setSyncing(true);
    try {
      await runFn();
      h.setNotice(okNotice);
      if (opts?.refresh !== false) await h.refresh();
    } catch (e) {
      h.setError(String(e));
    } finally {
      pending.load();
      h.setSyncing(false);
    }
  },
  /** As `mutate`, but confirm first. */
  async action(runFn: () => Promise<unknown>, msg: string, title: string, ok: string, note: string) {
    if (!h || !h.connected() || h.syncing()) return;
    if (!(await h.askConfirm(msg, title, ok))) return;
    await pending.mutate(runFn, note);
  },

  submit(change: string) {
    const what = change === "default" ? "the default changelist" : `changelist @${change}`;
    pending.action(
      () => p4.submit(h!.conn(), change),
      `Submit ${what}?\nThis commits the files to the depot and cannot be undone.`,
      "Submit changelist",
      "Submit",
      "Changelist submitted.",
    );
  },
  requestReview(change: string) {
    pending.action(
      () => p4.requestReview(h!.conn(), change),
      `Request a Swarm review for @${change}?\nThis adds #review to the description and shelves the files.`,
      "Request review",
      "Request",
      "Review requested.",
    );
  },
  updateReview(change: string) {
    pending.action(
      () => p4.shelveUpdate(h!.conn(), change),
      `Update the review for @${change} by re-shelving its files?`,
      "Update review",
      "Update",
      "Review updated.",
    );
  },
  deleteShelf(change: string) {
    pending.action(
      () => p4.shelveDelete(h!.conn(), change),
      `Delete the shelved files of @${change}?`,
      "Delete shelf",
      "Delete",
      "Shelf deleted.",
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
  revert(file: string) {
    pending.action(
      () => p4.revert(h!.conn(), file),
      `${file}\n\nRevert this file? Your local changes will be discarded.`,
      "Revert file",
      "Revert",
      "File reverted.",
    );
  },
  revertKeep(file: string) {
    pending.action(
      () => p4.revertKeep(h!.conn(), file),
      `${file}\n\nRemove from its changelist but keep your local edits on disk?`,
      "Remove from changelist",
      "Remove",
      "File removed from changelist (changes kept).",
    );
  },
  reopen(file: string, change: string) {
    const label = change === "default" ? "Default" : "@" + change;
    // CL move only — no synced content changes, so skip the tree/history refresh.
    pending.mutate(() => p4.reopen(h!.conn(), file, change), `Moved to ${label}.`, { refresh: false });
  },
  moveToNew(file: string, desc: string) {
    pending.mutate(
      async () => {
        const ch = await p4.newChangelist(h!.conn(), desc);
        await p4.reopen(h!.conn(), file, ch);
      },
      "Moved to a new changelist.",
      { refresh: false },
    );
  },
  rename(change: string, desc: string) {
    pending.mutate(() => p4.setDescription(h!.conn(), change, desc), "Changelist renamed.");
  },

  // --- file-content providers for PendingList (no `this`; safe as callbacks) --
  localFiles(change: string): Promise<P4Record[]> {
    return p4.opened(h!.conn(), change).catch(() => [] as P4Record[]);
  },
  shelvedFiles(change: string): Promise<P4Record[]> {
    return change === "default"
      ? Promise.resolve([] as P4Record[])
      : p4.describeShelved(h!.conn(), change).catch(() => [] as P4Record[]);
  },
  localDiff(file: string): Promise<string> {
    return p4.diffLocal(h!.conn(), file);
  },
  shelvedDiff(file: string, rev: number, change: string): Promise<string> {
    return p4.diffShelved(h!.conn(), file, rev, change);
  },
  async openLocalDiff(file: string) {
    try {
      await p4.openDiffLocal(h!.conn(), file);
    } catch (e) {
      h!.setNotice(String(e), 5000);
    }
  },
  async openShelvedDiff(file: string, rev: number, change: string) {
    try {
      await p4.openDiffShelved(h!.conn(), file, rev, change);
    } catch (e) {
      h!.setNotice(String(e), 5000);
    }
  },
};
