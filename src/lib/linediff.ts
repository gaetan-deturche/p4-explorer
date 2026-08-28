//! Line diff for the in-app side-by-side viewer: Myers O(ND) on interned lines
//! (common prefix/suffix trimmed first), del/add runs paired into "mod" rows,
//! and an intra-line changed range per mod row. Pure — no reactive state.

export interface DiffSide {
  no: number; // 1-based line number in its file
  text: string;
}

export interface DiffRow {
  type: "same" | "del" | "add" | "mod";
  l?: DiffSide;
  r?: DiffSide;
  lh?: [number, number]; // intra-line changed range [start, end) — mod rows only
  rh?: [number, number];
}

/** How lines are compared. The rows always carry the ORIGINAL text; only the
 *  comparison changes, so "ignore whitespace" hides a re-indent from the diff
 *  without hiding it from the reader. */
export interface DiffOptions {
  /** Treat lines that differ only in whitespace as equal. Runs of whitespace
   *  collapse to one space and the ends are trimmed, which covers a re-indent,
   *  tabs-to-spaces, and trailing space alike. */
  ignoreWhitespace?: boolean;
}

/** The string two lines are compared BY. */
export function lineKey(line: string, opts?: DiffOptions): string {
  return opts?.ignoreWhitespace ? line.replace(/\s+/g, " ").trim() : line;
}

/** Which line endings a file uses — the one invisible difference that survives
 *  every comparison here, since the diff strips CR before looking at a line. It
 *  is reported so the viewer can say so out loud. */
export function lineEndings(text: string): "crlf" | "lf" | "mixed" | "none" {
  const crlf = (text.match(/\r\n/g) ?? []).length;
  const lf = (text.match(/\n/g) ?? []).length - crlf;
  if (crlf && lf) return "mixed";
  if (crlf) return "crlf";
  if (lf) return "lf";
  return "none";
}

function splitLines(s: string): string[] {
  const lines = s.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop(); // trailing \n
  return lines.map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l));
}

/** Intern lines to ints so the Myers inner loop compares numbers. Interning by
 *  the comparison KEY is what makes "ignore whitespace" work all the way down:
 *  two lines with the same key get the same int and Myers calls them equal. */
function intern(a: string[], b: string[], opts?: DiffOptions): { ia: Int32Array; ib: Int32Array } {
  const ids = new Map<string, number>();
  const get = (s: string) => {
    let v = ids.get(s);
    if (v === undefined) {
      v = ids.size;
      ids.set(s, v);
    }
    return v;
  };
  const ia = new Int32Array(a.length);
  const ib = new Int32Array(b.length);
  for (let i = 0; i < a.length; i++) ia[i] = get(lineKey(a[i], opts));
  for (let i = 0; i < b.length; i++) ib[i] = get(lineKey(b[i], opts));
  return { ia, ib };
}

// Myers greedy diff returning "same/del/add" ops over the two line arrays.
// Bounded: if D exceeds MAXD (pathological inputs), fall back to one del-block +
// one add-block for the remaining middle — still correct, just coarse.
type Op = { type: "same" | "del" | "add"; count: number };

function myers(ia: Int32Array, ib: Int32Array): Op[] {
  const n = ia.length;
  const m = ib.length;
  if (n === 0 && m === 0) return [];
  if (n === 0) return [{ type: "add", count: m }];
  if (m === 0) return [{ type: "del", count: n }];
  const MAXD = 4096;
  const max = Math.min(n + m, MAXD);
  const offset = max;
  const v = new Int32Array(2 * max + 1);
  const trace: Int32Array[] = [];
  let found = -1;
  for (let d = 0; d <= max; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && v[offset + k - 1] < v[offset + k + 1])) {
        x = v[offset + k + 1]; // down: insertion
      } else {
        x = v[offset + k - 1] + 1; // right: deletion
      }
      let y = x - k;
      while (x < n && y < m && ia[x] === ib[y]) {
        x++;
        y++;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) {
        found = d;
        break;
      }
    }
    if (found >= 0) break;
  }
  if (found < 0) {
    // Too divergent — coarse fallback (whole middle replaced).
    return [
      { type: "del", count: n },
      { type: "add", count: m },
    ];
  }
  // Backtrack the D-path into ops (built in reverse). trace[d] is the V state
  // BEFORE iteration d ran, i.e. the state the step at depth d branched from.
  const rev: Op[] = [];
  let x = n;
  let y = m;
  for (let d = found; d > 0; d--) {
    const pv = trace[d];
    const k = x - y;
    const pk =
      k === -d || (k !== d && pv[offset + k - 1] < pv[offset + k + 1])
        ? k + 1 // came down (insertion)
        : k - 1; // came right (deletion)
    const px = pv[offset + pk];
    const py = px - pk;
    // The single step lands here; the snake (equal run) then slides to (x, y).
    const stepX = pk === k + 1 ? px : px + 1;
    const snake = x - stepX;
    if (snake > 0) rev.push({ type: "same", count: snake });
    rev.push({ type: pk === k + 1 ? "add" : "del", count: 1 });
    x = px;
    y = py;
  }
  if (x > 0) rev.push({ type: "same", count: x }); // leading equal run (d=0 snake)
  const ops = rev.reverse();
  // Merge adjacent same-type ops.
  const merged: Op[] = [];
  for (const op of ops) {
    const last = merged[merged.length - 1];
    if (last && last.type === op.type) last.count += op.count;
    else merged.push({ ...op });
  }
  return merged;
}

/** Intra-line changed range: common prefix/suffix trimmed, middle highlighted. */
function charRange(a: string, b: string): { la: [number, number]; lb: [number, number] } {
  let p = 0;
  const max = Math.min(a.length, b.length);
  while (p < max && a[p] === b[p]) p++;
  let sa = a.length;
  let sb = b.length;
  while (sa > p && sb > p && a[sa - 1] === b[sb - 1]) {
    sa--;
    sb--;
  }
  return { la: [p, sa], lb: [p, sb] };
}

// --- where to place an ambiguous run ---------------------------------------
//
// Myers is free to put an insertion (or deletion) anywhere its boundary lines
// repeat, and it takes the earliest such position. Both extremes are wrong half
// the time:
//
//   appended after a block with the same tail   → the earliest position cuts the
//     PREVIOUS block in half and shows its tail as part of the addition
//   prepended before a block with the same head → the latest position leaves the
//     run ending on an opening brace it never closes
//
// So every position in the window is scored and the best one wins. This is the
// same idea as git's indent heuristic (Haggerty, git 2.11, on by default since
// 2.14): score each candidate split by how well it lines up with the structure of
// the code, rather than picking an extreme. The weights here are our own and
// documented below — not git's constants, which this does not attempt to
// reproduce.
//
// The signals, in order of how much they matter:
//
//   the run closes a block it never opened  a run whose running brace depth goes
//     negative starts INSIDE something else — the ValueHeatMap case exactly
//   the run does not balance its braces     it borrows or leaves one
//   it begins with a closer / ends with an opener   the same fault seen from the
//     line rather than from the depth, which catches `}` and `{` on their own
//   a blank line (or the file edge) on either side  a split at a paragraph break
//   a shallow first line                    top-level code is a likelier boundary
//     than the middle of an indented body; this is what carries languages with no
//     braces, where indentation is the only structure there is
//
// Ties go to the later position: appending after something complete is the more
// common change, and it is what the earliest-position bug looked like.

const SLIDE_CAP = 100; // a pathological run of repeated lines is not worth scoring

function indentOf(line: string): number {
  let n = 0;
  for (const ch of line) {
    if (ch === " ") n += 1;
    else if (ch === "\t") n += 4;
    else break;
  }
  return n;
}
function isBlank(line: string | undefined): boolean {
  return line === undefined || line.trim() === "";
}
/** The line with quoted spans and a trailing line comment removed, so a brace in
 *  a string or a comment does not count as structure. Crude on purpose: it only
 *  has to be right often enough to rank candidates. */
function codeOnly(line: string): string {
  const noStr = line.replace(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/g, '""');
  const at = noStr.indexOf("//");
  return at >= 0 ? noStr.slice(0, at) : noStr;
}
/** Running brace depth over a run: where it ends, and how low it dips. A dip
 *  below zero means the run closed a block that was opened before it started. */
function depthOf(lines: string[]): { end: number; min: number } {
  let d = 0;
  let min = 0;
  for (const line of lines) {
    for (const ch of codeOnly(line)) {
      if (ch === "{" || ch === "(" || ch === "[") d++;
      else if (ch === "}" || ch === ")" || ch === "]") d--;
      if (d < min) min = d;
    }
  }
  return { end: d, min };
}
function closesBlock(line: string): boolean {
  const s = codeOnly(line).trim();
  return /^[}\)\]]/.test(s) || /^(end|fi|done|esac)\b/.test(s);
}
function opensBlock(line: string): boolean {
  const s = codeOnly(line).trim();
  return /[{(\[:]$/.test(s);
}

/** How good a placement is. Higher wins; see the note above for the reasoning. */
function placementScore(run: string[], before: string | undefined, after: string | undefined): number {
  const firstNB = run.find((l) => !isBlank(l));
  const lastNB = [...run].reverse().find((l) => !isBlank(l));
  let s = 0;
  const { end, min } = depthOf(run);
  if (min < 0) s -= 30; // starts inside a block it did not open
  if (end !== 0) s -= 20; // leaves one open, or closes one too many
  if (firstNB && closesBlock(firstNB)) s -= 25;
  if (lastNB && opensBlock(lastNB)) s -= 25;
  if (isBlank(before)) s += 8; // a paragraph break, or the top of the file
  if (isBlank(after)) s += 8;
  s -= 0.5 * Math.min(indentOf(firstNB ?? ""), 16); // prefer a shallow start
  return s;
}

/** Move each pure insertion / deletion to the best-scoring position in its
 *  ambiguity window. Paired del+add runs (a modification) are left alone: their
 *  rows are matched line to line, which is what the intra-line highlight is
 *  computed from, and moving them would break that pairing. */
function slideRuns(a: string[], b: string[], ops: Op[], opts?: DiffOptions): Op[] {
  const same = (x: string | undefined, y: string | undefined) =>
    x !== undefined && y !== undefined && lineKey(x, opts) === lineKey(y, opts);
  const out = ops.map((o) => ({ ...o }));
  let ia = 0;
  let ib = 0;
  for (let i = 0; i < out.length; i++) {
    if (out[i].type === "same") {
      ia += out[i].count;
      ib += out[i].count;
      continue;
    }
    let j = i;
    let dels = 0;
    let adds = 0;
    while (j < out.length && out[j].type !== "same") {
      if (out[j].type === "del") dels += out[j].count;
      else adds += out[j].count;
      j++;
    }
    const pure = dels === 0 && adds > 0 ? "add" : adds === 0 && dels > 0 ? "del" : "";
    if (pure) {
      const lines = pure === "add" ? b : a;
      const at = pure === "add" ? ib : ia;
      const len = pure === "add" ? adds : dels;
      const prev = i > 0 ? out[i - 1] : undefined;
      const next = out[j];
      // The window: down while the first line repeats after the run, up while the
      // last line repeats before it.
      let down = 0;
      if (next && next.type === "same") {
        while (
          down < next.count &&
          down < SLIDE_CAP &&
          same(lines[at + down], lines[at + len + down])
        ) {
          down++;
        }
      }
      let up = 0;
      if (prev && prev.type === "same") {
        while (
          up < prev.count &&
          up < SLIDE_CAP &&
          same(lines[at + len - 1 - up], lines[at - 1 - up])
        ) {
          up++;
        }
      }
      if (down > 0 || up > 0) {
        let best = 0;
        let bestScore = -Infinity;
        for (let shift = -up; shift <= down; shift++) {
          const from = at + shift;
          const run = lines.slice(from, from + len);
          const score =
            placementScore(run, lines[from - 1], lines[from + len]) + 0.001 * (shift + up);
          if (score > bestScore) {
            bestScore = score;
            best = shift;
          }
        }
        if (best !== 0) {
          // Moving the run by `best` means moving that many `same` lines across
          // it, from one neighbour to the other.
          const move = Math.abs(best);
          if (best > 0) {
            next!.count -= move;
            if (prev && prev.type === "same") prev.count += move;
            else {
              out.splice(i, 0, { type: "same", count: move });
              i++;
              j++;
            }
            ia += move;
            ib += move;
            if (next!.count === 0) out.splice(j, 1);
          } else {
            prev!.count -= move;
            if (next && next.type === "same") next.count += move;
            else out.splice(j, 0, { type: "same", count: move });
            ia -= move;
            ib -= move;
            if (prev!.count === 0) {
              out.splice(i - 1, 1);
              i--;
              j--;
            }
          }
        }
      }
    }
    ia += dels;
    ib += adds;
    i = j - 1;
  }
  return out.filter((o) => o.count > 0);
}

/** Aligned side-by-side rows for two file contents. */
export function diffLines(leftText: string, rightText: string, opts?: DiffOptions): DiffRow[] {
  const a = splitLines(leftText);
  const b = splitLines(rightText);
  const same = (x: string, y: string) => lineKey(x, opts) === lineKey(y, opts);

  // Trim common prefix/suffix before Myers — the dominant cost saver.
  let pre = 0;
  const maxPre = Math.min(a.length, b.length);
  while (pre < maxPre && same(a[pre], b[pre])) pre++;
  let sufA = a.length;
  let sufB = b.length;
  while (sufA > pre && sufB > pre && same(a[sufA - 1], b[sufB - 1])) {
    sufA--;
    sufB--;
  }

  const { ia, ib } = intern(a.slice(pre, sufA), b.slice(pre, sufB), opts);
  const raw: Op[] = [];
  if (pre > 0) raw.push({ type: "same", count: pre });
  raw.push(...myers(ia, ib));
  if (a.length - sufA > 0) raw.push({ type: "same", count: a.length - sufA });
  const ops = slideRuns(a, b, raw, opts);

  // Expand ops into rows, pairing each del-run with the add-run that follows it
  // (line i of the deletion pairs with line i of the insertion → "mod" rows).
  const rows: DiffRow[] = [];
  let la = 0;
  let lb = 0;
  let i = 0;
  while (i < ops.length) {
    const op = ops[i];
    if (op.type === "same") {
      for (let k = 0; k < op.count; k++) {
        rows.push({
          type: "same",
          l: { no: la + 1, text: a[la] },
          r: { no: lb + 1, text: b[lb] },
        });
        la++;
        lb++;
      }
      i++;
      continue;
    }
    // Collect the full changed hunk (consecutive del/add runs in any order).
    let dels = 0;
    let adds = 0;
    while (i < ops.length && ops[i].type !== "same") {
      if (ops[i].type === "del") dels += ops[i].count;
      else adds += ops[i].count;
      i++;
    }
    const paired = Math.min(dels, adds);
    for (let k = 0; k < paired; k++) {
      const lt = a[la];
      const rt = b[lb];
      const { la: lh, lb: rh } = charRange(lt, rt);
      rows.push({
        type: "mod",
        l: { no: la + 1, text: lt },
        r: { no: lb + 1, text: rt },
        lh,
        rh,
      });
      la++;
      lb++;
    }
    for (let k = paired; k < dels; k++) {
      rows.push({ type: "del", l: { no: la + 1, text: a[la] } });
      la++;
    }
    for (let k = paired; k < adds; k++) {
      rows.push({ type: "add", r: { no: lb + 1, text: b[lb] } });
      lb++;
    }
  }
  return rows;
}

/** First row index of every change block (for prev/next navigation). */
export function changeBlocks(rows: DiffRow[]): number[] {
  const out: number[] = [];
  let inBlock = false;
  for (let i = 0; i < rows.length; i++) {
    const changed = rows[i].type !== "same";
    if (changed && !inBlock) out.push(i);
    inBlock = changed;
  }
  return out;
}
