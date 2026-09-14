//! Three-way line merge (the engine behind the resolve window).
//!
//! Both sides are diffed against the base; the two change lists are then walked
//! together by base range. Changes that don't overlap are both taken, identical
//! changes are taken once, and anything overlapping differently becomes a
//! conflict region for the user to settle.

/// One side's replacement of `base[start..end]`.
#[derive(Debug, Clone, PartialEq)]
struct Chunk {
    start: usize,
    end: usize,
    lines: Vec<String>,
}

/// A stretch of the merged file.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Region {
    /// Untouched by either side.
    Same { lines: Vec<String> },
    /// Only the workspace changed here.
    Ours { base: Vec<String>, lines: Vec<String> },
    /// Only the depot changed here.
    Theirs { base: Vec<String>, lines: Vec<String> },
    /// Both made the same change.
    Both { base: Vec<String>, lines: Vec<String> },
    /// Both changed it differently — the user picks.
    Conflict { base: Vec<String>, ours: Vec<String>, theirs: Vec<String> },
}

impl Region {
    /// The lines this region contributes when auto-resolved (conflicts default
    /// to keeping the workspace side until the user chooses).
    pub fn resolved(&self) -> &[String] {
        match self {
            Region::Same { lines }
            | Region::Ours { lines, .. }
            | Region::Theirs { lines, .. }
            | Region::Both { lines, .. } => lines,
            Region::Conflict { ours, .. } => ours,
        }
    }
    pub fn is_conflict(&self) -> bool {
        matches!(self, Region::Conflict { .. })
    }
}

/// Merge `ours` and `theirs` over their common `base`.
pub fn merge3(base: &[String], ours: &[String], theirs: &[String]) -> Vec<Region> {
    let a = diff_chunks(base, ours);
    let b = diff_chunks(base, theirs);
    let mut out: Vec<Region> = Vec::new();
    let mut pos = 0usize; // how far through base we've emitted
    let (mut i, mut j) = (0usize, 0usize);

    while i < a.len() || j < b.len() {
        // Take whichever change starts first; equal starts are handled together.
        let next = match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) => x.start.min(y.start),
            (Some(x), None) => x.start,
            (None, Some(y)) => y.start,
            (None, None) => break,
        };
        if next > pos {
            push_same(&mut out, &base[pos..next]);
            pos = next;
        }

        // Grow a window over every chunk on either side that overlaps it, so
        // interleaved edits are judged as one region rather than sliced apart.
        let mut end = pos;
        let (i0, j0) = (i, j);
        loop {
            let mut grew = false;
            while let Some(x) = a.get(i) {
                if x.start <= end || (x.start == x.end && x.start == end) {
                    end = end.max(x.end).max(x.start);
                    i += 1;
                    grew = true;
                } else {
                    break;
                }
            }
            while let Some(y) = b.get(j) {
                if y.start <= end || (y.start == y.end && y.start == end) {
                    end = end.max(y.end).max(y.start);
                    j += 1;
                    grew = true;
                } else {
                    break;
                }
            }
            if !grew {
                break;
            }
        }

        let base_slice = base[pos..end.max(pos)].to_vec();
        let ours_slice = rebuild(base, &a[i0..i], pos, end);
        let theirs_slice = rebuild(base, &b[j0..j], pos, end);
        let ours_touched = i > i0;
        let theirs_touched = j > j0;

        out.push(match (ours_touched, theirs_touched) {
            (true, true) if ours_slice == theirs_slice => {
                Region::Both { base: base_slice, lines: ours_slice }
            }
            (true, true) => {
                Region::Conflict { base: base_slice, ours: ours_slice, theirs: theirs_slice }
            }
            (true, false) => Region::Ours { base: base_slice, lines: ours_slice },
            (false, true) => Region::Theirs { base: base_slice, lines: theirs_slice },
            (false, false) => Region::Same { lines: base_slice },
        });
        pos = end.max(pos);
    }
    if pos < base.len() {
        push_same(&mut out, &base[pos..]);
    }
    out
}

/// Coalesce adjacent unchanged stretches so the window shows one block.
fn push_same(out: &mut Vec<Region>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    if let Some(Region::Same { lines: prev }) = out.last_mut() {
        prev.extend_from_slice(lines);
    } else {
        out.push(Region::Same { lines: lines.to_vec() });
    }
}

/// One side's text for `base[from..to]`, applying that side's chunks in order.
fn rebuild(base: &[String], chunks: &[Chunk], from: usize, to: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut at = from;
    for c in chunks {
        if c.start > at {
            out.extend_from_slice(&base[at..c.start.min(to)]);
        }
        out.extend(c.lines.iter().cloned());
        at = at.max(c.end);
    }
    if at < to {
        out.extend_from_slice(&base[at..to]);
    }
    out
}

/// How divergent a pair of files may be before the diff gives up and calls the
/// whole middle one replacement. This is a bound on the number of EDITS, not on
/// the size of the file: two 20,000-line files that differ in ten places cost
/// ten steps here. The table this replaced was bounded by n*m, so a big file
/// with edits near both ends blew through it and the merge stopped locating
/// anything — a 3,000-line conflict whose two sides were almost entirely
/// identical (measured: `EditorViewportClient.cpp`, a 3015 x 2956 middle
/// against a 4,000,000-cell cap).
const MAX_EDITS: usize = 4096;

/// One step of an edit script over two line arrays.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Same(usize),
    Del(usize),
    Add(usize),
}

/// Lines as ints, so the inner loop compares numbers rather than strings.
fn intern(a: &[String], b: &[String]) -> (Vec<u32>, Vec<u32>) {
    let mut ids: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut ia = Vec::with_capacity(a.len());
    let mut ib = Vec::with_capacity(b.len());
    for s in a {
        let next = ids.len() as u32;
        ia.push(*ids.entry(s.as_str()).or_insert(next));
    }
    for s in b {
        let next = ids.len() as u32;
        ib.push(*ids.entry(s.as_str()).or_insert(next));
    }
    (ia, ib)
}

/// Myers' greedy diff: the shortest edit script between `a` and `b`.
///
/// A port of the front-end's `linediff.ts`, so the resolve window and the diff
/// window locate a change in the same place.
fn myers(a: &[u32], b: &[u32]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    if n == 0 && m == 0 {
        return Vec::new();
    }
    if n == 0 {
        return vec![Op::Add(m)];
    }
    if m == 0 {
        return vec![Op::Del(n)];
    }
    let max = (n + m).min(MAX_EDITS);
    let offset = max as isize;
    let mut v = vec![0isize; 2 * max + 1];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found: Option<usize> = None;
    for d in 0..=max {
        trace.push(v.clone()); // the state the step at depth d branches from
        let dd = d as isize;
        let mut k = -dd;
        while k <= dd {
            let idx = (offset + k) as usize;
            let mut x = if k == -dd || (k != dd && v[idx - 1] < v[idx + 1]) {
                v[idx + 1] // down: an insertion
            } else {
                v[idx - 1] + 1 // right: a deletion
            };
            let mut y = x - k;
            while (x as usize) < n && (y as usize) < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x as usize >= n && y as usize >= m {
                found = Some(d);
                break;
            }
            k += 2;
        }
        if found.is_some() {
            break;
        }
    }
    let Some(found) = found else {
        // Past MAX_EDITS: too divergent to place, so say so in one piece.
        return vec![Op::Del(n), Op::Add(m)];
    };

    // Backtrack the D-path into ops, then put them back in order.
    let mut rev: Vec<Op> = Vec::new();
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (1..=found).rev() {
        let pv = &trace[d];
        let dd = d as isize;
        let k = x - y;
        let idx = (offset + k) as usize;
        let came_down = k == -dd || (k != dd && pv[idx - 1] < pv[idx + 1]);
        let pk = if came_down { k + 1 } else { k - 1 };
        let px = pv[(offset + pk) as usize];
        let py = px - pk;
        // The single step lands here; the snake (equal run) slides on to (x, y).
        let step_x = if came_down { px } else { px + 1 };
        let snake = x - step_x;
        if snake > 0 {
            rev.push(Op::Same(snake as usize));
        }
        rev.push(if came_down { Op::Add(1) } else { Op::Del(1) });
        x = px;
        y = py;
    }
    if x > 0 {
        rev.push(Op::Same(x as usize)); // the leading equal run
    }
    rev.reverse();

    // Merge neighbours of the same kind.
    let mut ops: Vec<Op> = Vec::new();
    for op in rev {
        match (ops.last_mut(), op) {
            (Some(Op::Same(c)), Op::Same(n)) => *c += n,
            (Some(Op::Del(c)), Op::Del(n)) => *c += n,
            (Some(Op::Add(c)), Op::Add(n)) => *c += n,
            _ => ops.push(op),
        }
    }
    ops
}

/// `other` expressed as replacements of ranges of `base`.
fn diff_chunks(base: &[String], other: &[String]) -> Vec<Chunk> {
    // Trim the common ends; only the middle needs the real comparison.
    let mut lo = 0usize;
    while lo < base.len() && lo < other.len() && base[lo] == other[lo] {
        lo += 1;
    }
    let mut hi = 0usize;
    while hi < base.len() - lo && hi < other.len() - lo
        && base[base.len() - 1 - hi] == other[other.len() - 1 - hi]
    {
        hi += 1;
    }
    let (b, o) = (&base[lo..base.len() - hi], &other[lo..other.len() - hi]);
    if b.is_empty() && o.is_empty() {
        return Vec::new();
    }

    let (ib, io) = intern(b, o);
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut x, mut y) = (0usize, 0usize);
    let mut pend: Option<Chunk> = None;
    for op in myers(&ib, &io) {
        match op {
            Op::Same(count) => {
                if let Some(c) = pend.take() {
                    chunks.push(c);
                }
                x += count;
                y += count;
            }
            // A deletion and the insertion beside it are ONE replacement: they
            // share a pending chunk, which is what makes a changed line read as
            // a change rather than as a removal followed by an addition.
            Op::Del(count) => {
                let c = pend.get_or_insert(Chunk { start: lo + x, end: lo + x, lines: Vec::new() });
                x += count;
                c.end = lo + x;
            }
            Op::Add(count) => {
                let c = pend.get_or_insert(Chunk { start: lo + x, end: lo + x, lines: Vec::new() });
                c.lines.extend_from_slice(&o[y..y + count]);
                y += count;
            }
        }
    }
    if let Some(c) = pend.take() {
        chunks.push(c);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }
    /// The merged text when every conflict keeps the workspace side.
    fn flat(rs: &[Region]) -> Vec<String> {
        rs.iter().flat_map(|r| r.resolved().to_vec()).collect()
    }

    #[test]
    fn untouched_file_is_one_same_region() {
        let b = v(&["a", "b", "c"]);
        let rs = merge3(&b, &b, &b);
        assert_eq!(rs, vec![Region::Same { lines: b.clone() }]);
    }

    #[test]
    fn takes_both_sides_when_changes_do_not_overlap() {
        let base = v(&["a", "b", "c", "d", "e"]);
        let ours = v(&["A", "b", "c", "d", "e"]); // first line
        let theirs = v(&["a", "b", "c", "d", "E"]); // last line
        let rs = merge3(&base, &ours, &theirs);
        assert!(!rs.iter().any(Region::is_conflict), "{rs:?}");
        assert_eq!(flat(&rs), v(&["A", "b", "c", "d", "E"]));
    }

    #[test]
    fn identical_edits_collapse_to_both() {
        let base = v(&["a", "b", "c"]);
        let side = v(&["a", "B", "c"]);
        let rs = merge3(&base, &side, &side);
        assert!(!rs.iter().any(Region::is_conflict));
        assert!(rs.iter().any(|r| matches!(r, Region::Both { .. })), "{rs:?}");
        assert_eq!(flat(&rs), side);
    }

    #[test]
    fn different_edits_to_the_same_line_conflict() {
        let base = v(&["a", "b", "c"]);
        let ours = v(&["a", "OURS", "c"]);
        let theirs = v(&["a", "THEIRS", "c"]);
        let rs = merge3(&base, &ours, &theirs);
        let c: Vec<&Region> = rs.iter().filter(|r| r.is_conflict()).collect();
        assert_eq!(c.len(), 1, "{rs:?}");
        assert_eq!(
            *c[0],
            Region::Conflict { base: v(&["b"]), ours: v(&["OURS"]), theirs: v(&["THEIRS"]) }
        );
    }

    #[test]
    fn insertions_on_both_sides_at_the_same_place_conflict() {
        let base = v(&["a", "b"]);
        let ours = v(&["a", "ours1", "b"]);
        let theirs = v(&["a", "theirs1", "b"]);
        let rs = merge3(&base, &ours, &theirs);
        assert_eq!(rs.iter().filter(|r| r.is_conflict()).count(), 1, "{rs:?}");
    }

    #[test]
    fn deletion_on_one_side_only_is_taken() {
        let base = v(&["a", "b", "c"]);
        let ours = v(&["a", "c"]); // dropped b
        let rs = merge3(&base, &ours, &base);
        assert!(!rs.iter().any(Region::is_conflict), "{rs:?}");
        assert_eq!(flat(&rs), ours);
    }

    #[test]
    fn delete_versus_edit_is_a_conflict() {
        let base = v(&["a", "b", "c"]);
        let ours = v(&["a", "c"]); // deleted
        let theirs = v(&["a", "B!", "c"]); // edited
        let rs = merge3(&base, &ours, &theirs);
        assert_eq!(rs.iter().filter(|r| r.is_conflict()).count(), 1, "{rs:?}");
    }

    #[test]
    fn scattered_independent_edits_all_merge() {
        let base: Vec<String> = (0..40).map(|i| format!("line{i}")).collect();
        let mut ours = base.clone();
        let mut theirs = base.clone();
        ours[3] = "ours3".into();
        ours[20] = "ours20".into();
        theirs[10] = "theirs10".into();
        theirs[31] = "theirs31".into();
        let rs = merge3(&base, &ours, &theirs);
        assert!(!rs.iter().any(Region::is_conflict), "{rs:?}");
        let want: Vec<String> = base
            .iter()
            .enumerate()
            .map(|(i, l)| match i {
                3 => "ours3".to_string(),
                20 => "ours20".to_string(),
                10 => "theirs10".to_string(),
                31 => "theirs31".to_string(),
                _ => l.clone(),
            })
            .collect();
        assert_eq!(flat(&rs), want);
    }

    #[test]
    fn oversized_middle_degrades_to_one_chunk_without_hanging() {
        // Two long middles with nothing in common: 6,000 edits, past MAX_EDITS,
        // so the whole middle must come back as a single change rather than
        // line-by-line. This is the fallback, and it should stay reachable —
        // what changed is that it now takes genuine divergence to reach it, not
        // merely a big file.
        let n = 3000;
        let base: Vec<String> = (0..n).map(|i| format!("b{i}")).collect();
        let ours: Vec<String> = (0..n).map(|i| format!("o{i}")).collect();
        let rs = merge3(&base, &ours, &base);
        assert!(!rs.iter().any(Region::is_conflict));
        assert_eq!(flat(&rs), ours);
    }


    #[test]
    fn a_big_file_with_scattered_edits_still_locates_them() {
        // The shape that broke. 7,000 lines; one side edits near the top, the
        // other in six places spread over the bottom half. Trimming the common
        // ends still leaves a ~2,500-line middle on the depot side, and the old
        // table — bounded by n*m cells — gave up on it and returned the whole
        // middle as ONE change. Both sides then "touched" that range and the
        // file came back as a single conflict thousands of lines long, nearly
        // all of it identical on both sides.
        //
        // Measured on EditorViewportClient.cpp: a 3015 x 2956 middle against a
        // 4,000,000-cell cap. Not one of these edits touches the same line, so
        // there is nothing here for a person to settle — but with the depot side
        // reduced to one slab, our line 3200 fell inside it and the two "both
        // touched this" into a conflict spanning the lot.
        let n = 7000usize;
        let base: Vec<String> = (0..n).map(|i| format!("line {i}")).collect();
        let mut ours = base.clone();
        ours[300] = "ours near the top".to_string();
        ours[3200] = "ours in the middle".to_string(); // inside the depot's stretch
        let mut theirs = base.clone();
        for i in (3000..6000).step_by(500) {
            theirs[i] = format!("theirs {i}");
        }

        let rs = merge3(&base, &ours, &theirs);
        assert!(
            !rs.iter().any(Region::is_conflict),
            "no side edited the same place: {} regions",
            rs.len()
        );
        let mut want = base.clone();
        want[300] = "ours near the top".to_string();
        want[3200] = "ours in the middle".to_string();
        for i in (3000..6000).step_by(500) {
            want[i] = format!("theirs {i}");
        }
        assert_eq!(flat(&rs), want);

        // And they are LOCATED: every changed region is one line, not a slab.
        let biggest = rs
            .iter()
            .filter(|r| !matches!(r, Region::Same { .. }))
            .map(|r| r.resolved().len())
            .max()
            .unwrap_or(0);
        assert_eq!(biggest, 1, "a changed region should be the line that changed");
    }
}
