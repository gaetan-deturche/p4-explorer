//! Marking the selection's other occurrences in panes of plain DOM text.
//!
//! Drawn with the CSS Custom Highlight API (`::highlight(occur)`), never as
//! spans: splitting the line into spans rewrote the very text nodes the
//! selection sat in, the browser dropped the selection, the marks went with it,
//! and a double click on part of a word blinked instead of selecting.

import { rangesInLine } from "./find";

const NAME = "occur";

function registry(): HighlightRegistry | undefined {
  return typeof CSS !== "undefined" ? CSS.highlights : undefined;
}

/** Ranges over every occurrence of `query` in each line element's text. */
function rangesIn(lines: Iterable<Element>, query: string): Range[] {
  const out: Range[] = [];
  for (const el of lines) {
    const hits = rangesInLine(el.textContent ?? "", query, true);
    if (!hits.length) continue;
    const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    let node = walk.nextNode() as Text | null;
    let base = 0; // offset of `node` in the line
    const seek = (at: number, end: boolean) => {
      // An end may sit at the very end of a node; a start belongs to the next one.
      while (node && (end ? base + node.length < at : base + node.length <= at)) {
        base += node.length;
        node = walk.nextNode() as Text | null;
      }
      return node;
    };
    for (const [from, to] of hits) {
      const r = document.createRange();
      const a = seek(from, false);
      if (!a) break;
      r.setStart(a, from - base);
      const b = seek(to, true);
      if (!b) break;
      r.setEnd(b, to - base);
      out.push(r);
    }
  }
  return out;
}

/** Keep the occurrences of `query` marked in the line elements `selector`
 *  matches under `roots`, as rows are drawn and dropped. Returns the cleanup. */
export function watchOccurrences(roots: readonly (Element | undefined | null)[], selector: string, query: string): () => void {
  const reg = registry();
  const live = roots.filter((r): r is Element => !!r);
  if (!reg || !query || !live.length) {
    reg?.delete(NAME);
    return () => {};
  }
  const run = () => reg.set(NAME, new Highlight(...rangesIn(live.flatMap((r) => [...r.querySelectorAll(selector)]), query)));
  // Observer callbacks run before the next paint, so a scrolled-in row is never
  // shown unmarked. Highlights do not touch the DOM, so this cannot feed itself.
  const mo = new MutationObserver(run);
  for (const r of live) mo.observe(r, { childList: true, subtree: true, characterData: true });
  run();
  return () => {
    mo.disconnect();
    reg.delete(NAME);
  };
}
