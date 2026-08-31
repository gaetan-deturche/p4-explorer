import { invoke } from "@tauri-apps/api/core";
import { safe } from "$lib/safe.svelte";

/** A tagged p4 output record: field -> value (values are strings from p4). */
export type P4Record = Record<string, string>;

/** Connection context. Empty fields fall back to the ambient p4 environment. */
export interface P4Conn {
  port: string;
  user: string;
  client: string;
  cwd: string;
  charset: string; // "" ambient, "none" (force non-unicode), or e.g. "utf8"
  // Explicit ticket (-P), set only when p4's own lookup fails despite a valid
  // ticket (multi-edge auth.id keying — see connection.svelte.ts).
  ticket: string;
}

export function emptyConn(): P4Conn {
  return { port: "", user: "", client: "", cwd: "", charset: "", ticket: "" };
}

/** Result of a file-index search: literal matches plus fuzzy suggestions. */
export interface SearchHits {
  contains: string[];
  fuzzy: string[];
}

/** Local SQLite file index for file search. */
export const idx = {
  status: (client: string) => invoke<number>("index_status", { client }),
  build: (conn: P4Conn, client: string, root: string) =>
    invoke<number>("index_build", { conn, client, root }),
  buildDepot: (conn: P4Conn, key: string) => invoke<number>("index_build_depot", { conn, key }),
  buildLocal: (key: string, root: string, rootPath: string) =>
    invoke<number>("index_build_local", { key, root, rootPath }),
  /** Index search: `contains` = literal case-insensitive substring matches (what
   *  the file view filters on), `fuzzy` = ranked subsequence matches (suggestions). */
  search: (client: string, query: string, max = 200) =>
    invoke<SearchHits>("index_search", { client, query, max }),
};

export interface LocalDir {
  dirs: string[];
  files: string[];
}

/** One row of the Swarm review list. */
export interface ReviewRow {
  /** "review" for a Swarm review, "shelf" for a shelved changelist with no
   *  review at all — the latter has no state, reviewers or Swarm page. */
  kind: "review" | "shelf";
  id: number;
  state: string;
  stateLabel: string;
  author: string;
  description: string;
  created: number;
  updated: number;
  /** Changelist holding the current shelf: what to describe/diff/apply. Swarm's
   *  list endpoint returns no version list, and the review id is itself the
   *  `swarm`-owned changelist tracking the latest version. */
  change: number;
  pending: boolean;
  /** Submitted changelists this review landed as. Often non-empty while the
   *  review still says needsReview: submitting deletes the shelf, so this is
   *  where the content lives when someone pushed without waiting for approval. */
  commits: number[];
  reviewers: string[];
}

/** A page of reviews. `error` is set instead of throwing, so the tab can say
 *  why it is empty rather than showing a bare "no reviews". */
export interface ReviewPage {
  reviews: ReviewRow[];
  lastSeen: number; // cursor for the next page; 0 = end
  error: string;
}

/** What to ask Swarm for. `role` decides whether `user` is the author or a
 *  participant (Swarm ANDs unrelated filters, so it has to be one or the other). */
export interface ReviewQuery {
  states: string[];
  user: string;
  role: "author" | "reviewer" | "any";
  keywords: string;
  max: number;
  after: number;
  /** Current stream (`//Curiosity/main`); only reviews on it. "" = every depot. */
  streamPath: string;
  /** Drop reviews that already went in (Swarm leaves those at needsReview). */
  hideSubmitted: boolean;
}

/** A review's shelf written out as a patch, plus what it could not carry.
 *  `skipped` files are copied verbatim instead (binaries and adds). */
export interface ReviewPatch {
  path: string;
  files: number;
  skipped: string[];
}

/** What happened to one file copied straight from a review's shelf. */
export interface CopyResult {
  depot: string;
  local: string;
  action: string;
  status: "copied" | "opened" | "skipped" | "failed";
  message: string;
}

/** Where one hunk of a patch landed, or why it didn't. */
export interface PatchHunkReport {
  index: number;
  status: "clean" | "fuzz" | "already" | "conflict";
  line: number;
  offset: number; // lines away from the position recorded in the patch
}

/** Per-file outcome of previewing or applying a patch. */
export interface PatchFileReport {
  depot: string;
  local: string;
  status: "clean" | "fuzz" | "already" | "partial" | "conflict" | "missing" | "notext" | "binary";
  hunks: PatchHunkReport[];
  applied: number;
  conflicts: number;
  message: string;
  rejPath: string;
}

/** One stretch of a three-way merge. `same` needs no decision; `conflict` does. */
export type MergeRegion =
  | { kind: "same"; lines: string[] }
  | { kind: "ours"; base: string[]; lines: string[] }
  | { kind: "theirs"; base: string[]; lines: string[] }
  | { kind: "both"; base: string[]; lines: string[] }
  | { kind: "conflict"; base: string[]; ours: string[]; theirs: string[] };

/** A prepared three-way merge, as the resolve window receives it. */
export interface MergeData {
  id: string;
  kind: "resolve" | "patch";
  name: string;
  target: string;
  baseLabel: string;
  theirsLabel: string;
  yoursLabel: string;
  regions: MergeRegion[];
  conflicts: number;
}

/** A changelist's Swarm review status (id 0 = requested, not yet created). */
/** One review's participants, versions and state — the review WINDOW's subject,
 *  where ReviewRow is a row in the list. */
export interface ReviewDetail {
  id: number;
  state: string;
  stateLabel: string;
  author: string;
  description: string;
  updated: number;
  testStatus: string;
  reviewers: Reviewer[];
  versions: ReviewVersion[];
  changes: string[];
  commits: string[];
}
export interface Reviewer {
  user: string;
  /** 1 = up, -1 = down, 0 = has not voted. */
  vote: number;
  votedVersion: number;
  /** Their vote is older than the current version, so it no longer stands for
   *  what is on the review now. */
  stale: boolean;
  required: boolean;
  isAuthor: boolean;
}
export interface ReviewVersion {
  n: number; // 1-based, as Swarm numbers them
  change: string; // the changelist holding THIS version's files
  pending: boolean; // shelved (true) or submitted
  user: string;
  time: number;
}
/** One user on the server. */
export interface UserRow {
  user: string;
  fullName: string;
  email: string;
}

/** One Swarm comment on a review. Anchored ones carry the file, the version and
 *  the line they were written against. */
export interface Comment {
  id: number;
  body: string;
  user: string;
  time: number;
  updated: number;
  edited: boolean;
  /** "comment" | "open" | "addressed" | "verified" — Swarm's task workflow. */
  taskState: string;
  /** Swarm's `closed` flag: the thread is archived. */
  closed: boolean;
  file: string; // "" for a comment on the review as a whole
  version: number;
  leftLine: number;
  rightLine: number;
  /** The diff lines Swarm stored with the anchor, prefixes included. */
  content: string[];
  parent: number; // the comment this replies to; 0 = a new thread
  /** Nothing but the Unreal plugin's own bookkeeping — not a comment a person
   *  wrote, and hidden everywhere. */
  bookkeeping: boolean;
  assetFile: string;
  assetCategory: string;
}
/** Where to attach a new comment. */
export interface CommentAnchor {
  file: string;
  version: number;
  leftLine: number;
  rightLine: number;
  content: string[];
  parent: number;
}
/** Which snapshot to read a file at: a version, or the depot revisions that
 *  version was written against. */
export interface VersionRef {
  change: string;
  pending: boolean;
  label: string;
  /** Read each file's base revision instead of the version's own content — the
   *  comparison point that answers "what does this version change?". */
  base: boolean;
}
/** What this user may do to a review, straight from Swarm — the window offers
 *  these and nothing else, rather than guessing at permissions. */
export interface Transitions {
  items: { key: string; label: string }[];
  blocked: string[];
}
/** One file, compared between two versions of a review. */
export interface VersionFile {
  depotFile: string;
  status: "changed" | "same" | "added" | "removed";
  sizeA: number;
  sizeB: number;
}
export interface ReviewInfo {
  id: number;
  state: string; // needsReview | needsRevision | approved | rejected | archived | requested
  stateLabel: string;
}

/** List a local filesystem directory (names only). */
export function listLocalDir(path: string): Promise<LocalDir> {
  return invoke<LocalDir>("list_local_dir", { path });
}

/** Which of `paths` exist as directories on this machine (parallel to input). */
export function pathsExist(paths: string[]): Promise<boolean[]> {
  return invoke<boolean[]>("paths_exist", { paths });
}

/** True only for tagged release builds (dev/local builds skip the update check). */
export function isReleaseBuild(): Promise<boolean> {
  return invoke<boolean>("is_release_build");
}

/** Native folder-picker; returns the chosen directory or null if cancelled. */
export function pickFolder(start = ""): Promise<string | null> {
  return invoke<string | null>("pick_folder", { start });
}

/** Put text on the system clipboard (app-local — no p4, no safe-mode gate). */
export function setClipboard(text: string): Promise<void> {
  return invoke<void>("set_clipboard", { text });
}

/** An installed text editor the app can open files with. */
export interface EditorInfo {
  id: string;
  name: string;
  path: string;
}
/** Editors detected on this machine (Notepad always; Notepad++/VS Code/…). */
export function detectEditors(): Promise<EditorInfo[]> {
  return invoke<EditorInfo[]>("detect_editors");
}
/** The detected-editor id Windows opens .txt with, or "" if unrecognized. */
export function defaultEditorId(): Promise<string> {
  return invoke<string>("default_editor_id");
}
/** Launch `exe file` detached (app-local — no p4, no safe-mode gate). */
export function openInEditor(exe: string, file: string): Promise<void> {
  return invoke<void>("open_in_editor", { exe, file });
}

/** The two materialized sides of a diff, for the in-app diff window. */
export interface DiffPair {
  left: string;
  right: string;
  leftLabel: string;
  rightLabel: string;
  title: string;
  /** True when the right side is the workspace file, so it can be edited. */
  rightEditable?: boolean;
  /** Set when the file still needs a resolve: which depot revision is missing.
   *  Empty otherwise. */
  unresolvedNote?: string;
}
/** Write an edited workspace file back (the diff window's editable side). Goes
 *  through the safe-mode gate: it writes to the workspace. */
export function writeLocalFile(path: string, text: string): Promise<void> {
  return safe.guard("write_local_file", () => invoke<void>("write_local_file", { path, text }));
}

/** One line of a file, with the change that introduced it. */
export interface BlameLine {
  /** Set when this line's revision belongs to another file — a line written
   *  before this path was branched. Empty for the ordinary case. */
  file: string;
  change: string;
  /** The revision that change produced; empty when filelog didn't reach it. */
  rev: string;
  user: string;
  date: string;
  text: string;
}
export interface Blame {
  depotFile: string;
  rev: string;
  lines: BlameLine[];
}

/** One file's outcome from check out / add / delete / move. */
export interface OpenResult {
  file: string;
  ok: boolean;
  /** The action p4 opened it for, or why it refused — including the
   *  "also opened by <user>" line that names whoever is in the way. */
  message: string;
}

/** One holder of a file: someone who has it open, in which workspace. */
export interface Holder {
  user: string;
  client: string;
  action: string;
  change: string;
  /** This holder blocks everyone else — an explicit lock, or an open on a `+l`
   *  (exclusive) file. */
  blocking: boolean;
}
/** Who has a file, and whether anyone owns it exclusively. */
export interface FileHolders {
  depotFile: string;
  headType: string;
  /** The type carries `+l`: one open at a time. */
  exclusiveType: boolean;
  /** Our own open action ("" when we don't have it open). */
  ourAction: string;
  ourLock: boolean;
  otherLock: boolean;
  others: Holder[];
}

/** What an unshelve restored. `needsResolve` is set when p4 left files flagged
 *  unresolved — it does that when the shelf lands on files that were still open. */
export interface UnshelveResult {
  restored: number;
  needsResolve: boolean;
  notes: string[];
}

/** One file's fate in an undo. */
export interface UndoFile {
  depotFile: string;
  ok: boolean;
  /** p4's own words — what it did, or why it refused. */
  message: string;
}
/** What `p4 undo` produced: a pending changelist, not a depot change. */
export interface UndoResult {
  change: string;
  files: UndoFile[];
  undone: number;
  failed: number;
  /** At least one file must be resolved before the undo can be submitted. */
  needsResolve: boolean;
}

/** Open the blame window for a depot file at `revSpec` ("" = head). */
export function openBlameWindow(conn: P4Conn, depotFile: string, revSpec = ""): Promise<void> {
  return invoke<void>("open_blame_window", { conn, depotFile, revSpec });
}

/** Open the file-history window for a depot file (one window per file). */
export function openFileHistoryWindow(conn: P4Conn, depotFile: string): Promise<void> {
  return invoke<void>("open_file_history_window", { conn, depotFile });
}

/** Open (or re-focus) the window for one review. */
export function openReviewWindow(conn: P4Conn, id: number): Promise<void> {
  return invoke<void>("open_review_window", { conn, id });
}

/** Which review discussion a diff window is part of, and what each pane shows.
 *  `leftVersion` 0 means the pane shows the BASE of `leftOf`. */
export interface CommentTarget {
  review: number;
  file: string;
  leftVersion: number;
  leftOf: number;
  rightVersion: number;
}

/** Open the in-app side-by-side diff window on a materialized pair.
 *
 *  `conn` + `comments` turn on the comment layer: with them the window can read
 *  and post Swarm comments on the lines it is showing. Both or neither. */
export function openDiffWindow(
  pair: DiffPair,
  conn: P4Conn | null = null,
  comments: CommentTarget | null = null,
): Promise<void> {
  return invoke<void>("open_diff_window", { pair, conn, comments });
}

/** The connection and comment target a diff window was opened with. Rejects for
 *  an ordinary diff, which has no discussion to join. */
export function diffJob(job: string): Promise<{ conn: P4Conn; comments: CommentTarget }> {
  return invoke<{ conn: P4Conn; comments: CommentTarget }>("diff_job", { job });
}

// Gate every backend call through safe mode (the allow decision + labels live in
// $lib/safe + $lib/p4cmds; reads and app-local calls pass straight through).
function g<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return safe.guard(cmd, () => invoke<T>(cmd, args));
}
async function call(cmd: string, args: Record<string, unknown>): Promise<P4Record[]> {
  return g<P4Record[]>(cmd, args);
}

/** One reason a sync could not write a file (see p4_sync_blockers). */
export type SyncBlocker = {
  file: string;
  depotFile: string;
  clientFile: string;
  kind: "untracked" | "modified" | "writable" | "gone" | "unknown";
  reason: string;
  haveRev: string;
  headRev: string;
  localSize: number;
  depotSize: number;
};

/** Write a line into this session's log file. Not gated by safe mode (it runs no
 *  p4 command) and never throws: a diagnostic must not be able to break the app. */
export function appLog(line: string): void {
  void invoke("app_log", { line }).catch(() => {});
}

/** A workspace spec, as the manage dialog reads it. */
export type ClientSpec = {
  client: string;
  owner: string;
  host: string;
  root: string;
  stream: string;
  description: string;
  options: string;
  submitOptions: string;
  lineEnd: string;
  access: string;
  update: string;
};

export const p4 = {
  info: (conn: P4Conn) => call("p4_info", { conn }),
  clients: (conn: P4Conn) => call("p4_clients", { conn }),
  newClient: (conn: P4Conn, name: string, root: string, stream: string) =>
    g<void>("p4_new_client", { conn, name, root, stream }),
  dirs: (conn: P4Conn, path: string) => call("p4_dirs", { conn, path }),
  files: (conn: P4Conn, path: string) => call("p4_files", { conn, path }),
  changes: (conn: P4Conn, path: string, max = 50, before?: number) =>
    call("p4_changes", { conn, path, max, before: before ?? null }),
  pending: (conn: P4Conn, max = 50) => call("p4_pending", { conn, max }),
  haveChange: (conn: P4Conn, path: string) => call("p4_have_change", { conn, path }),
  changesExact: (conn: P4Conn, spec: string, max = 1) =>
    call("p4_changes_exact", { conn, spec, max }),
  describe: (conn: P4Conn, change: string) => call("p4_describe", { conn, change }),
  /** A file's revisions. `follow` walks back through the branch it was created
   *  from (p4's -i), which is what makes history readable across a depot
   *  migration: rows from before it name the OLD path, so acting on one reaches
   *  the file its revision number belongs to. */
  filelog: (conn: P4Conn, file: string, max = 100, follow = false) =>
    call("p4_filelog", { conn, file, max, follow }),
  fstat: (conn: P4Conn, file: string) => call("p4_fstat", { conn, file }),
  sync: (conn: P4Conn, path?: string) => call("p4_sync", { conn, path: path ?? null }),
  reconcile: (conn: P4Conn, path: string) => call("p4_reconcile", { conn, path }),
  /** Check out specific offline-modified files (exact-path reconcile). */
  /** Check out offline files; `change` empty = the default changelist. */
  reconcileFiles: (conn: P4Conn, files: string[], change = "") =>
    call("p4_reconcile_files", { conn, files, change }),
  /** Revert offline changes: restore files to their depot state (p4 clean). */
  clean: (conn: P4Conn, files: string[]) => call("p4_clean", { conn, files }),
  /** Repair have/disk desyncs: update the have record to #head, disk untouched. */
  flush: (conn: P4Conn, files: string[]) => call("p4_flush", { conn, files }),
  status: (conn: P4Conn) => call("p4_status", { conn }),
  cancelOfflineScan: () => g<void>("cancel_offline_scan"),
  resync: (conn: P4Conn, files: string[], force: boolean) =>
    call("p4_resync", { conn, files, force }),
  /** Streaming sync of one or more specs (empty list = whole workspace). */
  syncStream: (conn: P4Conn, paths: string[] = []) =>
    g<number>("p4_sync_stream", { conn, paths }),
  syncCancel: () => g<void>("sync_cancel"),
  search: (conn: P4Conn, root: string, term: string, max = 300) =>
    call("p4_search", { conn, root, term, max }),
  diff2: (conn: P4Conn, depotFile: string, rev: number) =>
    g<string>("p4_diff2", { conn, depotFile, rev }),
  openDiff: (conn: P4Conn, depotFile: string, rev: number) =>
    g<void>("open_diff", { conn, depotFile, rev }),
  describeShelved: (conn: P4Conn, change: string) => call("p4_describe_shelved", { conn, change }),
  diffShelved: (conn: P4Conn, depotFile: string, rev: number, change: string) =>
    g<string>("p4_diff_shelved", { conn, depotFile, rev, change }),
  openDiffShelved: (conn: P4Conn, depotFile: string, rev: number, change: string) =>
    g<void>("open_diff_shelved", { conn, depotFile, rev, change }),
  streams: (conn: P4Conn) => call("p4_streams", { conn }),
  depots: (conn: P4Conn) => call("p4_depots", { conn }),
  switch: (conn: P4Conn, stream: string) => call("p4_switch", { conn, stream }),
  submit: (conn: P4Conn, change: string) => call("p4_submit", { conn, change }),
  /** Remove shelved files: the whole shelf, or just `files`. */
  shelveDelete: (conn: P4Conn, change: string, files: string[] = []) =>
    call("p4_shelve_delete", { conn, change, files }),
  /** (Re)shelve a changelist, or just `files` of it — naming files ADDS to the
   *  shelf, it does not trim it to that set. */
  shelveUpdate: (conn: P4Conn, change: string, files: string[] = []) =>
    call("p4_shelve", { conn, change, files }),
  /** Restore a changelist's shelved files into the workspace; the shelf stays. */
  unshelve: (conn: P4Conn, change: string, files: string[] = []) =>
    g<UnshelveResult>("p4_unshelve", { conn, change, files }),
  requestReview: (conn: P4Conn, change: string) =>
    g<void>("p4_request_review", { conn, change }),
  swarmUrl: (conn: P4Conn) => g<string>("swarm_url", { conn }),
  /** A page of Swarm reviews; every filter is applied server-side. */
  swarmReviews: (conn: P4Conn, query: ReviewQuery) => g<ReviewPage>("swarm_reviews", { conn, query }),
  /** Everyone on the server: the id p4 and Swarm key on, plus the name people
   *  know each other by. Used to resolve a half-typed name in a filter. */
  users: (conn: P4Conn) => g<UserRow[]>("p4_users", { conn }),
  /** Every comment on a review, oldest first. */
  swarmComments: (conn: P4Conn, review: number) => g<Comment[]>("swarm_comments", { conn, review }),
  /** Post a comment: on the review, on a line of a version's diff (anchor), or
   *  as a reply (anchor.parent). Swarm mails the participants immediately. */
  swarmAddComment: (
    conn: P4Conn,
    review: number,
    body: string,
    anchor: CommentAnchor | null,
    taskState = "comment",
  ) => g<Comment>("swarm_add_comment", { conn, review, body, anchor, taskState }),
  /** Edit a body, move a task state, archive or unarchive a thread. */
  swarmEditComment: (
    conn: P4Conn,
    id: number,
    body: string | null,
    taskState: string | null,
    closed: boolean | null,
  ) => g<Comment>("swarm_edit_comment", { conn, id, body, taskState, closed }),
  /** The task states Swarm will accept for this comment (it names them when it
   *  refuses one, which is the only way this version exposes them). */
  swarmTaskTransitions: (conn: P4Conn, id: number) =>
    g<string[]>("swarm_task_transitions", { conn, id }),
  /** Everything about one review: reviewers with their votes, and its versions. */
  swarmReviewDetail: (conn: P4Conn, id: number) =>
    g<ReviewDetail>("swarm_review_detail", { conn, id }),
  /** The state changes Swarm will accept from THIS user on THIS review. */
  swarmTransitions: (conn: P4Conn, id: number) => g<Transitions>("swarm_transitions", { conn, id }),
  /** Approve / reject / archive / ask for revision. Returns the new state label. */
  swarmSetState: (conn: P4Conn, id: number, state: string) =>
    g<string>("swarm_set_state", { conn, id, state }),
  /** Which files differ between two versions of a review (digest comparison). */
  reviewVersionFiles: (conn: P4Conn, a: VersionRef, b: VersionRef) =>
    g<VersionFile[]>("review_version_files", { conn, a, b }),
  /** One file as it stood at two versions, materialized for the diff window. */
  diffPairVersions: (conn: P4Conn, depotFile: string, a: VersionRef, b: VersionRef) =>
    g<DiffPair>("diff_pair_versions", { conn, depotFile, a, b }),
  /** Write a review's shelf out as a patch so the apply pipeline can take it. */
  reviewPatch: (conn: P4Conn, change: string) => g<ReviewPatch>("review_patch", { conn, change }),
  /** Copy a review's binary/added files verbatim (p4 print of the shelved rev). */
  reviewCopyFiles: (conn: P4Conn, change: string, files: string[], mode: "edit" | "offline") =>
    g<CopyResult[]>("review_copy_files", { conn, change, files, mode }),
  /** Shelved changelists under a stream that no review covers (empty path = the
   *  whole depot). `scan` bounds the `p4 changes` window, not the result. */
  shelvedNoReview: (conn: P4Conn, streamPath: string, scan: number) =>
    g<ReviewRow[]>("swarm_shelved_no_review", { conn, streamPath, scan }),
  /** Review status for MANY changelists in one request; rows only for the ones
   *  that have a review. */
  swarmReviewsFor: (conn: P4Conn, changes: string[]) =>
    call("swarm_reviews_for", { conn, changes }),
  /** Authenticated? `error` carries the p4 text when not (charset vs ticket). */
  loginStatus: (conn: P4Conn) => g<{ ok: boolean; error: string }>("p4_login_status", { conn }),
  /** The cached ticket value for this connection (address, else user match). */
  ticketValue: (conn: P4Conn) => g<string>("p4_ticket_value", { conn }),
  ticketUser: (conn: P4Conn) => g<string>("p4_ticket_user", { conn }),
  login: (conn: P4Conn, password: string) => g<void>("p4_login", { conn, password }),
  trust: (conn: P4Conn) => g<void>("p4_trust", { conn }),
  opened: (conn: P4Conn, change: string) => call("p4_opened", { conn, change }),
  /** Depot paths opened for edit but identical to the depot (`diff -sr`). */
  unchangedOpen: (conn: P4Conn) => g<string[]>("p4_unchanged_open", { conn }),
  /** Why a sync could not overwrite these paths — one entry per file. */
  syncBlockers: (conn: P4Conn, files: string[]) => g<SyncBlocker[]>("p4_sync_blockers", { conn, files }),
  /** One workspace's spec. */
  clientSpec: (conn: P4Conn, client: string) => g<ClientSpec>("p4_client_spec", { conn, client }),
  /** Change a workspace's root / stream / host / description. */
  clientSave: (
    conn: P4Conn,
    client: string,
    root: string,
    stream: string,
    host: string,
    description: string,
  ) => g<void>("p4_client_save", { conn, client, root, stream, host, description }),
  /** Delete a workspace; p4's reason comes back on refusal. */
  clientDelete: (conn: P4Conn, client: string) => g<void>("p4_client_delete", { conn, client }),
  /** Rename a workspace, moving its pending work with it. */
  clientRename: (conn: P4Conn, from: string, to: string) =>
    g<void>("p4_client_rename", { conn, from, to }),
  /** Depot paths that differ from these only in case (one file on Windows). */
  caseTwins: (conn: P4Conn, files: string[]) =>
    g<{ file: string; twin: string }[]>("p4_case_twins", { conn, files }),
  /** Path of this session's command log file on disk. */
  sessionLogPath: () => g<string>("session_log_path", {}),
  /** Revert a mixed selection (open files reverted, offline ones cleaned), with
   *  a checked outcome per file. */
  revertLocal: (conn: P4Conn, files: string[]) =>
    g<{ file: string; ok: boolean; how: string; message: string }[]>("p4_revert_local", {
      conn,
      files,
    }),
  diffLocal: (conn: P4Conn, depotFile: string) => g<string>("p4_diff_local", { conn, depotFile }),
  diffOffline: (conn: P4Conn, depotFile: string) =>
    g<string>("p4_diff_local_forced", { conn, depotFile }),
  exportPatch: (conn: P4Conn, change: string, files: string[], defaultName: string) =>
    g<string | null>("export_patch", { conn, change, files, defaultName }),
  /** Depot files under `path` that p4 says still need resolving. */
  resolveNeeded: (conn: P4Conn, path = "") => g<string[]>("resolve_needed", { conn, path }),
  /** Prepare a three-way merge for a p4 resolve conflict; returns the merge id. */
  mergeStartResolve: (conn: P4Conn, depotFile: string) =>
    g<string>("merge_start_resolve", { conn, depotFile }),
  /** Prepare a three-way merge for one rejected patch hunk. */
  mergeStartPatch: (conn: P4Conn, patchPath: string, depotFile: string, hunkIndex: number) =>
    g<string>("merge_start_patch", { conn, patchPath, depotFile, hunkIndex }),
  /** Hand a prepared merge to P4MERGE and wait for it; "cancelled" if unsaved. */
  mergeExternal: (id: string) => g<string>("merge_external", { id }),
  /** Native picker for a .patch/.diff to apply; null if cancelled. */
  pickPatchFile: () => g<string | null>("pick_patch_file"),
  /** Dry-run a patch against this workspace — reports only, writes nothing. */
  previewPatch: (conn: P4Conn, patchPath: string) =>
    g<PatchFileReport[]>("preview_patch", { conn, patchPath }),
  /** Apply a patch. mode "edit" opens each target in `change` first; "offline"
   *  writes to disk only. `partial` takes the hunks that fit, rejecting the rest. */
  applyPatch: (
    conn: P4Conn,
    patchPath: string,
    mode: "edit" | "offline",
    change: string,
    partial: boolean,
  ) => g<PatchFileReport[]>("apply_patch", { conn, patchPath, mode, change, partial }),
  openDiffLocal: (conn: P4Conn, depotFile: string) => g<void>("open_diff_local", { conn, depotFile }),
  /** p4 print a revision spec to a temp file; returns the temp path. */
  printToTemp: (conn: P4Conn, spec: string) => g<string>("print_to_temp", { conn, spec }),
  // Materialize the two sides of a diff for the in-app diff window.
  diffPairRev: (conn: P4Conn, depotFile: string, rev: number) =>
    g<DiffPair>("diff_pair_rev", { conn, depotFile, rev }),
  diffPairShelved: (conn: P4Conn, depotFile: string, rev: number, change: string) =>
    g<DiffPair>("diff_pair_shelved", { conn, depotFile, rev, change }),
  diffPairLocal: (conn: P4Conn, depotFile: string) => g<DiffPair>("diff_pair_local", { conn, depotFile }),
  /** Diff two UE asset files in Unreal: in a RUNNING editor when one is up
   *  (remote-exec DiffAssets), else a fresh `UnrealEditor -diff` instance.
   *  Resolves to "remote" or "launched". `name` = the asset's object name. */
  openUnrealDiff: (
    conn: P4Conn,
    left: string,
    right: string,
    name: string,
    leftRev: string,
    rightRev: string,
  ) => g<string>("open_unreal_diff", { conn, left, right, name, leftRev, rightRev }),
  revert: (conn: P4Conn, depotFile: string) => call("p4_revert", { conn, depotFile }),
  /** Blame: every line with the changelist that introduced it. `revSpec` is a
   *  p4 suffix ("#8", "@=1234") or "" for the head revision. */
  /** Blame `file`. `follow` credits lines written before this path was branched
   *  to whoever wrote them, rather than to whoever branched it. */
  annotate: (conn: P4Conn, depotFile: string, revSpec = "", follow = false) =>
    g<Blame>("p4_annotate", { conn, depotFile, revSpec, follow }),
  /** Check out / mark for add / mark for delete. One p4 call per file so a
   *  refusal can be attributed; `change` empty = the default changelist. */
  openFiles: (conn: P4Conn, verb: "edit" | "add" | "delete", files: string[], change = "") =>
    g<OpenResult[]>("p4_open_files", { conn, verb, files, change }),
  /** Rename/move a file, keeping its history (p4 edit + p4 move). */
  moveFile: (conn: P4Conn, from: string, to: string, change = "") =>
    g<OpenResult>("p4_move_file", { conn, from, to, change }),
  /** Who has this file open, and who (if anyone) holds it exclusively. */
  fileHolders: (conn: P4Conn, depotFile: string) =>
    g<FileHolders>("p4_file_holders", { conn, depotFile }),
  /** The client's pending changelists that hold shelved files. */
  shelvedChanges: (conn: P4Conn) => call("p4_shelved_changes", { conn }),
  /** Revert every file open in a changelist, discarding their local edits. */
  revertChange: (conn: P4Conn, change: string) => call("p4_revert_change", { conn, change }),
  /** Delete an empty pending changelist; p4 refuses if it still holds files. */
  deleteChange: (conn: P4Conn, change: string) => call("p4_delete_change", { conn, change }),
  /** Dry-run an undo (`p4 undo -n`): one row per file, `ok` false for the ones
   *  p4 would refuse. Pass no files to mean the whole changelist. */
  undoPreview: (conn: P4Conn, change: string, files: string[] = []) =>
    call("p4_undo_preview", { conn, change, files }),
  /** Undo a SUBMITTED change into a new pending changelist. Nothing reaches the
   *  depot until that changelist is submitted. */
  undoChange: (conn: P4Conn, change: string, files: string[] = []) =>
    g<UndoResult>("p4_undo_change", { conn, change, files }),
  revertKeep: (conn: P4Conn, depotFile: string) => call("p4_revert_keep", { conn, depotFile }),
  /** Move opened files to another changelist — one command for the set. */
  reopen: (conn: P4Conn, depotFiles: string[], change: string) =>
    call("p4_reopen", { conn, depotFiles, change }),
  newChangelist: (conn: P4Conn, description: string) =>
    g<string>("p4_new_changelist", { conn, description }),
  envPort: (conn: P4Conn) => g<string>("p4_env_port", { conn }),
  setDescription: (conn: P4Conn, change: string, description: string) =>
    g<void>("p4_set_description", { conn, change, description }),
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

/** Split a depot/local path into its directory (with trailing slash) and name. */
export function splitPath(path: string): { dir: string; name: string } {
  const i = path.lastIndexOf("/");
  return i >= 0 ? { dir: path.slice(0, i + 1), name: path.slice(i + 1) } : { dir: "", name: path };
}
