//! Applying a `.patch` onto the workspace: pick a file, preview what it would
//! do, let the user choose the end state, apply, then report. The preview and
//! the apply run the same Rust pipeline, so the dialog can't promise one thing
//! and the apply do another.

import { p4, type CopyResult, type P4Conn, type PatchFileReport } from "$lib/p4";

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
// Set when the patch came from a review rather than a file on disk: the dialog
// titles itself with the review instead of a temp filename, and lists what the
// shelf carried that a patch cannot.
let subject = $state("");
let skipped = $state<string[]>([]);
let reviewChange = ""; // set when the source is a review; drives the verbatim copy

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
  get subject() {
    return subject;
  },
  get skipped() {
    return skipped;
  },
  close() {
    open = false;
    files = [];
    path = "";
    phase = "preview";
    subject = "";
    skipped = [];
    reviewChange = "";
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
      subject = "";
      skipped = [];
      reviewChange = "";
      phase = "preview";
      open = true;
    } catch (e) {
      h.setError(String(e));
    } finally {
      busy = false;
    }
  },

  /** Preview a Swarm review's shelved content as a patch on this workspace.
   *  The shelf is written out as a real patch file, so from here on this is the
   *  ordinary apply-patch flow — same preview, same end-state choice, same
   *  per-hunk three-way resolve for whatever doesn't fit. */
  async previewReview(change: string, label: string) {
    if (!h || !h.connected()) return;
    busy = true;
    try {
      const rp = await p4.reviewPatch(h.conn(), change);
      files = await p4.previewPatch(h.conn(), rp.path);
      path = rp.path;
      subject = label;
      skipped = rp.skipped;
      reviewChange = change;
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
      // Binaries and adds carry no diff, so they are copied from the shelf
      // instead — otherwise "apply this review" would silently skip most of a
      // content changelist.
      let copied = 0;
      let copyFailed: string[] = [];
      if (reviewChange && skipped.length) {
        const res = await p4
          .reviewCopyFiles(h.conn(), reviewChange, skipped, mode)
          .catch(() => [] as CopyResult[]);
        copied = res.filter((r) => r.status === "copied" || r.status === "opened").length;
        copyFailed = res
          .filter((r) => r.status === "failed" || r.status === "skipped")
          .map((r) => `${r.depot.split("/").pop()}: ${r.message}`);
        if (copyFailed.length) h.setError(`Could not take ${copyFailed.length} file(s) — ${copyFailed[0]}`);
      }
      const parts = [`${ok} patched`];
      if (copied) parts.push(`${copied} copied`);
      if (bad) parts.push(`${bad} with conflicts`);
      h.setNotice(`${subject || "Patch"}: ${parts.join(", ")}.`, 8000);
      // Reflect the new state: opened files show up in pending, disk-only edits
      // in the Offline section.
      if (ok > 0 || copied > 0) {
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
