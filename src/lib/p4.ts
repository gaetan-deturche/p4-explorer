import { invoke } from "@tauri-apps/api/core";

/** A tagged p4 output record: field -> value (values are strings from p4). */
export type P4Record = Record<string, string>;

/** Connection context. Empty fields fall back to the ambient p4 environment. */
export interface P4Conn {
  port: string;
  user: string;
  client: string;
  cwd: string;
}

export function emptyConn(): P4Conn {
  return { port: "", user: "", client: "", cwd: "" };
}

/** Local SQLite file index for fuzzy search. */
export const idx = {
  status: (client: string) => invoke<number>("index_status", { client }),
  build: (conn: P4Conn, client: string, root: string) =>
    invoke<number>("index_build", { conn, client, root }),
  search: (client: string, query: string, max = 200) =>
    invoke<string[]>("index_search", { client, query, max }),
};

export interface LocalDir {
  dirs: string[];
  files: string[];
}

/** List a local filesystem directory (names only). */
export function listLocalDir(path: string): Promise<LocalDir> {
  return invoke<LocalDir>("list_local_dir", { path });
}

/** True only for tagged release builds (dev/local builds skip the update check). */
export function isReleaseBuild(): Promise<boolean> {
  return invoke<boolean>("is_release_build");
}

async function call(cmd: string, args: Record<string, unknown>): Promise<P4Record[]> {
  return await invoke<P4Record[]>(cmd, args);
}

export const p4 = {
  info: (conn: P4Conn) => call("p4_info", { conn }),
  clients: (conn: P4Conn) => call("p4_clients", { conn }),
  dirs: (conn: P4Conn, path: string) => call("p4_dirs", { conn, path }),
  files: (conn: P4Conn, path: string) => call("p4_files", { conn, path }),
  changes: (conn: P4Conn, path: string, max = 50, before?: number) =>
    call("p4_changes", { conn, path, max, before: before ?? null }),
  pending: (conn: P4Conn, max = 50) => call("p4_pending", { conn, max }),
  haveChange: (conn: P4Conn, path: string) => call("p4_have_change", { conn, path }),
  describe: (conn: P4Conn, change: string) => call("p4_describe", { conn, change }),
  filelog: (conn: P4Conn, file: string, max = 100) => call("p4_filelog", { conn, file, max }),
  fstat: (conn: P4Conn, file: string) => call("p4_fstat", { conn, file }),
  sync: (conn: P4Conn, path?: string) => call("p4_sync", { conn, path: path ?? null }),
  reconcile: (conn: P4Conn, path: string) => call("p4_reconcile", { conn, path }),
  syncStream: (conn: P4Conn, path?: string) =>
    invoke<number>("p4_sync_stream", { conn, path: path ?? null }),
  syncCancel: () => invoke<void>("sync_cancel"),
  search: (conn: P4Conn, root: string, term: string, max = 300) =>
    call("p4_search", { conn, root, term, max }),
  diff2: (conn: P4Conn, depotFile: string, rev: number) =>
    invoke<string>("p4_diff2", { conn, depotFile, rev }),
  openDiff: (conn: P4Conn, depotFile: string, rev: number) =>
    invoke<void>("open_diff", { conn, depotFile, rev }),
  describeShelved: (conn: P4Conn, change: string) => call("p4_describe_shelved", { conn, change }),
  diffShelved: (conn: P4Conn, depotFile: string, rev: number, change: string) =>
    invoke<string>("p4_diff_shelved", { conn, depotFile, rev, change }),
  openDiffShelved: (conn: P4Conn, depotFile: string, rev: number, change: string) =>
    invoke<void>("open_diff_shelved", { conn, depotFile, rev, change }),
  streams: (conn: P4Conn) => call("p4_streams", { conn }),
  depots: (conn: P4Conn) => call("p4_depots", { conn }),
  switch: (conn: P4Conn, stream: string) => call("p4_switch", { conn, stream }),
  submit: (conn: P4Conn, change: string) => call("p4_submit", { conn, change }),
  shelveDelete: (conn: P4Conn, change: string) => call("p4_shelve_delete", { conn, change }),
  shelveUpdate: (conn: P4Conn, change: string) => call("p4_shelve", { conn, change }),
  requestReview: (conn: P4Conn, change: string) =>
    invoke<void>("p4_request_review", { conn, change }),
  swarmUrl: (conn: P4Conn) => invoke<string>("swarm_url", { conn }),
  opened: (conn: P4Conn, change: string) => call("p4_opened", { conn, change }),
  diffLocal: (conn: P4Conn, depotFile: string) =>
    invoke<string>("p4_diff_local", { conn, depotFile }),
  openDiffLocal: (conn: P4Conn, depotFile: string) =>
    invoke<void>("open_diff_local", { conn, depotFile }),
  revert: (conn: P4Conn, depotFile: string) => call("p4_revert", { conn, depotFile }),
  revertKeep: (conn: P4Conn, depotFile: string) => call("p4_revert_keep", { conn, depotFile }),
  reopen: (conn: P4Conn, depotFile: string, change: string) =>
    call("p4_reopen", { conn, depotFile, change }),
  newChangelist: (conn: P4Conn, description: string) =>
    invoke<string>("p4_new_changelist", { conn, description }),
  envPort: (conn: P4Conn) => invoke<string>("p4_env_port", { conn }),
  setDescription: (conn: P4Conn, change: string, description: string) =>
    invoke<void>("p4_set_description", { conn, change, description }),
};

/** Last path segment of a depot path. */
export function baseName(path: string): string {
  const p = path.replace(/\/+$/, "");
  const i = p.lastIndexOf("/");
  return i >= 0 ? p.slice(i + 1) : p;
}

/** Format a p4 unix-epoch-seconds string as a local datetime. */
export function fmtTime(epoch: string | undefined): string {
  if (!epoch) return "";
  const n = Number(epoch);
  if (!Number.isFinite(n)) return epoch;
  return new Date(n * 1000).toLocaleString();
}

/** First line of a (possibly multi-line) changelist description. */
export function firstLine(desc: string | undefined): string {
  if (!desc) return "";
  return desc.split("\n")[0].trim();
}
