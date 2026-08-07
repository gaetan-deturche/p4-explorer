//! Preferred-file-editor feature store: the detected editors, the user's choice
//! (persisted; defaults to the editor Windows opens .txt with), and the "open
//! in editor" actions — a local path directly, or a server revision via
//! `p4 print` to a temp file (depot/workspace/history/shelved).

import {
  detectEditors,
  defaultEditorId,
  openInEditor,
  p4,
  type EditorInfo,
  type P4Conn,
} from "$lib/p4";
import { localPathFor } from "$lib/cache";
import { cacheGet, cacheSet } from "$lib/store.svelte";

const CHOICE_KEY = "editor"; // store scope `nav`: the chosen editor id
const DIFF_KEY = "difftool"; // store scope `nav`: "inapp" | "external"
const MERGE_KEY = "mergetool"; // same, for the three-way resolve window

let editors = $state<EditorInfo[]>([]);
let chosenId = $state<string>("");
export type DiffTool = "inapp" | "external";
let diffTool = $state<DiffTool>("inapp");
let mergeTool = $state<DiffTool>("inapp");

/** Binary UE assets — text diffs are useless; diff them in Unreal's asset-diff
 *  tool (`UnrealEditor -diff`) instead. */
export function isUnrealAsset(path: string): boolean {
  return /\.(uasset|umap)$/i.test(path);
}

/** A UE asset's object name: its file base name (an editor invariant). */
export function unrealAssetName(depotFile: string): string {
  const base = depotFile.split("/").pop() ?? depotFile;
  return base.replace(/\.[^.]+$/, "");
}

function resolve(): EditorInfo | null {
  return editors.find((e) => e.id === chosenId) ?? editors[0] ?? null;
}

export const editor = {
  /** Detect installed editors and pick the initial choice: the saved one, else
   *  the Windows default, else the first detected (Notepad always exists). */
  async init() {
    // Read AUTHORITATIVELY (SQLite), never cacheGetSync. Two things defeat the
    // synchronous read here: init() runs before the `nav` scope is hydrated, and
    // localStorage is a bounded mirror whose write fails silently once the cache
    // fills (see storeSet) — so the preference lived only in SQLite and every
    // restart answered "nothing saved" and reset the choice to the Windows
    // default.
    const [savedDiff, savedMerge, saved, found] = await Promise.all([
      cacheGet("nav", DIFF_KEY),
      cacheGet("nav", MERGE_KEY),
      cacheGet("nav", CHOICE_KEY),
      detectEditors().catch(() => [] as EditorInfo[]),
    ]);
    diffTool = savedDiff === "external" ? "external" : "inapp";
    mergeTool = savedMerge === "external" ? "external" : "inapp";
    editors = found;
    if (saved && editors.some((e) => e.id === saved)) {
      chosenId = saved;
      return;
    }
    const def = await defaultEditorId().catch(() => "");
    chosenId = editors.some((e) => e.id === def) ? def : (editors[0]?.id ?? "");
  },
  get list() {
    return editors;
  },
  /** The effective editor (chosen, else the first detected), or null if none. */
  get current(): EditorInfo | null {
    return resolve();
  },
  get chosenId() {
    return chosenId;
  },
  setChosen(id: string) {
    chosenId = id;
    cacheSet("nav", CHOICE_KEY, id);
  },
  /** Which tool the double-click "view diff" actions use. */
  get diffTool(): DiffTool {
    return diffTool;
  },
  setDiffTool(t: DiffTool) {
    diffTool = t;
    cacheSet("nav", DIFF_KEY, t);
  },
  /** Which tool settles a three-way conflict ("external" = P4MERGE). */
  get mergeTool(): DiffTool {
    return mergeTool;
  },
  setMergeTool(t: DiffTool) {
    mergeTool = t;
    cacheSet("nav", MERGE_KEY, t);
  },

  /** Open a local file in the preferred editor. */
  async openLocal(path: string): Promise<void> {
    const e = resolve();
    if (!e) throw "No editor detected.";
    await openInEditor(e.path, path);
  },
  /** Open a server revision: p4 print `spec` to a temp file, then open that.
   *  `spec` examples: `//p/f.cpp` (head), `#4`, `@=12345` appended by callers. */
  async openSpec(conn: P4Conn, spec: string): Promise<void> {
    const e = resolve();
    if (!e) throw "No editor detected.";
    const tmp = await p4.printToTemp(conn, spec);
    await openInEditor(e.path, tmp);
  },
  /** Open an opened/local file by its depot path: map it under the workspace
   *  root (instant), falling back to `p4 fstat` for files outside the stream
   *  mapping. */
  async openDepotLocal(
    conn: P4Conn,
    clientRoot: string,
    rootPath: string,
    depotFile: string,
  ): Promise<void> {
    const mapped = localPathFor(clientRoot, rootPath, depotFile);
    if (mapped) {
      await editor.openLocal(mapped);
      return;
    }
    const recs = await p4.fstat(conn, depotFile).catch(() => []);
    const local = recs[0]?.clientFile;
    if (!local) throw "File is not in this workspace.";
    await editor.openLocal(local);
  },
};
