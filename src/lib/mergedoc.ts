//! The merge document: regions that own their lines.
//!
//! This is the model behind the resolve window's result pane. The point of it is
//! what it makes impossible. Tracking regions as ranges over one big document —
//! which is what a general-purpose editor gives you — means a line can be claimed
//! by two regions, a keystroke at a boundary belongs to neither, and undo restores
//! the text while the boundaries stay where the undone edit left them. Here a line
//! belongs to exactly one region by construction, editing happens inside a region
//! and cannot dissolve a boundary, and undo is a snapshot of the whole thing.
//!
//! Everything is pure: no DOM, no editor, so it is testable on its own.

/** One merge region. `kind` drives its band: add | del | vs | keep | "". */
export interface DocRegion {
  region: number;
  kind: string;
  conflict: boolean;
  lines: string[];
}

/** Where the caret is: a line within a region, and a column within that line. */
export interface Caret {
  region: number;
  line: number;
  col: number;
}

export interface MergeDoc {
  regions: DocRegion[];
}

export interface DocState {
  doc: MergeDoc;
  caret: Caret;
}

/** What the result pane asks the host to do. Keeping intent separate from the
 *  model means the component never mutates anything itself. */
export type MergeAction =
  | { t: "insert"; text: string }
  | { t: "enter" }
  | { t: "backspace" }
  | { t: "delete" }
  | { t: "move"; dir: "left" | "right" | "up" | "down" | "home" | "end" }
  | { t: "caret"; caret: Caret }
  | { t: "undo" }
  | { t: "redo" };

/** The merged file, as it would be saved. */
export function docText(doc: MergeDoc): string {
  return doc.regions.flatMap((r) => r.lines).join("\n");
}

/** Lines a region contributes. An unsettled conflict contributes none, and shows
 *  as a void the width of what the side panes need. */
export function regionLineCount(r: DocRegion): number {
  return r.lines.length;
}

function clone(doc: MergeDoc): MergeDoc {
  return { regions: doc.regions.map((r) => ({ ...r, lines: r.lines.slice() })) };
}

function at(doc: MergeDoc, region: number): DocRegion | undefined {
  return doc.regions.find((r) => r.region === region);
}

/** Rendered width of a tab, in columns. Must match the panes' CSS tab-size. */
export const TAB_WIDTH = 4;

/** Visual column of a character index: a tab advances to the next tab stop, so
 *  `col` (characters) and the caret's x (columns) are not the same number on any
 *  indented line. */
export function visualColumn(line: string, col: number, tab = TAB_WIDTH): number {
  let x = 0;
  for (let i = 0; i < Math.min(col, line.length); i++) {
    x = line[i] === "\t" ? x + (tab - (x % tab)) : x + 1;
  }
  return x + Math.max(0, col - line.length); // past the end: plain columns
}

/** The character index nearest a visual column — the inverse, for mouse clicks. */
export function columnFromVisual(line: string, vx: number, tab = TAB_WIDTH): number {
  let x = 0;
  for (let i = 0; i < line.length; i++) {
    const next = line[i] === "\t" ? x + (tab - (x % tab)) : x + 1;
    if (vx <= x + (next - x) / 2) return i;
    if (vx < next) return i + 1;
    x = next;
  }
  return line.length;
}

/** Clamp a caret onto real text. A region with no lines takes (0, 0). */
export function clampCaret(doc: MergeDoc, caret: Caret): Caret {
  const r = at(doc, caret.region) ?? doc.regions[0];
  if (!r) return { region: 0, line: 0, col: 0 };
  const line = Math.max(0, Math.min(caret.line, Math.max(0, r.lines.length - 1)));
  const text = r.lines[line] ?? "";
  return { region: r.region, line, col: Math.max(0, Math.min(caret.col, text.length)) };
}

/** Insert text at the caret. Newlines split the caret's line; the text stays
 *  inside the caret's region, so no edit can merge two regions. */
export function insertText(state: DocState, text: string): DocState {
  const doc = clone(state.doc);
  const r = at(doc, state.caret.region);
  if (!r) return state;
  const caret = clampCaret(doc, state.caret);
  if (!r.lines.length) r.lines.push("");
  const line = r.lines[caret.line] ?? "";
  const before = line.slice(0, caret.col);
  const after = line.slice(caret.col);
  const parts = text.replace(/\r\n?/g, "\n").split("\n");
  if (parts.length === 1) {
    r.lines[caret.line] = before + parts[0] + after;
    return { doc, caret: { ...caret, col: before.length + parts[0].length } };
  }
  const inserted = [before + parts[0], ...parts.slice(1, -1), parts[parts.length - 1] + after];
  r.lines.splice(caret.line, 1, ...inserted);
  return {
    doc,
    caret: {
      region: r.region,
      line: caret.line + parts.length - 1,
      col: parts[parts.length - 1].length,
    },
  };
}

/** Split the caret's line — the Enter key. */
export function insertLineBreak(state: DocState): DocState {
  return insertText(state, "\n");
}

/** Backspace. At the start of a line it joins with the line above WITHIN the
 *  region; at the region's first line it does nothing, because a region's extent
 *  is not the user's to dissolve. */
export function deleteBackward(state: DocState): DocState {
  const doc = clone(state.doc);
  const r = at(doc, state.caret.region);
  if (!r || !r.lines.length) return state;
  const caret = clampCaret(doc, state.caret);
  if (caret.col > 0) {
    const line = r.lines[caret.line];
    r.lines[caret.line] = line.slice(0, caret.col - 1) + line.slice(caret.col);
    return { doc, caret: { ...caret, col: caret.col - 1 } };
  }
  if (caret.line === 0) {
    // Deleting the last remaining line empties the region: a legitimate
    // resolution ("drop this code"), and it keeps its place as a void.
    if (r.lines.length === 1 && r.lines[0] === "") {
      r.lines = [];
      return { doc, caret: { region: r.region, line: 0, col: 0 } };
    }
    return state;
  }
  const prev = r.lines[caret.line - 1];
  r.lines[caret.line - 1] = prev + r.lines[caret.line];
  r.lines.splice(caret.line, 1);
  return { doc, caret: { region: r.region, line: caret.line - 1, col: prev.length } };
}

/** Delete forward. Joins with the following line within the region only. */
export function deleteForward(state: DocState): DocState {
  const doc = clone(state.doc);
  const r = at(doc, state.caret.region);
  if (!r || !r.lines.length) return state;
  const caret = clampCaret(doc, state.caret);
  const line = r.lines[caret.line];
  if (caret.col < line.length) {
    r.lines[caret.line] = line.slice(0, caret.col) + line.slice(caret.col + 1);
    return { doc, caret };
  }
  if (caret.line + 1 >= r.lines.length) {
    if (r.lines.length === 1 && line === "") {
      r.lines = [];
      return { doc, caret: { region: r.region, line: 0, col: 0 } };
    }
    return state;
  }
  r.lines[caret.line] = line + r.lines[caret.line + 1];
  r.lines.splice(caret.line + 1, 1);
  return { doc, caret };
}

/** Replace a region's lines wholesale — taking a side, or resetting it. */
export function setRegionLines(state: DocState, region: number, lines: string[]): DocState {
  const doc = clone(state.doc);
  const r = at(doc, region);
  if (!r) return state;
  r.lines = lines.slice();
  return {
    doc,
    caret: {
      region,
      line: Math.max(0, lines.length - 1),
      col: lines.length ? lines[lines.length - 1].length : 0,
    },
  };
}

// --- caret movement --------------------------------------------------------
// Regions are traversed in order, so the caret crosses them as if the panes were
// one document, while edits stay inside one region.

function neighbour(doc: MergeDoc, region: number, dir: -1 | 1): DocRegion | undefined {
  const i = doc.regions.findIndex((r) => r.region === region);
  if (i < 0) return undefined;
  for (let k = i + dir; k >= 0 && k < doc.regions.length; k += dir) {
    if (doc.regions[k].lines.length) return doc.regions[k];
  }
  return undefined;
}

export function moveLeft(doc: MergeDoc, caret: Caret): Caret {
  const c = clampCaret(doc, caret);
  if (c.col > 0) return { ...c, col: c.col - 1 };
  const r = at(doc, c.region);
  if (r && c.line > 0) return { region: r.region, line: c.line - 1, col: r.lines[c.line - 1].length };
  const prev = neighbour(doc, c.region, -1);
  if (!prev) return c;
  const last = prev.lines.length - 1;
  return { region: prev.region, line: last, col: prev.lines[last].length };
}

export function moveRight(doc: MergeDoc, caret: Caret): Caret {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  if (!r) return c;
  if (c.col < (r.lines[c.line]?.length ?? 0)) return { ...c, col: c.col + 1 };
  if (c.line + 1 < r.lines.length) return { region: r.region, line: c.line + 1, col: 0 };
  const next = neighbour(doc, c.region, 1);
  return next ? { region: next.region, line: 0, col: 0 } : c;
}

export function moveVertical(doc: MergeDoc, caret: Caret, dir: -1 | 1): Caret {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  if (!r) return c;
  const line = c.line + dir;
  if (line >= 0 && line < r.lines.length) {
    return clampCaret(doc, { region: r.region, line, col: c.col });
  }
  const other = neighbour(doc, c.region, dir);
  if (!other) return c;
  const target = dir < 0 ? other.lines.length - 1 : 0;
  return clampCaret(doc, { region: other.region, line: target, col: c.col });
}

export function moveLineStart(doc: MergeDoc, caret: Caret): Caret {
  return { ...clampCaret(doc, caret), col: 0 };
}
export function moveLineEnd(doc: MergeDoc, caret: Caret): Caret {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  return { ...c, col: r?.lines[c.line]?.length ?? 0 };
}

// --- history ---------------------------------------------------------------
// A snapshot of document AND caret, so undo can never restore one without the
// other — the failure that made ctrl+z leave the boundaries behind.

export interface History {
  past: DocState[];
  future: DocState[];
}

export function emptyHistory(): History {
  return { past: [], future: [] };
}

/** Record `before` as undoable. `coalesce` merges consecutive typing into one
 *  step, so undo does not walk back one character at a time. */
export function push(h: History, before: DocState, coalesce: boolean): History {
  if (coalesce && h.past.length) return { past: h.past, future: [] };
  return { past: [...h.past.slice(-200), before], future: [] };
}

export function undo(h: History, current: DocState): { state: DocState; history: History } | null {
  if (!h.past.length) return null;
  const state = h.past[h.past.length - 1];
  return {
    state,
    history: { past: h.past.slice(0, -1), future: [...h.future, current] },
  };
}

export function redo(h: History, current: DocState): { state: DocState; history: History } | null {
  if (!h.future.length) return null;
  const state = h.future[h.future.length - 1];
  return {
    state,
    history: { past: [...h.past, current], future: h.future.slice(0, -1) },
  };
}
