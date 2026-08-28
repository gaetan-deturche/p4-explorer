//! Turning Swarm's flat comment list into threads, and building the anchor a new
//! comment needs.
//!
//! Plain functions rather than a store: the review window and each diff window
//! are separate webviews with separate JS, so there is no state to share — only
//! this logic, which both need to agree on exactly.

import type { Comment, CommentAnchor } from "$lib/p4";

/** A root comment and its replies, in the order they were written. */
export interface Thread {
  root: Comment;
  replies: Comment[];
  /** The anchor all of them share, for grouping and for the reply anchor. */
  file: string;
  version: number;
  leftLine: number;
  rightLine: number;
  /** Newest activity in the thread, so a file's threads can be ordered by it. */
  updated: number;
  /** True when any comment in it is an unfinished task. */
  openTask: boolean;
}

/** The Unreal plugin's bookkeeping entries are not comments; nothing shows them. */
export function realComments(all: Comment[]): Comment[] {
  return all.filter((c) => !c.bookkeeping);
}

/** Group a flat list into threads. A reply points at its parent through
 *  `context.comment`; a reply whose parent is missing (archived away, or outside
 *  the page we fetched) becomes its own thread rather than vanishing. */
export function threadsOf(all: Comment[]): Thread[] {
  const real = realComments(all);
  const byId = new Map<number, Comment>(real.map((c) => [c.id, c]));
  const roots: Thread[] = [];
  const index = new Map<number, Thread>();

  for (const c of real) {
    if (c.parent && byId.has(c.parent)) continue;
    const t: Thread = {
      root: c,
      replies: [],
      file: c.file,
      version: c.version,
      leftLine: c.leftLine,
      rightLine: c.rightLine,
      updated: c.updated || c.time,
      openTask: c.taskState === "open",
    };
    roots.push(t);
    index.set(c.id, t);
  }
  for (const c of real) {
    if (!c.parent) continue;
    const t = index.get(c.parent) ?? findByAnyReply(index, c.parent);
    if (!t) continue;
    t.replies.push(c);
    t.updated = Math.max(t.updated, c.updated || c.time);
    if (c.taskState === "open") t.openTask = true;
  }
  for (const t of roots) t.replies.sort((a, b) => a.time - b.time || a.id - b.id);
  return roots;
}

/** A reply may point at another reply rather than the root; walk what we have. */
function findByAnyReply(index: Map<number, Thread>, parent: number): Thread | undefined {
  for (const t of index.values()) {
    if (t.replies.some((r) => r.id === parent)) return t;
  }
  return undefined;
}

/** Threads that belong to one file, newest activity first. */
export function threadsForFile(threads: Thread[], file: string): Thread[] {
  return threads.filter((t) => t.file === file).sort((a, b) => b.updated - a.updated);
}

/** Threads with no file anchor: comments on the review as a whole (including the
 *  Unreal plugin's per-asset comments, which carry an asset instead of a line). */
export function generalThreads(threads: Thread[]): Thread[] {
  return threads.filter((t) => !t.file).sort((a, b) => b.updated - a.updated);
}

/** How many files a set of threads touches, and how many threads are open tasks. */
export function commentCounts(threads: Thread[]): { threads: number; tasks: number } {
  return {
    threads: threads.length,
    tasks: threads.filter((t) => t.openTask).length,
  };
}

/** One row of a rendered diff, as far as an anchor cares: which side(s) the row
 *  has, and the text. `type` is the row's kind in the diff. */
export interface AnchorRow {
  type: "same" | "add" | "del" | "mod";
  leftNo: number;
  rightNo: number;
  leftText: string;
  rightText: string;
}

/** The five lines Swarm stores with an anchor: the anchored line and the four
 *  before it, each prefixed the way a unified diff prefixes them.
 *
 *  Measured against a real comment — `[" \t}", " }", " ", "+void A3DFlow..."]`
 *  for a comment on an added line — so: `+` for a line only on the right, `-`
 *  for one only on the left, a space for context. Swarm keeps this so the comment
 *  still reads correctly once the file has moved under it. */
export function contextLines(rows: AnchorRow[], at: number, side: "left" | "right"): string[] {
  const out: string[] = [];
  for (let i = Math.max(0, at - 4); i <= at; i++) {
    const r = rows[i];
    if (!r) continue;
    if (r.type === "add") out.push(`+${r.rightText}`);
    else if (r.type === "del") out.push(`-${r.leftText}`);
    else if (r.type === "mod") out.push(side === "left" ? `-${r.leftText}` : `+${r.rightText}`);
    else out.push(` ${side === "left" ? r.leftText : r.rightText}`);
  }
  return out;
}

/** Which review version a pane is showing, for anchoring. 0 = the base, which is
 *  not a version of its own: a comment on it is anchored to the version it is the
 *  base OF, on the LEFT line — which is how Swarm anchors its own base column. */
export interface PaneVersion {
  /** The version number shown in this pane (0 = the base of `of`). */
  version: number;
  /** For the base pane: the version it is the base of. */
  of: number;
}

/** Build the anchor for a new comment on one row of one pane. */
export function anchorFor(
  file: string,
  pane: PaneVersion,
  rows: AnchorRow[],
  at: number,
  side: "left" | "right",
  parent = 0,
): CommentAnchor | null {
  const row = rows[at];
  if (!row) return null;
  const line = side === "left" ? row.leftNo : row.rightNo;
  if (!line) return null; // that side has no line here (a gap opposite an insert)
  const content = contextLines(rows, at, side);
  if (pane.version > 0) {
    // A real version: its own file, so the line is a RIGHT line of that
    // version's diff — the same numbering Swarm uses.
    return { file, version: pane.version, leftLine: 0, rightLine: line, content, parent };
  }
  // The base of a version: Swarm calls that the left side of that version.
  return { file, version: pane.of, leftLine: line, rightLine: 0, content, parent };
}

/** Where a comment belongs among the two panes on screen, if anywhere.
 *
 *  A comment is anchored to (version, left|right line). A pane showing version N
 *  shows the same lines Swarm numbered as N's right side; a pane showing the base
 *  of version N shows N's left side. Anything else — a comment on a version
 *  neither pane is displaying — is deliberately NOT placed: guessing a line
 *  across versions would put someone's remark next to the wrong code. */
export function placeComment(
  t: Thread,
  left: PaneVersion,
  right: PaneVersion,
): { side: "left" | "right"; line: number } | null {
  const match = (p: PaneVersion): number => {
    if (p.version > 0) return t.version === p.version && t.rightLine ? t.rightLine : 0;
    return t.version === p.of && t.leftLine ? t.leftLine : 0;
  };
  const r = match(right);
  if (r) return { side: "right", line: r };
  const l = match(left);
  if (l) return { side: "left", line: l };
  return null;
}

/** Short label for a task state, or "" for a plain comment. */
export function taskLabel(state: string): string {
  switch (state) {
    case "open":
      return "task";
    case "addressed":
      return "addressed";
    case "verified":
      return "verified";
    default:
      return "";
  }
}
