//! Shader source carried in another language's strings, coloured as shader source.
//!
//! A Blender add-on keeps its compute shaders as Python triple-quoted strings —
//! 524 of gpu_bake.py's 831 lines — and the Python grammar paints all of it one
//! string colour, with `% c` of a modulo lit up as a printf placeholder. Nothing
//! marks those strings as GLSL (the source opens with `NAME = """`, and Blender
//! adds `#version` itself), so they are recognised by what is in them.

import type { TokenRun } from "./syntax";
import { TokenSession } from "./tokensession";

/** Content lines [from, to) of `text` (0-based, CR stripped, as TokenSession
 *  splits) that belong to another language. The delimiter lines are not in it. */
export interface Embedded {
  from: number;
  to: number;
  lang: string;
}

/** Things only shader source says. A string needs two DIFFERENT ones: one alone
 *  turns up in prose (a docstring that mentions `vec3`). */
const GLSL_SIGNALS: [string, RegExp][] = [
  ["vector type", /\b[iub]?vec[234]\b/],
  ["matrix type", /\bmat[234](x[234])?\b/],
  ["image load/store", /\bimage(Load|Store|Size)\s*\(/],
  ["texture call", /\btexture(Lod|Fetch|Size)?\s*\(/],
  ["gl_ builtin", /\bgl_[A-Z]\w*/],
  ["void main", /\bvoid\s+main\s*\(/],
  ["preprocessor", /^\s*#\s*(define|if|ifdef|ifndef|elif|else|endif|version|extension|pragma)\b/m],
  ["storage qualifier", /\b(uniform|buffer|shared)\s+\w/],
  ["layout", /\blayout\s*\(/],
  ["barrier", /\b(barrier|memoryBarrier\w*)\s*\(\s*\)/],
  // GLSL-only names (no Python or NumPy namesake), so a string of pure helper
  // functions — vector maths and nothing else — still has a second witness.
  [
    "glsl builtin",
    /\b(lessThan(Equal)?|greaterThan(Equal)?|notEqual|inversesqrt|smoothstep|fract|mix|texelFetch|floatBitsTo(U?Int)|u?intBitsToFloat|bitCount|findLSB|findMSB|atomic(Add|Min|Max|And|Or|Xor|Exchange|CompSwap))\s*\(/,
  ],
];

/** A comment says nothing about whether the lines around it are code. */
const COMMENT = /^\s*(\/\/|\/\*|\*)/;

/** Code is mostly lines that end a statement or open/close a block; prose is
 *  mostly not. */
const SHAPED = /[;{}]\s*(\/\/.*)?$|^\s*[{}]/;

export interface Verdict {
  glsl: boolean;
  signals: string[];
  /** Fraction of the non-blank lines shaped like C-family code. */
  shaped: number;
  lines: number;
}

export function scoreGlsl(body: string[]): Verdict {
  const text = body.join("\n");
  const signals = GLSL_SIGNALS.filter(([, re]) => re.test(text)).map(([name]) => name);
  const nonBlank = body.filter((l) => l.trim());
  const code = nonBlank.filter((l) => !COMMENT.test(l));
  const shaped = code.length ? code.filter((l) => SHAPED.test(l)).length / code.length : 0;
  return {
    // Two witnesses, or one when the text is plainly code: across 233 TechArt
    // docstrings none had a signal and none was more than 20% code-shaped.
    glsl: nonBlank.length >= 2 && ((signals.length >= 2 && shaped >= 0.4) || (signals.length >= 1 && shaped >= 0.6)),
    signals,
    shaped,
    lines: nonBlank.length,
  };
}

/** Every Python triple-quoted string whose WHOLE lines sit between its delimiter
 *  lines, with its body. A one-line string has no such lines and is left out. */
export function pythonBlockStrings(lines: string[]): { from: number; to: number; open: number }[] {
  const out: { from: number; to: number; open: number }[] = [];
  let open: { quote: string; line: number } | null = null;
  for (let i = 0; i < lines.length; i++) {
    const s = lines[i];
    let pos = 0;
    for (;;) {
      if (!open) {
        // Outside a string, a `#` starts a comment: nothing after it opens one.
        const hash = s.indexOf("#", pos);
        const m = /"""|'''/.exec(s.slice(pos));
        if (!m) break;
        const at = pos + m.index;
        if (hash >= 0 && hash < at) break;
        open = { quote: m[0], line: i };
        pos = at + 3;
      } else {
        let at = s.indexOf(open.quote, pos);
        while (at > 0 && s[at - 1] === "\\") at = s.indexOf(open.quote, at + 1);
        if (at < 0) break;
        if (i > open.line + 1) out.push({ from: open.line + 1, to: i, open: open.line });
        open = null;
        pos = at + 3;
      }
    }
  }
  return out;
}

/** The stretches of `text` (in `host`'s language) that another grammar should
 *  colour. Only Python carrying GLSL, for now. */
export function findEmbedded(text: string, host: string): Embedded[] {
  if (host !== "python") return [];
  const lines = text.replace(/\r/g, "").split("\n");
  return pythonBlockStrings(lines)
    .filter((b) => scoreGlsl(lines.slice(b.from, b.to)).glsl)
    .map((b) => ({ from: b.from, to: b.to, lang: "glsl" }));
}


/** How far a coloured embedded token leans toward the host's string colour:
 *  it keeps 60% of its own, so types stay blue and functions yellow while the
 *  block still reads as a string. Mixed in the sRGB-encoded values — in linear
 *  light the blues turned pink. */
const TOWARD_STRING = 0.4;

/** Below this spread between channels a colour is grey, with no hue of its own. */
const GREY_SPREAD = 12;

function rgb(colour: string | undefined): number[] | null {
  const m = /^#([0-9a-f]{6})/i.exec(colour ?? "");
  return m ? [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)) : null;
}

/** `colour` moved `t` of the way toward `toward`. */
function lean(colour: string | undefined, toward: string | undefined, t: number): string | undefined {
  const a = rgb(colour);
  const b = rgb(toward);
  if (!a || !b) return colour;
  return "#" + a.map((v, i) => Math.round(v + (b[i] - v) * t).toString(16).padStart(2, "0")).join("");
}

/** An embedded token's colour, anchored on the host's string colour. Plain
 *  text — grey, the bulk of the code — becomes the string colour itself: half
 *  way toward white it was a near-white peach and the string disappeared. */
function inString(colour: string | undefined, stringColour: string): string | undefined {
  const c = rgb(colour);
  if (c && Math.max(...c) - Math.min(...c) <= GREY_SPREAD) return stringColour;
  return lean(colour, stringColour, TOWARD_STRING);
}

/** The host's colouring, with each embedded stretch coloured by its own session.
 *  The host still walks the whole file, so its state is right on either side of
 *  a string; inside one, its single string colour is replaced line for line. */
export class LayeredSession {
  constructor(
    private readonly host: TokenSession,
    private readonly parts: { at: Embedded; session: TokenSession }[],
  ) {}

  get lineCount(): number {
    return this.host.lineCount;
  }

  window(from: number, count: number): TokenRun[][] {
    const runs = this.host.window(from, count);
    const end = from + runs.length;
    for (const { at, session } of this.parts) {
      const a = Math.max(from, at.from);
      const b = Math.min(end, at.to);
      if (a >= b) continue;
      const inner = session.window(a - at.from, b - a);
      // The host painted these lines its string colour, in whatever theme is
      // showing. A blank line has no run to read it from, so it is taken once.
      let stringColour: string | undefined;
      for (let i = a; i < b && !stringColour; i++) {
        stringColour = runs[i - from]?.find((t) => t.content.trim())?.color;
      }
      for (let i = a; i < b; i++) {
        const row = inner[i - a];
        if (!row) continue;
        const s = stringColour;
        runs[i - from] = s ? row.map((t) => ({ ...t, color: inString(t.color, s) })) : row;
      }
    }
    return runs;
  }
}
