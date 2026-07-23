//! Depot-browse persistence: folder-contents + file/folder-history caches in
//! localStorage (stale-while-revalidate), plus depot→local path mapping used to
//! show synced contents from disk instantly. All pure — the client name and
//! workspace roots are passed in, so this has no reactive state of its own.

import { listLocalDir, type P4Record } from "$lib/p4";
import { makeNode, type TreeNode } from "$lib/tree";

export type FolderContents = { dirs: P4Record[]; files: P4Record[] };
export type HistEntry = { mode: "folder" | "file"; subject: string; rows: P4Record[]; have: string };

const folderKey = (client: string, path: string) => `p4tree:${client}:${path}`;
const histKey = (client: string, id: string) => `p4hist:${client}:${id}`;

export function loadFolder(client: string, path: string): FolderContents | null {
  try {
    const s = localStorage.getItem(folderKey(client, path));
    return s ? (JSON.parse(s) as FolderContents) : null;
  } catch {
    return null;
  }
}

export function saveFolder(client: string, path: string, c: FolderContents) {
  try {
    localStorage.setItem(folderKey(client, path), JSON.stringify(c));
  } catch {
    /* quota / disabled: ignore */
  }
}

export function loadHist(client: string, id: string): HistEntry | null {
  try {
    const s = localStorage.getItem(histKey(client, id));
    return s ? (JSON.parse(s) as HistEntry) : null;
  } catch {
    return null;
  }
}

export function saveHist(client: string, id: string, e: HistEntry) {
  try {
    // Bound persisted size: keep the newest 100 rows.
    localStorage.setItem(histKey(client, id), JSON.stringify({ ...e, rows: e.rows.slice(0, 100) }));
  } catch {
    /* quota / disabled: ignore */
  }
}

/** Drop all persisted folder + history entries for a client (on Refresh). */
export function clearClientCache(client: string) {
  try {
    const prefixes = [`p4tree:${client}:`, `p4hist:${client}:`];
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && prefixes.some((p) => k.startsWith(p))) keys.push(k);
    }
    keys.forEach((k) => localStorage.removeItem(k));
  } catch {
    /* ignore */
  }
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
