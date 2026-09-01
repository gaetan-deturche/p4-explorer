//! Syntax highlighting, off the main thread — and the state that goes with it.
//!
//! A TextMate grammar is a state machine, so a file cannot be coloured from the
//! middle: what a line means depends on the lines before it. Shiki hands that
//! state out and takes it back, but a `GrammarState` is a live object and cannot
//! be posted between threads — so it stays HERE, and the window is what crosses.
//!
//! The main thread opens a session over a file and then asks for the rows it is
//! about to draw; each answer is ~60 lines of runs rather than the whole file's
//! (69710 of them for HLSLMaterialTranslator.cpp, which is what the one-shot
//! version had to clone back).

import { makeTokenizer, type TokenRun } from "./syntax";
import { TokenSession } from "./tokensession";

interface Open {
  k: "open";
  req: number;
  id: number;
  text: string;
  lang: string;
  dark: boolean;
}
interface Win {
  k: "win";
  req: number;
  id: number;
  from: number;
  count: number;
}
interface Close {
  k: "close";
  req: number;
  id: number;
}

const sessions = new Map<number, TokenSession>();
const post = (msg: { req: number; lines?: number; runs?: TokenRun[][] }) =>
  (self as unknown as Worker).postMessage(msg);

self.onmessage = async (e: MessageEvent<Open | Win | Close>) => {
  const m = e.data;
  try {
    if (m.k === "open") {
      const tk = await makeTokenizer(m.lang, m.dark);
      if (!tk) return post({ req: m.req, lines: -1 });
      const session = new TokenSession(m.text, tk);
      sessions.set(m.id, session);
      // Nothing is tokenized yet: the state is walked only as far as the first
      // window asked for, so opening a file costs nothing.
      post({ req: m.req, lines: session.lineCount });
    } else if (m.k === "win") {
      const session = sessions.get(m.id);
      post({ req: m.req, runs: session ? session.window(m.from, m.count) : [] });
    } else {
      sessions.delete(m.id);
    }
  } catch {
    post({ req: m.req, lines: -1, runs: [] }); // uncoloured is fine
  }
};
