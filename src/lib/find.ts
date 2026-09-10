//! Finding text in a windowed pane.
//!
//! The webview's own Ctrl+F searches the DOM, and these panes only render the
//! rows on screen — sixty or so out of twenty thousand. So it reported a
//! fraction of the matches and fought the virtual list trying to scroll to
//! them. This searches the LINES instead, which are all there whether or not
//! they are drawn.
//!
//! Pure: no DOM, no runes. What a window supplies is its panes' lines and a way
//! to turn a hit into a y position; everything else is here, and testable.

/** One occurrence: which pane, which line of it, and the range within it. */
export interface Hit {
  pane: number;
  line: number;
  start: number;
  /** Exclusive, so `line.slice(start, end)` is the match. */
  end: number;
}

/** Index of `needle` in `haystack` at or after `from`, ignoring case.
 *
 *  Not `toLowerCase()` on the whole line: lowering can CHANGE LENGTH for a few
 *  characters (İ becomes two), which would shift every index after it and put
 *  the highlight on the wrong glyphs. The fast path is used only when the
 *  lengths agree, which is every line anyone will actually search.
 */
function indexOfCI(haystack: string, needle: string, from: number): number {
  const h = haystack.toLowerCase();
  const n = needle.toLowerCase();
  if (h.length === haystack.length && n.length === needle.length) return h.indexOf(n, from);
  for (let i = from; i + needle.length <= haystack.length; i++) {
    let ok = true;
    for (let k = 0; k < needle.length; k++) {
      if (haystack[i + k].toLowerCase() !== needle[k].toLowerCase()) {
        ok = false;
        break;
      }
    }
    if (ok) return i;
  }
  return -1;
}

/** Every occurrence of `query` in one line, left to right and non-overlapping. */
export function rangesInLine(
  line: string,
  query: string,
  caseSensitive = false,
): [number, number][] {
  if (!query) return [];
  const out: [number, number][] = [];
  let at = 0;
  for (;;) {
    const i = caseSensitive ? line.indexOf(query, at) : indexOfCI(line, query, at);
    if (i < 0) return out;
    out.push([i, i + query.length]);
    at = i + query.length; // non-overlapping: "aa" hits "aaa" once
  }
}

/** Every occurrence across a window's panes, pane by pane. The caller orders
 *  them by what is on screen — only it knows how its rows are laid out. */
export function findHits(
  panes: readonly (readonly string[])[],
  query: string,
  caseSensitive = false,
): Hit[] {
  const out: Hit[] = [];
  if (!query) return out;
  panes.forEach((lines, pane) => {
    lines.forEach((line, i) => {
      for (const [start, end] of rangesInLine(line, query, caseSensitive)) {
        out.push({ pane, line: i, start, end });
      }
    });
  });
  return out;
}

/** The hits in the order the window draws them.
 *
 *  `rowOf` maps a hit to its y (or row index); hits the window cannot place —
 *  a line in a region that is not rendered — are dropped rather than sorted to
 *  the top, where "next match" would jump somewhere with nothing to show.
 */
export function sortByRow(hits: Hit[], rowOf: (h: Hit) => number | null): Hit[] {
  const placed: { h: Hit; y: number }[] = [];
  for (const h of hits) {
    const y = rowOf(h);
    if (y !== null) placed.push({ h, y });
  }
  placed.sort((a, b) => a.y - b.y || a.h.pane - b.h.pane || a.h.start - b.h.start);
  return placed.map((p) => p.h);
}

/** Ranges per line for one pane, for the renderer to highlight. */
export function rangesByLine(hits: readonly Hit[], pane: number): Map<number, [number, number][]> {
  const out = new Map<number, [number, number][]>();
  for (const h of hits) {
    if (h.pane !== pane) continue;
    const list = out.get(h.line);
    if (list) list.push([h.start, h.end]);
    else out.set(h.line, [[h.start, h.end]]);
  }
  return out;
}

/** The selection, when it is worth marking the other occurrences of.
 *
 *  One line only, and at least two characters: a multi-line selection is a
 *  region rather than a term, and marking every `i` in a file is noise. The
 *  text is returned as selected — untrimmed — because that is what the other
 *  occurrences have to match.
 */
export function selectionTerm(raw: string): string {
  const text = raw.replace(/\r/g, "");
  if (!text || text.includes("\n")) return "";
  return text.trim().length >= 2 ? text : "";
}

/** Next/previous match, wrapping. `current` may be -1 (nothing selected yet),
 *  in which case a step forward lands on the first and back on the last. */
export function stepHit(count: number, current: number, delta: number): number {
  if (count <= 0) return -1;
  if (current < 0) return delta >= 0 ? 0 : count - 1;
  return (((current + delta) % count) + count) % count;
}
