//! Applying a `.patch` onto the workspace: pick a file, preview what it would
//! do, let the user choose the end state, apply, then report. The preview and
//! the apply run the same Rust pipeline, so the dialog can't promise one thing
//! and the apply do another.

import { p4, type P4Conn, type PatchFileReport } from "$lib/p4";

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  setNotice: (m: string, ms?: number) => void;
  setError: (m: string) => void;
  refresh: () => Promise<void>;
  loadPending: () => void;
  scanOffline: () => Promise<boolean>;
};

/** "preview" while the user decides, "done" once applied. */
type Phase = "preview" | "applying" | "done";

let h: Hooks | null = null;
let path = $state("");
let phase = $state<Phase>("preview");
let files = $state<PatchFileReport[]>([]);
let busy = $state(false);
let open = $state(false);

export const patches = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get open() {
    return open;
  },
  get path() {
    return path;
  },
  get phase() {
    return phase;
  },
  get files() {
    return files;
  },
  get busy() {
    return busy;
  },
  close() {
    open = false;
    files = [];
    path = "";
    phase = "preview";
  },

  /** Prompt for a .patch, dry-run it, and open the dialog with the outcome. */
  async pickAndPreview() {
    if (!h || !h.connected()) return;
    busy = true;
    try {
      const picked = await p4.pickPatchFile();
      if (!picked) return;
      files = await p4.previewPatch(h.conn(), picked);
      path = picked;
      phase = "preview";
      open = true;
    } catch (e) {
      h.setError(String(e));
    } finally {
      busy = false;
    }
  },

  /**
   * Apply the previewed patch. `mode` "edit" opens each target for edit so the
   * result lands in a pending changelist; "offline" only writes to disk and
   * leaves the files to the Offline section. `partial` lets a file take the
   * hunks that fit, rejecting the rest to a `.rej`.
   */
  async apply(mode: "edit" | "offline", partial: boolean) {
    if (!h || !path) return;
    busy = true;
    phase = "applying";
    try {
      const rep = await p4.applyPatch(h.conn(), path, mode, "", partial);
      files = rep;
      phase = "done";
      const ok = rep.filter((f) => f.applied > 0).length;
      const bad = rep.filter((f) => f.conflicts > 0).length;
      h.setNotice(
        bad > 0
          ? `Patch applied to ${ok} file(s); ${bad} with conflicts.`
          : `Patch applied to ${ok} file(s).`,
        8000,
      );
      // Reflect the new state: opened files show up in pending, disk-only edits
      // in the Offline section.
      if (ok > 0) {
        if (mode === "edit") {
          await h.refresh();
          h.loadPending();
        } else {
          void h.scanOffline();
        }
      }
    } catch (e) {
      phase = "preview";
      h.setError(String(e));
    } finally {
      busy = false;
    }
  },
};
