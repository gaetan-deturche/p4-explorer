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
import { cacheGetSync, cacheSet } from "$lib/store.svelte";

const CHOICE_KEY = "editor"; // store scope `nav`: the chosen editor id

let editors = $state<EditorInfo[]>([]);
let chosenId = $state<string>("");

function resolve(): EditorInfo | null {
  return editors.find((e) => e.id === chosenId) ?? editors[0] ?? null;
}

export const editor = {
  /** Detect installed editors and pick the initial choice: the saved one, else
   *  the Windows default, else the first detected (Notepad always exists). */
  async init() {
    editors = await detectEditors().catch(() => []);
    const saved = cacheGetSync("nav", CHOICE_KEY) ?? "";
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
