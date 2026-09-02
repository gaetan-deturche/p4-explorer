//! Stashes: a change set aside as a patch in the app's own database.
//!
//! The store is deliberately thin. Taking one is a single backend call, and
//! applying one is not implemented here at all — a stash is a patch, so it goes
//! through `patches.previewStash` into the ordinary apply dialog, which already
//! knows how to preview, place fuzzy hunks, choose an end state and resolve what
//! does not fit.
//!
//! The list is machine-wide on purpose: a stash taken in one workspace is meant
//! to be applied in another, so the rows carry the workspace they came from
//! rather than being filtered down to the current one.

import { p4, type StashRow } from "$lib/p4";

type Hooks = {
  conn: () => import("$lib/p4").P4Conn;
  connected: () => boolean;
  setNotice: (m: string, ms?: number) => void;
  setError: (m: string) => void;
  askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
  /** Repaint after the workspace changed under us (a stash that cleared it). */
  refresh: () => void;
};

let h: Hooks | null = null;
let rows = $state<StashRow[]>([]);
let loading = $state(false);
let loaded = false;

export const stashes = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get rows() {
    return rows;
  },
  get loading() {
    return loading;
  },
  async load() {
    if (!h) return;
    loading = !loaded; // only the first load is allowed to show a spinner
    try {
      rows = await p4.stashList();
      loaded = true;
    } catch (e) {
      h.setError(String(e));
    } finally {
      loading = false;
    }
  },

  /** Take a stash from a changelist (files = []) or an explicit file set.
   *
   *  `clear` is the git-stash move: put the change away AND take it out of the
   *  workspace. It reverts only what the patch actually CARRIED — anything it
   *  could not is left open, because reverting that would throw away work the
   *  stash cannot give back. And it runs only after the
   *  stash is safely written: the order is what makes it non-destructive. */
  async save(name: string, change: string, files: string[], clear = false) {
    if (!h || !h.connected()) return;
    let row;
    try {
      row = await p4.stashSave(h.conn(), name, change, files);
      await stashes.load();
    } catch (e) {
      h.setError(String(e));
      return;
    }
    const n = row.files.length;
    h.setNotice(`Stashed ${n} file${n === 1 ? "" : "s"} as “${name}”.`, 6000);
    if (!clear || !n) return;
    try {
      const results = await p4.revertLocal(
        h.conn(),
        row.files.map((f) => f.depotFile),
      );
      const failed = results.filter((r) => !r.ok);
      const kept = row.skipped.length; // anything the patch could not carry
      if (failed.length) {
        h.setError(
          `Stashed, but ${failed.length} file${failed.length === 1 ? "" : "s"} could not be reverted:\n` +
            failed.map((r) => `${r.file.split("/").pop()} — ${r.message}`).join("\n"),
        );
      } else {
        h.setNotice(
          `Stashed and reverted ${n} file${n === 1 ? "" : "s"}.` +
            (kept ? ` ${kept} file${kept === 1 ? "" : "s"} the patch could not carry left open.` : ""),
          kept ? 0 : 6000,
        );
      }
      h.refresh();
    } catch (e) {
      h.setError(String(e));
    }
  },

  async rename(id: number, name: string) {
    if (!h || !name.trim()) return;
    try {
      await p4.stashRename(id, name);
      await stashes.load();
    } catch (e) {
      h.setError(String(e));
    }
  },

  /** Delete a stash. The patch exists nowhere else, so this asks first. */
  async remove(row: StashRow) {
    if (!h) return;
    const ok = await h.askConfirm(
      `“${row.name}”\n\nDelete this stash? It holds ${row.files.length} file${
        row.files.length === 1 ? "" : "s"
      } and exists nowhere else — deleting it cannot be undone.`,
      "Delete stash",
      "Delete",
    );
    if (!ok) return;
    try {
      await p4.stashDelete(row.id);
      await stashes.load();
      h.setNotice("Stash deleted.");
    } catch (e) {
      h.setError(String(e));
    }
  },
};
