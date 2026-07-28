//! Depot-browse persistence: folder-contents + file/folder-history caches,
//! stale-while-revalidate, plus depot→local path mapping used to show synced
//! contents from disk instantly. Backed by the generic `store` (SQLite source
//! of truth + localStorage/memory fast layers). All pure — the client name and
//! workspace roots are passed in, so this has no reactive state of its own.

import { listLocalDir, type P4Record } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";
import { cacheGetSync, cacheGet, cacheSet, cacheClearScope } from "$lib/store.svelte";

export type FolderContents = { dirs: P4Record[]; files: P4Record[] };
export type HistEntry = { mode: "folder" | "file"; subject: string; rows: P4Record[]; have: string };

// Scopes group a client's entries so Refresh can drop them in one call. The
// localStorage keys these produce (`p4tree:<client>:<path>`) match the previous
// format, so any already-cached data is reused.
const treeScope = (client: string) => `p4tree:${client}`;
const histScope = (client: string) => `p4hist:${client}`;

function parse<T>(s: string | null): T | null {
  if (s === null) return null;
  try {
    return JSON.parse(s) as T;
  } catch {
    return null;
  }
}

/** Synchronous folder read (memory/localStorage) for instant paint; null misses
 *  fall to `loadFolderAsync` (SQLite). */
export function loadFolder(client: string, path: string): FolderContents | null {
  return parse<FolderContents>(cacheGetSync(treeScope(client), path));
}
/** Folder read including the SQLite source of truth (for a cold/evicted cache). */
export async function loadFolderAsync(client: string, path: string): Promise<FolderContents | null> {
  return parse<FolderContents>(await cacheGet(treeScope(client), path));
}
export function saveFolder(client: string, path: string, c: FolderContents) {
  cacheSet(treeScope(client), path, JSON.stringify(c));
}

// History entries (scope p4hist:<client>) are written/read by history.svelte.ts
// straight through the store; only the shared HistEntry type + histScope (for the
// Refresh clear below) live here now.

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
