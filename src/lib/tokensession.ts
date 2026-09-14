//! Colouring a window of a file without tokenizing the whole thing first.
//!
//! A TextMate grammar is a state machine: a line inside a block comment is only
//! known to be inside one from the lines before it, so a file cannot be coloured
//! from the middle. That is why the naive approach tokenizes everything up front
//! — seconds of work on a big file, all of it wasted on the ~60 rows on screen.
//!
//! Shiki hands out that state (`getLastGrammarState`) and takes it back
//! (`grammarState` on `codeToTokensBase`), so the file only has to be walked for
//! its STATE, keeping a checkpoint every few hundred lines. Colouring a window is
//! then: resume from the checkpoint below it, tokenize the handful of lines from
//! there, and keep the ones asked for.
//!
//! The logic lives here, apart from Shiki, so it can be tested against a fake
//! grammar — the property being that a window coloured this way is identical to
//! the same lines coloured in one whole-file pass.
//!
//! With one deliberate exception. A grammar can get a file WRONG and stay wrong:
//! VS Code's C++ grammar reads `class X : MACRO("Y");` as an inheritance clause,
//! swallows the opening quote, and the closing one opens a string it never
//! closes — every line to the end of the file is then one string token. Carrying
//! that state faithfully reproduces the damage, so a tokenizer may declare a
//! state `lost`, and a checkpoint holding one re-anchors. The file then agrees
//! with a whole-file pass everywhere except the stretch the grammar had already
//! ruined.

import type { TokenRun } from "./syntax";

/** What this needs of a grammar. `state` is opaque and belongs to the tokenizer:
 *  it is Shiki's `GrammarState`, which cannot leave the thread it was made on. */
export interface Tokenizer {
  /** The state after tokenizing `text`, starting from `from` (null = the top). */
  stateAfter(text: string, from: unknown | null): unknown;
  /** Coloured runs per line of `text`, tokenized starting from `from`. */
  tokens(text: string, from: unknown | null): TokenRun[][];
  /** Optional: says this state must NOT be carried past a checkpoint, because
   *  the grammar is plainly lost in it. Absent means carry everything, which is
   *  what a grammar that never goes wrong would want. */
  lost?(state: unknown): boolean;
}

/** One file being coloured a window at a time. */
export class TokenSession {
  private readonly lines: string[];
  private readonly tk: Tokenizer;
  private readonly every: number;
  /** State at the start of line `k * every`; `[0]` is always the top of file. */
  private readonly marks: (unknown | null)[] = [null];

  /**
   * @param every lines between checkpoints. Larger means less state kept and
   *   more lines re-tokenized per window; a few hundred keeps both small.
   */
  constructor(text: string, tk: Tokenizer, every = 400) {
    // CR is stripped to match linediff's splitLines, so line i here is line i
    // there.
    this.lines = text.replace(/\r/g, "").split("\n");
    this.tk = tk;
    this.every = Math.max(1, every);
  }

  get lineCount(): number {
    return this.lines.length;
  }

  /** Walk the state forward until line `upto` can be resumed from. Cheap to call
   *  repeatedly: it never redoes a stretch it has already walked. */
  primeTo(upto: number): void {
    const want = Math.min(Math.floor(upto / this.every), Math.floor(this.lines.length / this.every));
    while (this.marks.length - 1 < want) {
      const k = this.marks.length - 1;
      const chunk = this.lines.slice(k * this.every, (k + 1) * this.every).join("\n");
      const state = this.tk.stateAfter(chunk, this.marks[k]);
      // A lost grammar stays lost to the end of the file, so a state that cannot
      // be right is dropped rather than carried: the next checkpoint starts from
      // the top of the language again and the damage stops there.
      this.marks.push(this.tk.lost?.(state) ? null : state);
    }
  }

  /** Coloured runs for `count` lines starting at `from` (0-based). */
  window(from: number, count: number): TokenRun[][] {
    if (count <= 0 || from >= this.lines.length) return [];
    const start = Math.max(0, from);
    const end = Math.min(this.lines.length, start + count);
    this.primeTo(start);
    const k = Math.min(Math.floor(start / this.every), this.marks.length - 1);
    const base = k * this.every;
    // From the checkpoint to the end of the window: the lines between the
    // checkpoint and the window are tokenized and thrown away, which is what
    // bounds the cost to `every` extra lines however far down the file we are.
    const text = this.lines.slice(base, end).join("\n");
    const runs = this.tk.tokens(text, this.marks[k]);
    return runs.slice(start - base, end - base);
  }
}
