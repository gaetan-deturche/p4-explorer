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

/** One caret and the document it sits in: the shape every primitive edit below
 *  works on. Multi-cursor is a layer on top (see DocState) that runs a primitive
 *  once per cursor — so each edit stays the small, testable thing it was. */
export interface CaretState {
  doc: MergeDoc;
  caret: Caret;
}

/** One cursor: where it is, and where its selection started (null: no selection). */
export interface Cursor {
  head: Caret;
  anchor: Caret | null;
}

/** The editor's state: the document and EVERY cursor in it.
 *
 *  Cursors are kept in document order and never overlap — `normalize` enforces
 *  both after every operation, which is what stops two carets from editing the
 *  same text twice. `focus` is the cursor the view scrolls to (the one that just
 *  moved, or the one just added); an index, so it survives the reordering. */
export interface DocState {
  doc: MergeDoc;
  cursors: Cursor[];
  focus: number;
}

/** What the result pane asks the host to do. Keeping intent separate from the
 *  model means the component never mutates anything itself. */
export type MoveDir = "left" | "right" | "up" | "down" | "home" | "end" | "wordLeft" | "wordRight";

export type MergeAction =
  | { t: "insert"; text: string }
  | { t: "enter" }
  | { t: "backspace" }
  | { t: "delete" }
  | { t: "move"; dir: MoveDir; extend?: boolean }
  | { t: "caret"; caret: Caret; extend?: boolean; add?: boolean }
  | { t: "moveLines"; delta: number; extend?: boolean }
  | { t: "selectAll" }
  | { t: "selectWord"; caret: Caret; add?: boolean }
  | { t: "selectLine"; caret: Caret; add?: boolean }
  | { t: "deleteWord"; forward: boolean }
  | { t: "copy"; cut?: boolean }
  | { t: "addCursor"; dir: -1 | 1 }
  | { t: "addCursorMatch" }
  | { t: "collapse" }
  | { t: "undo" }
  | { t: "redo" }
  | { t: "save" };

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
export function insertText(state: CaretState, text: string): CaretState {
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
export function insertLineBreak(state: CaretState): CaretState {
  return insertText(state, "\n");
}

/** Backspace. At the start of a line it joins with the line above WITHIN the
 *  region; at the region's first line it does nothing, because a region's extent
 *  is not the user's to dissolve. */
export function deleteBackward(state: CaretState): CaretState {
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
    // Otherwise join this line onto the previous region's last line. The regions
    // themselves are never merged or removed — this one may end up empty, which
    // is a perfectly valid block — so the structure (conflict tracking in the
    // resolve window, diff blocks in the diff window) survives the edit while
    // the keystroke still does what the user asked.
    const i = doc.regions.findIndex((x) => x.region === r.region);
    const prev = doc.regions.slice(0, i).reverse().find((x) => x.lines.length > 0);
    if (!prev) return state; // start of the document: nothing to join onto
    const at = prev.lines.length - 1;
    const col = prev.lines[at].length;
    prev.lines[at] += r.lines[0];
    r.lines.splice(0, 1);
    return { doc, caret: { region: prev.region, line: at, col } };
  }
  const prev = r.lines[caret.line - 1];
  r.lines[caret.line - 1] = prev + r.lines[caret.line];
  r.lines.splice(caret.line, 1);
  return { doc, caret: { region: r.region, line: caret.line - 1, col: prev.length } };
}

/** Delete forward. Joins with the following line within the region only. */
export function deleteForward(state: CaretState): CaretState {
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
    // Pull the NEXT region's first line up onto this one — same reasoning as
    // backspace at a region's first line: text joins, blocks stay (the next one
    // may become empty).
    const i = doc.regions.findIndex((x) => x.region === r.region);
    const next = doc.regions.slice(i + 1).find((x) => x.lines.length > 0);
    if (!next) return state; // end of the document
    r.lines[caret.line] = line + next.lines[0];
    next.lines.splice(0, 1);
    return { doc, caret };
  }
  r.lines[caret.line] = line + r.lines[caret.line + 1];
  r.lines.splice(caret.line + 1, 1);
  return { doc, caret };
}

/** Replace a region's lines wholesale — taking a side, or resetting it. */
export function setRegionLines(state: CaretState, region: number, lines: string[]): CaretState {
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

// --- selection -------------------------------------------------------------
// A selection is an anchor plus the caret. It may span regions — reading across
// them is useful — but deleting one never merges regions: each keeps its identity
// and loses only the text that was selected inside it.

/** Position of a region in document order, for comparing carets. */
function order(doc: MergeDoc, region: number): number {
  return doc.regions.findIndex((r) => r.region === region);
}

export function sameCaret(a: Caret, b: Caret): boolean {
  return a.region === b.region && a.line === b.line && a.col === b.col;
}

/** The two ends of a selection, in document order. */
export function orderCarets(doc: MergeDoc, a: Caret, b: Caret): { from: Caret; to: Caret } {
  const ai = order(doc, a.region);
  const bi = order(doc, b.region);
  const aFirst = ai !== bi ? ai < bi : a.line !== b.line ? a.line < b.line : a.col <= b.col;
  return aFirst ? { from: a, to: b } : { from: b, to: a };
}

/** The selected text, for the clipboard. */
export function selectedText(doc: MergeDoc, a: Caret, b: Caret): string {
  const { from, to } = orderCarets(doc, a, b);
  const fi = order(doc, from.region);
  const ti = order(doc, to.region);
  if (fi < 0 || ti < 0) return "";
  const out: string[] = [];
  for (let i = fi; i <= ti; i++) {
    const r = doc.regions[i];
    if (!r) continue;
    const first = i === fi ? from.line : 0;
    const last = i === ti ? to.line : r.lines.length - 1;
    for (let l = first; l <= last; l++) {
      const line = r.lines[l] ?? "";
      const a0 = i === fi && l === from.line ? from.col : 0;
      const b0 = i === ti && l === to.line ? to.col : line.length;
      out.push(line.slice(a0, b0));
    }
  }
  return out.join("\n");
}

/** Remove the selected text. Regions survive; only their content shrinks. */
export function deleteRange(state: CaretState, a: Caret, b: Caret): CaretState {
  const { from, to } = orderCarets(state.doc, a, b);
  if (sameCaret(from, to)) return state;
  const doc = clone(state.doc);
  const fi = order(doc, from.region);
  const ti = order(doc, to.region);
  if (fi < 0 || ti < 0) return state;

  if (fi === ti) {
    const r = doc.regions[fi];
    const head = (r.lines[from.line] ?? "").slice(0, from.col);
    const tail = (r.lines[to.line] ?? "").slice(to.col);
    r.lines.splice(from.line, to.line - from.line + 1, head + tail);
    return { doc, caret: { region: from.region, line: from.line, col: from.col } };
  }

  for (let i = fi; i <= ti; i++) {
    const r = doc.regions[i];
    if (!r) continue;
    if (i === fi) {
      const head = (r.lines[from.line] ?? "").slice(0, from.col);
      r.lines = [...r.lines.slice(0, from.line), head];
      if (r.lines.length === 1 && r.lines[0] === "") r.lines = [];
    } else if (i === ti) {
      const tail = (r.lines[to.line] ?? "").slice(to.col);
      r.lines = [tail, ...r.lines.slice(to.line + 1)];
      if (r.lines.length === 1 && r.lines[0] === "") r.lines = [];
    } else {
      r.lines = [];
    }
  }
  const head = doc.regions[fi];
  return {
    doc,
    caret: {
      region: from.region,
      line: Math.min(from.line, Math.max(0, head.lines.length - 1)),
      col: from.col,
    },
  };
}

/** Replace the selection with `text`. */
export function insertOverRange(state: CaretState, a: Caret, b: Caret, text: string): CaretState {
  const cleared = deleteRange(state, a, b);
  return insertText(cleared, text);
}

/** Anchor and head covering everything with text. */
export function selectAll(doc: MergeDoc): { anchor: Caret; head: Caret } | null {
  const first = doc.regions.find((r) => r.lines.length);
  const last = [...doc.regions].reverse().find((r) => r.lines.length);
  if (!first || !last) return null;
  const lastLine = last.lines.length - 1;
  return {
    anchor: { region: first.region, line: 0, col: 0 },
    head: { region: last.region, line: lastLine, col: last.lines[lastLine].length },
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

// --- words and pages -------------------------------------------------------
// Word boundaries are the usual three classes: identifier characters, whitespace,
// and everything else. Motion stops where the class changes, which is what makes
// ctrl+arrow feel right in code.

type CharClass = "word" | "space" | "punct";
function classOf(ch: string): CharClass {
  if (/\s/.test(ch)) return "space";
  return /[A-Za-z0-9_$]/.test(ch) ? "word" : "punct";
}

export function wordLeft(doc: MergeDoc, caret: Caret): Caret {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  if (!r) return c;
  const line = r.lines[c.line] ?? "";
  if (c.col === 0) return moveLeft(doc, c); // hop to the previous line / region
  let i = c.col;
  while (i > 0 && classOf(line[i - 1]) === "space") i--;
  if (i > 0) {
    const cls = classOf(line[i - 1]);
    while (i > 0 && classOf(line[i - 1]) === cls) i--;
  }
  return { ...c, col: i };
}

export function wordRight(doc: MergeDoc, caret: Caret): Caret {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  if (!r) return c;
  const line = r.lines[c.line] ?? "";
  if (c.col >= line.length) return moveRight(doc, c);
  // Whitespace first, then the run it leads into: the caret lands at the END of
  // the next word, which is what ctrl+right does elsewhere.
  let i = c.col;
  while (i < line.length && classOf(line[i]) === "space") i++;
  if (i < line.length) {
    const cls = classOf(line[i]);
    while (i < line.length && classOf(line[i]) === cls) i++;
  }
  return { ...c, col: i };
}

/** The word under a caret — what a double click selects. */
export function wordRange(doc: MergeDoc, caret: Caret): { from: Caret; to: Caret } {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  const line = r?.lines[c.line] ?? "";
  if (!line.length) return { from: c, to: c };
  const pivot = Math.min(c.col, line.length - 1);
  const cls = classOf(line[pivot]);
  let a = pivot;
  let b = pivot + 1;
  while (a > 0 && classOf(line[a - 1]) === cls) a--;
  while (b < line.length && classOf(line[b]) === cls) b++;
  return { from: { ...c, col: a }, to: { ...c, col: b } };
}

/** The whole line — what a triple click selects. */
export function lineRange(doc: MergeDoc, caret: Caret): { from: Caret; to: Caret } {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  const line = r?.lines[c.line] ?? "";
  return { from: { ...c, col: 0 }, to: { ...c, col: line.length } };
}

/** Move by whole rows across regions — page up and page down. */
export function moveByLines(doc: MergeDoc, caret: Caret, delta: number): Caret {
  const flat: { region: number; line: number }[] = [];
  for (const r of doc.regions) for (let l = 0; l < r.lines.length; l++) flat.push({ region: r.region, line: l });
  if (!flat.length) return caret;
  const c = clampCaret(doc, caret);
  const here = flat.findIndex((f) => f.region === c.region && f.line === c.line);
  const idx = Math.max(0, Math.min(flat.length - 1, (here < 0 ? 0 : here) + delta));
  return clampCaret(doc, { ...flat[idx], col: c.col });
}

/** Delete the word before or after the caret. */
export function deleteWord(state: CaretState, forward: boolean): CaretState {
  const to = forward ? wordRight(state.doc, state.caret) : wordLeft(state.doc, state.caret);
  if (sameCaret(to, state.caret)) return state;
  return deleteRange(state, state.caret, to);
}

export function moveLineStart(doc: MergeDoc, caret: Caret): Caret {
  return { ...clampCaret(doc, caret), col: 0 };
}
export function moveLineEnd(doc: MergeDoc, caret: Caret): Caret {
  const c = clampCaret(doc, caret);
  const r = at(doc, c.region);
  return { ...c, col: r?.lines[c.line]?.length ?? 0 };
}

// --- cursors ---------------------------------------------------------------
// Multi-cursor, built on the primitives above: an operation runs the SAME
// primitive once per cursor, bottom-up.
//
// That order is the whole trick. An edit can only shift text at or after its own
// position, so a cursor ABOVE the edit keeps its coordinates untouched and needs
// no mapping — and a cursor below has already had its turn. No position
// arithmetic, no drift, and the primitives stay single-caret and testable.
//
// The two invariants `normalize` maintains after every operation: cursors are in
// document order, and their ranges never overlap. Overlap is what would let two
// carets edit the same text twice, so they are merged instead.

/** Document order of two carets: -1, 0 or 1. */
export function compareCarets(doc: MergeDoc, a: Caret, b: Caret): number {
  const ai = order(doc, a.region);
  const bi = order(doc, b.region);
  if (ai !== bi) return ai < bi ? -1 : 1;
  if (a.line !== b.line) return a.line < b.line ? -1 : 1;
  if (a.col !== b.col) return a.col < b.col ? -1 : 1;
  return 0;
}

/** A cursor's extent in document order. An empty selection gives from === to. */
export function cursorRange(doc: MergeDoc, c: Cursor): { from: Caret; to: Caret } {
  if (!c.anchor) return { from: c.head, to: c.head };
  return orderCarets(doc, c.anchor, c.head);
}

function isEmpty(doc: MergeDoc, c: Cursor): boolean {
  const { from, to } = cursorRange(doc, c);
  return sameCaret(from, to);
}

/** A state with one cursor — what every window starts with. */
export function singleCursor(doc: MergeDoc, caret: Caret, anchor: Caret | null = null): DocState {
  return normalize(doc, [{ head: clampCaret(doc, caret), anchor }], 0);
}

/** The cursor the view follows: the one that last moved, or was last added. */
export function focusCursor(state: DocState): Cursor {
  return state.cursors[state.focus] ?? state.cursors[0];
}

/** The focus cursor's caret — for callers that only care where "the" caret is
 *  (which line of the right-hand file is being edited, where to scroll). */
export function primaryCaret(state: DocState): Caret {
  return focusCursor(state).head;
}

/** Every selection with text in it, in document order. */
export function selectionsOf(state: DocState): { from: Caret; to: Caret }[] {
  return state.cursors
    .filter((c) => !isEmpty(state.doc, c))
    .map((c) => cursorRange(state.doc, c));
}

export function hasSelection(state: DocState): boolean {
  return state.cursors.some((c) => !isEmpty(state.doc, c));
}

/** Sort into document order, clamp onto real text, and merge cursors that would
 *  fight over the same characters. `focus` is an index into `cursors` and is
 *  carried to wherever that cursor ends up (or to the one it merged into). */
export function normalize(doc: MergeDoc, cursors: Cursor[], focus: number): DocState {
  const first = doc.regions[0]?.region ?? 0;
  if (!cursors.length) {
    const head = clampCaret(doc, { region: first, line: 0, col: 0 });
    return { doc, cursors: [{ head, anchor: null }], focus: 0 };
  }
  const tagged = cursors.map((c, i) => ({
    head: clampCaret(doc, c.head),
    anchor: c.anchor ? clampCaret(doc, c.anchor) : null,
    was: i,
  }));
  tagged.sort((a, b) => {
    const ra = cursorRange(doc, a);
    const rb = cursorRange(doc, b);
    return compareCarets(doc, ra.from, rb.from) || compareCarets(doc, ra.to, rb.to);
  });

  const out: Cursor[] = [];
  let focusOut = 0;
  for (const c of tagged) {
    const prev = out[out.length - 1];
    if (prev) {
      const pr = cursorRange(doc, prev);
      const cr = cursorRange(doc, c);
      const overlaps = compareCarets(doc, cr.from, pr.to) < 0;
      const sameSpot = sameCaret(cr.from, pr.to) && isEmpty(doc, prev) && isEmpty(doc, c);
      if (overlaps || sameSpot) {
        // One cursor spanning both. The merged selection reads forward, which is
        // only visible if the user then extends it — a fair price for never
        // letting two cursors own the same characters.
        if (!sameSpot) {
          const to = compareCarets(doc, cr.to, pr.to) > 0 ? cr.to : pr.to;
          out[out.length - 1] = { anchor: pr.from, head: to };
        }
        if (c.was === focus) focusOut = out.length - 1;
        continue;
      }
    }
    out.push({ head: c.head, anchor: c.anchor });
    if (c.was === focus) focusOut = out.length - 1;
  }
  return { doc, cursors: out, focus: Math.max(0, Math.min(focusOut, out.length - 1)) };
}

/** A caret as a character offset into its region's text. */
function toOffset(r: DocRegion, caret: Caret): number {
  let n = 0;
  for (let l = 0; l < caret.line && l < r.lines.length; l++) n += r.lines[l].length + 1;
  return n + caret.col;
}

function fromOffset(r: DocRegion, off: number): Caret {
  let n = 0;
  for (let l = 0; l < r.lines.length; l++) {
    const end = n + r.lines[l].length;
    if (off <= end) return { region: r.region, line: l, col: off - n };
    n = end + 1;
  }
  const last = Math.max(0, r.lines.length - 1);
  return { region: r.region, line: last, col: r.lines[last]?.length ?? 0 };
}

/** Carry a caret across one edit.
 *
 *  An edit is a single splice in one region's text (a cross-region join splices
 *  two), so comparing the region's text before and after — common prefix, common
 *  suffix — recovers exactly where and by how much the text moved. That is all
 *  the caret needs, and it works for every primitive without any of them having
 *  to report what it did. */
function remapCaret(before: MergeDoc, after: MergeDoc, caret: Caret): Caret {
  const b = at(before, caret.region);
  const a = at(after, caret.region);
  if (!b || !a) return caret;
  const oldText = b.lines.join("\n");
  const newText = a.lines.join("\n");
  if (oldText === newText) return caret;
  let pre = 0;
  while (pre < oldText.length && pre < newText.length && oldText[pre] === newText[pre]) pre++;
  let suf = 0;
  while (
    suf < oldText.length - pre &&
    suf < newText.length - pre &&
    oldText[oldText.length - 1 - suf] === newText[newText.length - 1 - suf]
  ) {
    suf++;
  }
  const removedEnd = oldText.length - suf; // end of the replaced span, old coords
  const off = toOffset(b, caret);
  if (off < pre) return caret; // before the edit: untouched
  const delta = newText.length - oldText.length;
  // At or after the edit, shift by the size change. A caret INSIDE the replaced
  // span cannot happen here (cursor ranges never overlap), but clamping to the
  // end of the replacement is the sane answer if it ever did.
  const moved = off >= removedEnd ? off + delta : newText.length - suf;
  return fromOffset(a, Math.max(0, Math.min(moved, newText.length)));
}

/** Run one primitive at every cursor, bottom-up. Each cursor's selection is
 *  consumed by the edit, leaving a caret where the text landed.
 *
 *  Bottom-up keeps every cursor's INPUT coordinates valid (only text below it has
 *  moved so far). The results already recorded below are then carried across the
 *  edit, which is the half that a naive bottom-up loop gets wrong: pressing Enter
 *  at three carets must leave three carets on three new lines, not three carets
 *  drifting up by one line each. */
function editEach(state: DocState, fn: (s: CaretState, cur: Cursor) => CaretState): DocState {
  let doc = state.doc;
  const heads: Caret[] = new Array(state.cursors.length);
  // Cursors are in document order, so counting down IS bottom-up.
  for (let i = state.cursors.length - 1; i >= 0; i--) {
    const cur = state.cursors[i];
    const before = doc;
    const r = fn({ doc, caret: cur.head }, cur);
    doc = r.doc;
    heads[i] = r.caret;
    for (let j = i + 1; j < heads.length; j++) heads[j] = remapCaret(before, doc, heads[j]);
  }
  return normalize(
    doc,
    heads.map((head) => ({ head, anchor: null })),
    state.focus,
  );
}

/** A cursor's selection, if it has one — in the shape deleteRange wants. */
function sel(doc: MergeDoc, cur: Cursor): { a: Caret; b: Caret } | null {
  return cur.anchor && !sameCaret(cur.anchor, cur.head) ? { a: cur.anchor, b: cur.head } : null;
}

export function applyInsert(state: DocState, text: string): DocState {
  return editEach(state, (s, cur) => {
    const r = sel(s.doc, cur);
    return r ? insertOverRange(s, r.a, r.b, text) : insertText(s, text);
  });
}

export function applyEnter(state: DocState): DocState {
  return applyInsert(state, "\n");
}

export function applyBackspace(state: DocState): DocState {
  return editEach(state, (s, cur) => {
    const r = sel(s.doc, cur);
    return r ? deleteRange(s, r.a, r.b) : deleteBackward(s);
  });
}

export function applyDelete(state: DocState): DocState {
  return editEach(state, (s, cur) => {
    const r = sel(s.doc, cur);
    return r ? deleteRange(s, r.a, r.b) : deleteForward(s);
  });
}

export function applyDeleteWord(state: DocState, forward: boolean): DocState {
  return editEach(state, (s, cur) => {
    const r = sel(s.doc, cur);
    return r ? deleteRange(s, r.a, r.b) : deleteWord(s, forward);
  });
}

/** Replace a region's lines — taking a side. It rewrites a whole block, so the
 *  cursors that were inside it have nothing left to point at: collapse to one. */
export function applyRegionLines(state: DocState, region: number, lines: string[]): DocState {
  const r = setRegionLines({ doc: state.doc, caret: primaryCaret(state) }, region, lines);
  return singleCursor(r.doc, r.caret);
}

type Mover = (doc: MergeDoc, caret: Caret) => Caret;
const MOVERS: Record<MoveDir, Mover> = {
  left: moveLeft,
  right: moveRight,
  up: (d, c) => moveVertical(d, c, -1),
  down: (d, c) => moveVertical(d, c, 1),
  home: moveLineStart,
  end: moveLineEnd,
  wordLeft,
  wordRight,
};

/** Move every cursor. Extending keeps each cursor's own anchor, so N selections
 *  grow at once. */
export function applyMove(state: DocState, dir: MoveDir, extend = false): DocState {
  const move = MOVERS[dir];
  const cursors = state.cursors.map((cur) => ({
    head: move(state.doc, cur.head),
    anchor: extend ? (cur.anchor ?? cur.head) : null,
  }));
  return normalize(state.doc, cursors, state.focus);
}

export function applyMoveLines(state: DocState, delta: number, extend = false): DocState {
  const cursors = state.cursors.map((cur) => ({
    head: moveByLines(state.doc, cur.head, delta),
    anchor: extend ? (cur.anchor ?? cur.head) : null,
  }));
  return normalize(state.doc, cursors, state.focus);
}

/** Click. Plain: one cursor here. Shift: extend the focus cursor. Add: keep the
 *  others and put another one here (alt-click). */
export function applyCaret(
  state: DocState,
  caret: Caret,
  opts: { extend?: boolean; add?: boolean } = {},
): DocState {
  const head = clampCaret(state.doc, caret);
  if (opts.add) {
    return normalize(state.doc, [...state.cursors, { head, anchor: null }], state.cursors.length);
  }
  if (opts.extend) {
    const cur = focusCursor(state);
    return singleCursor(state.doc, head, cur.anchor ?? cur.head);
  }
  return singleCursor(state.doc, head);
}

export function applySelectAll(state: DocState): DocState {
  const all = selectAll(state.doc);
  if (!all) return state;
  return singleCursor(state.doc, all.head, all.anchor);
}

export function applySelectWord(state: DocState, caret: Caret, add = false): DocState {
  const w = wordRange(state.doc, caret);
  const cur: Cursor = { head: w.to, anchor: w.from };
  if (!add) return normalize(state.doc, [cur], 0);
  return normalize(state.doc, [...state.cursors, cur], state.cursors.length);
}

export function applySelectLine(state: DocState, caret: Caret, add = false): DocState {
  const l = lineRange(state.doc, caret);
  const cur: Cursor = { head: l.to, anchor: l.from };
  if (!add) return normalize(state.doc, [cur], 0);
  return normalize(state.doc, [...state.cursors, cur], state.cursors.length);
}

/** Escape: back to one plain caret, where the focus cursor is. */
export function applyCollapse(state: DocState): DocState {
  return singleCursor(state.doc, primaryCaret(state));
}

/** Add a cursor above the top one, or below the bottom one — alt+shift+up/down.
 *  Growing from the EDGE (not from the focus) is what makes repeating the
 *  keystroke walk down a column, and it is why cursors are kept in order. */
export function addCursorVertical(state: DocState, dir: -1 | 1): DocState {
  const edge = dir < 0 ? state.cursors[0] : state.cursors[state.cursors.length - 1];
  if (!edge) return state;
  const head = moveVertical(state.doc, edge.head, dir);
  if (sameCaret(head, edge.head)) return state; // already at the document edge
  return normalize(state.doc, [...state.cursors, { head, anchor: null }], state.cursors.length);
}

/** Every line in the document, in order — the space a search walks. */
function flatLines(doc: MergeDoc): { region: number; line: number; text: string }[] {
  const out: { region: number; line: number; text: string }[] = [];
  for (const r of doc.regions) {
    for (let l = 0; l < r.lines.length; l++) out.push({ region: r.region, line: l, text: r.lines[l] });
  }
  return out;
}

/** Is `[from, to)` a whole word where it sits: all word characters, with
 *  something other than a word character on each side? If so, matches elsewhere
 *  have to be whole words too — otherwise selecting `id` would start collecting
 *  carets inside `width`. Read off the selection itself, so nothing has to
 *  remember how the selection was made. */
function isWholeWord(line: string, from: number, to: number): boolean {
  if (to <= from) return false;
  for (let i = from; i < to; i++) if (classOf(line[i]) !== "word") return false;
  const before = from > 0 ? classOf(line[from - 1]) : "space";
  const after = to < line.length ? classOf(line[to]) : "space";
  return before !== "word" && after !== "word";
}

/** Ctrl+D: with nothing selected, select the word under each caret; with a
 *  selection, add a cursor at the next occurrence of that text and select it
 *  there too. Pressing it repeatedly walks down the file and wraps.
 *
 *  Matching is case-sensitive, whole-word when the selection is one (see
 *  isWholeWord), and occurrences already carrying a cursor are skipped —
 *  otherwise the key would stall on a match it had already taken. A selection
 *  spanning lines has nothing to look for and is left alone. */
export function addCursorAtNextMatch(state: DocState): DocState {
  const focus = focusCursor(state);
  if (isEmpty(state.doc, focus)) {
    // The first press: every caret grabs its own word. Finding matches starts on
    // the press after that, once there is something to look for.
    const cursors = state.cursors.map((cur) => {
      const w = wordRange(state.doc, cur.head);
      return sameCaret(w.from, w.to) ? cur : { anchor: w.from, head: w.to };
    });
    return normalize(state.doc, cursors, state.focus);
  }
  const r = cursorRange(state.doc, focus);
  if (r.from.region !== r.to.region || r.from.line !== r.to.line) return state;
  const home = at(state.doc, r.from.region);
  const source = home?.lines[r.from.line] ?? "";
  const needle = source.slice(r.from.col, r.to.col);
  if (!needle) return state;
  const wholeWord = isWholeWord(source, r.from.col, r.to.col);

  const flat = flatLines(state.doc);
  if (!flat.length) return state;
  const taken = new Set(
    state.cursors.map((c) => {
      const cr = cursorRange(state.doc, c);
      return `${cr.from.region}:${cr.from.line}:${cr.from.col}`;
    }),
  );
  // Search starts after the LAST cursor, so repeated presses move down the file
  // rather than fighting over the same neighbourhood.
  const last = cursorRange(state.doc, state.cursors[state.cursors.length - 1]).to;
  const from = Math.max(
    0,
    flat.findIndex((f) => f.region === last.region && f.line === last.line),
  );
  // One extra step so the starting line is also searched from its beginning:
  // that is the wrap.
  for (let n = 0; n <= flat.length; n++) {
    const f = flat[(from + n) % flat.length];
    let at0 = f.text.indexOf(needle, n === 0 ? last.col : 0);
    while (at0 >= 0) {
      const fits = !wholeWord || isWholeWord(f.text, at0, at0 + needle.length);
      if (fits && !taken.has(`${f.region}:${f.line}:${at0}`)) {
        const cur: Cursor = {
          anchor: { region: f.region, line: f.line, col: at0 },
          head: { region: f.region, line: f.line, col: at0 + needle.length },
        };
        return normalize(state.doc, [...state.cursors, cur], state.cursors.length);
      }
      at0 = f.text.indexOf(needle, at0 + 1);
    }
  }
  return state; // the only occurrences are the ones already selected
}

/** What ctrl+c puts on the clipboard: every selection, top to bottom, one per
 *  line — so a column of selections copies as a column. */
export function copyText(state: DocState): string {
  const parts = selectionsOf(state).map((r) => selectedText(state.doc, r.from, r.to));
  return parts.join("\n");
}

// --- history ---------------------------------------------------------------
// A snapshot of the document AND every cursor, so undo can never restore one
// without the other — the failure that made ctrl+z leave the boundaries behind.
// Multi-cursor came for free here: DocState holds the cursors, so an undone
// multi-cursor edit puts all of them back where they were.

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
