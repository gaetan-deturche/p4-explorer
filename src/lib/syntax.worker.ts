//! Syntax highlighting, off the main thread.
//!
//! Shiki tokenizes a whole file in one call, because the grammar is a state
//! machine: a line inside a block comment is only known to be inside one from
//! the lines before it. That cannot be split into "just the visible rows", and
//! on a big file it is seconds of work — 17942 lines of HLSLMaterialTranslator
//! took 1.3s in a plain node run, and a diff tokenizes BOTH sides — during which
//! the window did not respond at all.
//!
//! So it runs here instead. The pane paints its text immediately, uncoloured,
//! and repaints when the runs arrive.

import { tokenizeLinesSync, type TokenRun } from "./syntax";

export interface TokenRequest {
  id: number;
  text: string;
  lang: string;
  dark: boolean;
}
export interface TokenReply {
  id: number;
  lines: TokenRun[][] | null;
}

self.onmessage = async (e: MessageEvent<TokenRequest>) => {
  const { id, text, lang, dark } = e.data;
  let lines: TokenRun[][] | null = null;
  try {
    lines = await tokenizeLinesSync(text, lang, dark);
  } catch {
    lines = null; // unhighlighted is fine
  }
  (self as unknown as Worker).postMessage({ id, lines } satisfies TokenReply);
};
