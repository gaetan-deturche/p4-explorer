//! Showing the characters you cannot see: spaces, tabs, and where a line really
//! ends.
//!
//! Widths are preserved exactly, which is the constraint everything else follows
//! from. The panes measure text with a probe span to place the caret and to map a
//! click back to a column, so a marker that took more or less room than the
//! character it stands for would put the caret in the wrong place. A middle dot
//! replaces a space one-for-one, and a tab becomes an arrow followed by spaces up
//! to its tab stop — in a monospace font that is the same advance the tab had.

/** One run of a rendered line: `ghost` runs are the markers, drawn dim. */
export interface Seg {
  text: string;
  color?: string;
  ghost?: boolean;
}

export const SPACE_MARK = "\u00b7"; // ·
export const TAB_MARK = "\u2192"; // →

/** Split a line into segments, replacing whitespace with visible marks. Coloured
 *  runs (from the highlighter) are preserved: each is visualized in place, with
 *  the tab stops counted across the whole line so the columns stay true. */
export function visualize(
  runs: { content: string; color?: string }[] | undefined,
  line: string,
  tab = 4,
): Seg[] {
  const parts = runs?.length ? runs : [{ content: line, color: undefined }];
  const out: Seg[] = [];
  let col = 0; // visual column, carried across runs so tabs land on real stops
  const push = (text: string, color: string | undefined, ghost: boolean) => {
    const last = out[out.length - 1];
    if (last && last.ghost === ghost && last.color === color) last.text += text;
    else out.push({ text, color, ghost });
  };
  for (const run of parts) {
    let plain = "";
    for (const ch of run.content) {
      if (ch === " ") {
        if (plain) {
          push(plain, run.color, false);
          plain = "";
        }
        push(SPACE_MARK, run.color, true);
        col++;
      } else if (ch === "\t") {
        if (plain) {
          push(plain, run.color, false);
          plain = "";
        }
        const width = tab - (col % tab);
        push(TAB_MARK + " ".repeat(width - 1), run.color, true);
        col += width;
      } else {
        plain += ch;
        col++;
      }
    }
    if (plain) push(plain, run.color, false);
  }
  return out;
}

/** What to call a file's line endings in the UI. */
export function endingLabel(kind: "crlf" | "lf" | "mixed" | "none"): string {
  return kind === "crlf" ? "CRLF" : kind === "lf" ? "LF" : kind === "mixed" ? "mixed CRLF/LF" : "";
}
