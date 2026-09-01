//! Which rows of a block are worth rendering.
//!
//! Every pane in the app draws a file as uniform-height rows, and on a big file
//! rendering all of them is what makes scrolling stutter — 17942 lines of
//! HLSLMaterialTranslator.cpp come to ~283k DOM nodes in a diff. So each block
//! renders the rows in view and pads above and below with a spacer.
//!
//! The padding is why this is worth its own function: the three numbers have to
//! add up to the block's full height in every case, including a block scrolled
//! entirely past, or the rows below it sit at the wrong place.

export interface RowWindow {
  /** First row to render. */
  first: number;
  /** Last row to render; `first - 1` when the block is off screen entirely. */
  last: number;
  /** Rows to leave empty above and below, in rows. Together with the rendered
   *  ones these always sum to the block's line count. */
  padBefore: number;
  padAfter: number;
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
  if (lines <= 0) return { first: 0, last: -1, padBefore: 0, padAfter: 0 };
  const rawFirst = Math.floor((viewTop - top) / lineH) - overscan;
  const rawLast = Math.ceil((viewTop + viewH - top) / lineH) + overscan;
  // `first` may run past the end (the block is above the view) and `last` may
  // fall short of the start (below it); clamping both to the block keeps the
  // spacers honest either way.
  const first = Math.max(0, Math.min(lines, rawFirst));
  const last = Math.max(first - 1, Math.min(lines - 1, rawLast));
  return { first, last, padBefore: first, padAfter: lines - 1 - last };
}
