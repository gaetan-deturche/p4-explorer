import type { P4Record } from "$lib/p4";

/** A node in the depot tree. Directories load their children lazily on expand. */
export interface TreeNode {
  path: string; // depot path
  name: string; // last segment
  isDir: boolean;
  rec?: P4Record; // fstat record for a file (drives the sync marker)
  expanded: boolean;
  loaded: boolean; // children fetched from the server at least once
  loading: boolean;
  children: TreeNode[];
}

export function makeNode(path: string, isDir: boolean, rec?: P4Record): TreeNode {
  const clean = path.replace(/\/+$/, "");
  const i = clean.lastIndexOf("/");
  return {
    path: clean,
    name: i >= 0 ? clean.slice(i + 1) : clean,
    isDir,
    rec,
    expanded: false,
    loaded: false,
    loading: false,
    children: [],
  };
}
