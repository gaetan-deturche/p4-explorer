//! Which rows of a block are worth rendering.
//!
//! Every pane in the app draws a file as uniform-height rows, and on a big file
//! rendering all of them is what makes scrolling stutter — 17942 lines of
//! HLSLMaterialTranslator.cpp come to ~283k DOM nodes in a diff. So each block
//! renders the rows in view and no others.
//!
//! The rows it names are PLACED, not stacked: each one is positioned at
//! `line * lineH` inside its block, so this answers "which rows" and nothing
//! else. It used to also report the filler to leave above and below, back when
//! the drawn rows were stacked after a spacer — and that is exactly what went
//! wrong: the spacer was sized in nominal rows (12px * 1.45 = 17.4) while the
//! caret and the selection bands used the height the browser had actually given
//! a row (17.3906 at 125% scaling), so a selection sat right at the top of a
//! file and a third of a row high by line 600. Placing every row from the same
//! arithmetic the overlays use leaves nothing to drift.

export interface RowWindow {
  /** First row to render. */
  first: number;
  /** Last row to render; `first - 1` when the block is off screen entirely. */
  last: number;
}

/**
 * @param viewTop   the scroller's scrollTop
 * @param viewH     the scroller's visible height
 * @param top       this block's y within the scrolled content
 * @param lines     how many rows the block has
 * @param lineH     row height in px
 * @param overscan  rows to keep either side, so a fast scroll shows no gap
 */
export function rowWindow(
  viewTop: number,
  viewH: number,
  top: number,
  lines: number,
  lineH: number,
  overscan = 24,
): RowWindow {
  if (lines <= 0) return { first: 0, last: -1 };
  const rawFirst = Math.floor((viewTop - top) / lineH) - overscan;
  const rawLast = Math.ceil((viewTop + viewH - top) / lineH) + overscan;
  // `last < first` is how a block with nothing on screen says so, and `first`
  // is deliberately allowed to reach `lines` (one past the end) to produce it:
  // for a block scrolled entirely ABOVE the view, `last` then clamps to
  // `first - 1`. Clamping `first` to `lines - 1` instead looks tidier and makes
  // every such block draw its last row — 300 stray rows on a long file.
  const first = Math.max(0, Math.min(lines, rawFirst));
  const last = Math.max(first - 1, Math.min(lines - 1, rawLast));
  return { first, last };
}
