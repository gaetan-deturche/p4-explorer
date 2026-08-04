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

/// Cap on the LCS table. Past this the middle is reported as one big change
/// instead — correct, just coarser than a line-by-line diff.
const MAX_CELLS: usize = 4_000_000;

/// `other` expressed as replacements of ranges of `base`.
fn diff_chunks(base: &[String], other: &[String]) -> Vec<Chunk> {
    // Trim the common ends; only the middle needs the expensive comparison.
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
    if b.is_empty() || o.is_empty() || b.len() * o.len() > MAX_CELLS {
        return vec![Chunk { start: lo, end: lo + b.len(), lines: o.to_vec() }];
    }

    // LCS table, then walk it back into runs of equal / replaced lines.
    let (n, m) = (b.len(), o.len());
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    let at = |x: usize, y: usize| x * (m + 1) + y;
    for x in (0..n).rev() {
        for y in (0..m).rev() {
            t[at(x, y)] = if b[x] == o[y] {
                t[at(x + 1, y + 1)] + 1
            } else {
                t[at(x + 1, y)].max(t[at(x, y + 1)])
            };
        }
    }
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut x, mut y) = (0usize, 0usize);
    let mut pend: Option<Chunk> = None;
    while x < n || y < m {
        let same = x < n && y < m && b[x] == o[y];
        if same {
            if let Some(c) = pend.take() {
                chunks.push(c);
            }
            x += 1;
            y += 1;
            continue;
        }
        let c = pend.get_or_insert(Chunk { start: lo + x, end: lo + x, lines: Vec::new() });
        // Prefer the direction the table says keeps more of the common run.
        if y >= m || (x < n && t[at(x + 1, y)] >= t[at(x, y + 1)]) {
            x += 1;
            c.end = lo + x;
        } else {
            c.lines.push(o[y].clone());
            y += 1;
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
        // Two long, fully different middles: past the cell cap, so the whole
        // middle must come back as a single change rather than line-by-line.
        let n = 3000;
        let base: Vec<String> = (0..n).map(|i| format!("b{i}")).collect();
        let ours: Vec<String> = (0..n).map(|i| format!("o{i}")).collect();
        let rs = merge3(&base, &ours, &base);
        assert!(!rs.iter().any(Region::is_conflict));
        assert_eq!(flat(&rs), ours);
    }
}
