//! Starting a three-way resolve: prepare the merge in Rust, then hand it to the
//! in-app resolve window or to P4MERGE depending on the user's preference
//! (Options → Merge / resolve), mirroring how the diff tool is chosen.

import { invoke } from "@tauri-apps/api/core";
import { p4, type P4Conn } from "$lib/p4";
import { editor } from "$lib/editor.svelte";

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  setNotice: (m: string, ms?: number) => void;
  setError: (m: string) => void;
  refresh: () => Promise<void>;
  loadPending: () => void;
};

let h: Hooks | null = null;

export const merges = {
  init(hooks: Hooks) {
    h = hooks;
  },

  /** Resolve what p4 is holding on `depotFile`. */
  async resolveFile(depotFile: string) {
    if (!h || !h.connected()) return;
    const name = depotFile.split("/").pop() || depotFile;
    try {
      await hand(await p4.mergeStartResolve(h.conn(), depotFile), name);
    } catch (e) {
      h.setError(String(e));
    }
  },

  /** Resolve one hunk a patch could not place. */
  async resolvePatchHunk(patchPath: string, depotFile: string, hunkIndex: number) {
    if (!h || !h.connected()) return;
    const name = `${depotFile.split("/").pop() || depotFile} #${hunkIndex}`;
    try {
      await hand(await p4.mergeStartPatch(h.conn(), patchPath, depotFile, hunkIndex), name);
    } catch (e) {
      h.setError(String(e));
    }
  },
};

/** Route a prepared merge to whichever tool the user prefers. */
async function hand(id: string, name: string) {
  if (!h) return;
  if (editor.mergeTool === "external") {
    h.setNotice("Waiting for P4MERGE…", 6000);
    const r = await p4.mergeExternal(id);
    if (r === "cancelled" || r === "unchanged") {
      h.setNotice("P4MERGE closed without a decision — nothing changed.", 6000);
      return;
    }
    h.setNotice(r === "resolved" ? `Resolved ${name}.` : `Merged ${name}.`, 6000);
    await afterMerge();
    return;
  }
  await invoke<void>("open_merge_window", { id, name });
}

/** Pick up the new state once a merge has been written. */
export async function afterMerge() {
  if (!h) return;
  await h.refresh();
  h.loadPending();
}
