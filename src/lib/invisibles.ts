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
}

// Tab width, matching mergedoc's TAB_WIDTH and the panes' CSS tab-size. Declared
// here rather than imported: this module has no dependencies, which is what lets
// the headless suite run it with plain `node`.
const TAB_WIDTH = 4;

export const SPACE_MARK = "\u00b7"; // ·
export const TAB_MARK = "\u2192"; // →

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
  opts: { invisibles?: boolean; hot?: readonly [number, number] | null } = {},
  tab = TAB_WIDTH,
): Seg[] {
  const marks = !!opts.invisibles;
  const hot = opts.hot ?? null;
  const parts = runs?.length ? runs : [{ content: line, color: undefined }];
  // Fast path: with neither marks nor a range there is nothing to split, and the
  // panes render every line of the file on every pass — a character walk per line
  // is not something to pay for a result identical to the input.
  if (!marks && !hot) return parts.map((r) => ({ text: r.content, color: r.color }));
  const out: Seg[] = [];
  let col = 0; // visual column, so a tab lands on a real stop
  let src = 0; // source character index, so `hot` means what it says
  const push = (text: string, color: string | undefined, ghost: boolean, isHot: boolean) => {
    const last = out[out.length - 1];
    if (last && last.ghost === ghost && last.color === color && last.hot === isHot) {
      last.text += text;
    } else {
      out.push({ text, color, ghost, hot: isHot });
    }
  };
  for (const run of parts) {
    for (const ch of run.content) {
      const isHot = !!hot && src >= hot[0] && src < hot[1];
      if (marks && ch === " ") {
        push(SPACE_MARK, run.color, true, isHot);
        col++;
      } else if (marks && ch === "	") {
        const width = tab - (col % tab);
        push(TAB_MARK + " ".repeat(width - 1), run.color, true, isHot);
        col += width;
      } else {
        push(ch, run.color, false, isHot);
        col += ch === "	" ? tab - (col % tab) : 1;
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
