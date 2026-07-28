//! Depot-browse helpers: shared cache types, tree-node building, and depot→local
//! path mapping used to show synced contents from disk instantly. All pure — the
//! client name and workspace roots are passed in, so this has no reactive state.
//! Folder-contents and history are written/read by browse/history straight
//! through the store now; only the shared types + the Refresh clear live here.

import { listLocalDir, type P4Record } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";
import { cacheClearScope } from "$lib/store.svelte";

export type FolderContents = { dirs: P4Record[]; files: P4Record[] };
export type HistEntry = { mode: "folder" | "file"; subject: string; rows: P4Record[]; have: string };

// Scopes group a client's entries so Refresh can drop them in one call. These MUST
// match the scope strings browse/history write to (`p4tree:<client>`, `p4hist:<client>`).
const treeScope = (client: string) => `p4tree:${client}`;
const histScope = (client: string) => `p4hist:${client}`;

/** Drop all persisted folder + history entries for a client (on Refresh). */
export function clearClientCache(client: string) {
  cacheClearScope(treeScope(client));
  cacheClearScope(histScope(client));
}

export function buildChildren(c: FolderContents): TreeNode[] {
  const dirNodes = c.dirs.filter((d) => d.dir).map((d) => makeNode(d.dir, true));
  const fileNodes = c.files.filter((f) => f.depotFile).map((f) => makeNode(f.depotFile, false, f));
  return [...dirNodes, ...fileNodes];
}

/** Map a depot path under the current stream to its local workspace path. */
export function localPathFor(clientRoot: string, rootPath: string, depotPath: string): string | null {
  if (!clientRoot || !rootPath) return null;
  if (depotPath === rootPath) return clientRoot;
  if (!depotPath.startsWith(rootPath + "/")) return null;
  const rel = depotPath.slice(rootPath.length + 1).split("/").join("\\");
  return `${clientRoot}\\${rel}`;
}

/** Provisional folder contents read from the local filesystem (instant view). */
export async function localChildren(
  clientRoot: string,
  rootPath: string,
  path: string,
): Promise<FolderContents | null> {
  const lp = localPathFor(clientRoot, rootPath, path);
  if (!lp) return null;
  try {
    const ld = await listLocalDir(lp);
    return {
      dirs: ld.dirs.map((n) => ({ dir: `${path}/${n}` }) as P4Record),
      files: ld.files.map((n) => ({ depotFile: `${path}/${n}` }) as P4Record),
    };
  } catch {
    return null;
  }
}
