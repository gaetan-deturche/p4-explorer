//! Showing the characters you cannot see: spaces, tabs, and where a line really
//! ends.
//!
//! Widths are preserved exactly, which is the constraint everything else follows
//! from. The panes measure text with a probe span to place the caret and to map a
//! click back to a column, so a marker that took more or less room than the
//! character it stands for would put the caret in the wrong place. A middle dot
//! replaces a space one-for-one, and a tab becomes an arrow followed by spaces up
//! to its tab stop — in a monospace font that is the same advance the tab had.

/** One run of a rendered line: `ghost` runs are the whitespace markers, `hot` is
 *  the part of the line the diff actually changed. */
export interface Seg {
  text: string;
  color?: string;
  ghost?: boolean;
  hot?: boolean;
  /** Inside a search match. */
  found?: boolean;
  /** Inside the match the find bar is currently ON — the one Enter just moved
   *  to, drawn differently from the others so "where am I" needs no counting. */
  current?: boolean;
}

// Tab width, matching mergedoc's TAB_WIDTH and the panes' CSS tab-size. Declared
// here rather than imported: this module has no dependencies, which is what lets
// the headless suite run it with plain `node`.
const TAB_WIDTH = 4;

export const SPACE_MARK = "\u00b7"; // ·
export const TAB_MARK = "\u2192"; // →

/** The line, cut at the highlighter's run boundaries.
 *
 *  The text always comes from `line`; the runs are read for their LENGTHS and
 *  their colours only. A run array is a colouring of a line, and a colouring can
 *  drift from the line it was asked for — a session answering about the text as
 *  it stood before an edit, a window asked for while the document was changing.
 *  Drift may cost the right colours. It must never change what the file says. */
export function colorParts(
  line: string,
  runs: { content: string; color?: string }[] | undefined,
): { content: string; color?: string }[] {
  if (!runs?.length || !line) return [{ content: line, color: undefined }];
  const out: { content: string; color?: string }[] = [];
  let at = 0;
  for (const r of runs) {
    if (at >= line.length) break; // the runs describe more than this line holds
    out.push({ content: line.slice(at, at + r.content.length), color: r.color });
    at += r.content.length;
  }
  if (at < line.length) out.push({ content: line.slice(at), color: undefined });
  return out;
}

/** Render one line as segments, composing the three things that can apply to it:
 *  the highlighter's colours, the whitespace marks, and the range the diff
 *  changed.
 *
 *  They have to be composed in one pass rather than layered, because the marks
 *  change LENGTHS (a tab becomes an arrow plus padding) while `hot` is a range of
 *  SOURCE characters. Walking the line once, carrying both the source index and
 *  the visual column, is the only way the range lands on the right glyphs. */
export function renderLine(
  line: string,
  runs: { content: string; color?: string }[] | undefined,
  opts: {
    invisibles?: boolean;
    hot?: readonly [number, number] | null;
    /** Search matches on this line, as character ranges. */
    finds?: readonly (readonly [number, number])[];
    /** The one of them the find bar is on, if it is on this line. */
    current?: readonly [number, number] | null;
  } = {},
  tab = TAB_WIDTH,
): Seg[] {
  const marks = !!opts.invisibles;
  const hot = opts.hot ?? null;
  const finds = opts.finds ?? [];
  const current = opts.current ?? null;
  const parts = colorParts(line, runs);
  // Fast path: with nothing to split, the result is the input — and the panes
  // render every line on every pass, so a character walk is not something to pay
  // for that.
  if (!marks && !hot && !finds.length && !current) {
    return parts.map((r) => ({ text: r.content, color: r.color }));
  }
  const out: Seg[] = [];
  let col = 0; // visual column, so a tab lands on a real stop
  let src = 0; // source character index, so the ranges mean what they say
  const within = (rs: readonly (readonly [number, number])[], i: number) =>
    rs.some((r) => i >= r[0] && i < r[1]);
  const push = (
    text: string,
    color: string | undefined,
    ghost: boolean,
    isHot: boolean,
    found: boolean,
    isCurrent: boolean,
  ) => {
    const last = out[out.length - 1];
    if (
      last &&
      last.ghost === ghost &&
      last.color === color &&
      last.hot === isHot &&
      last.found === found &&
      last.current === isCurrent
    ) {
      last.text += text;
    } else {
      out.push({ text, color, ghost, hot: isHot, found, current: isCurrent });
    }
  };
  for (const run of parts) {
    for (const ch of run.content) {
      const isHot = !!hot && src >= hot[0] && src < hot[1];
      const isCurrent = !!current && src >= current[0] && src < current[1];
      const found = isCurrent || within(finds, src);
      if (marks && ch === " ") {
        push(SPACE_MARK, run.color, true, isHot, found, isCurrent);
        col++;
      } else if (marks && ch === "\t") {
        const width = tab - (col % tab);
        push(TAB_MARK + " ".repeat(width - 1), run.color, true, isHot, found, isCurrent);
        col += width;
      } else {
        push(ch, run.color, false, isHot, found, isCurrent);
        col += ch === "\t" ? tab - (col % tab) : 1;
      }
      src++;
    }
  }
  // An empty line yields no segments, and stays that way: inventing a space here
  // would give the line width it does not have, and the panes measure width to
  // place the caret. The markup renders a blank box for an empty line instead.
  return out;
}

/** Split a line into segments, replacing whitespace with visible marks. Coloured
 *  runs (from the highlighter) are preserved: each is visualized in place, with
 *  the tab stops counted across the whole line so the columns stay true. */
export function visualize(
  runs: { content: string; color?: string }[] | undefined,
  line: string,
  tab = TAB_WIDTH,
): Seg[] {
  return renderLine(line, runs, { invisibles: true }, tab);
}

/** What to call a file's line endings in the UI. */
export function endingLabel(kind: "crlf" | "lf" | "mixed" | "none"): string {
  return kind === "crlf" ? "CRLF" : kind === "lf" ? "LF" : kind === "mixed" ? "mixed CRLF/LF" : "";
}
