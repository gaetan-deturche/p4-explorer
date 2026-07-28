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

function splitLines(s: string): string[] {
  const lines = s.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop(); // trailing \n
  return lines.map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l));
}

/** Intern lines to ints so the Myers inner loop compares numbers. */
function intern(a: string[], b: string[]): { ia: Int32Array; ib: Int32Array } {
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
  for (let i = 0; i < a.length; i++) ia[i] = get(a[i]);
  for (let i = 0; i < b.length; i++) ib[i] = get(b[i]);
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

/** Aligned side-by-side rows for two file contents. */
export function diffLines(leftText: string, rightText: string): DiffRow[] {
  const a = splitLines(leftText);
  const b = splitLines(rightText);

  // Trim common prefix/suffix before Myers — the dominant cost saver.
  let pre = 0;
  const maxPre = Math.min(a.length, b.length);
  while (pre < maxPre && a[pre] === b[pre]) pre++;
  let sufA = a.length;
  let sufB = b.length;
  while (sufA > pre && sufB > pre && a[sufA - 1] === b[sufB - 1]) {
    sufA--;
    sufB--;
  }

  const { ia, ib } = intern(a.slice(pre, sufA), b.slice(pre, sufB));
  const ops: Op[] = [];
  if (pre > 0) ops.push({ type: "same", count: pre });
  ops.push(...myers(ia, ib));
  if (a.length - sufA > 0) ops.push({ type: "same", count: a.length - sufA });

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
