//! Diffing two value trees into the rows of a side-by-side tree view.

use serde::Serialize;

use super::archive::Val;

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Same,
    Changed,
    Added,
    Removed,
}

/// One line of the tree, in display order. Children follow their parent with a
/// greater depth; `kids` says whether any do.
#[derive(Serialize, Debug)]
pub struct Row {
    pub depth: u16,
    pub key: String,
    pub left: Option<String>,
    pub right: Option<String>,
    pub status: Status,
    pub kids: bool,
}

/// Rows for every entry of the two roots, which are maps.
pub fn diff(left: &Val, right: &Val) -> Vec<Row> {
    let mut rows = Vec::new();
    children(&mut rows, 0, Some(left), Some(right));
    rows
}

fn is_container(v: &Val) -> bool {
    matches!(v, Val::Map(_) | Val::List(_))
}

pub fn scalar(v: &Val) -> String {
    match v {
        Val::Null => "None".into(),
        Val::Bool(b) => b.to_string(),
        Val::Int(i) => i.to_string(),
        Val::UInt(u) => u.to_string(),
        Val::Float(f) if f.is_finite() && f.abs() < 1e9 => {
            let r = (f * 1e6).round() / 1e6;
            format!("{}", if r == 0.0 { 0.0 } else { r })
        }
        Val::Float(f) => format!("{f}"),
        Val::Str(s) if s.is_empty() => "\"\"".into(),
        Val::Str(s) => s.clone(),
        Val::List(items) => format!("[{}]", items.len()),
        Val::Map(entries) => format!("{{{}}}", entries.len()),
    }
}

/// Two values that differ but print the same at display precision.
fn exact(v: &Val) -> String {
    match v {
        Val::Float(f) => format!("{f:?}"),
        other => scalar(other),
    }
}

fn push(rows: &mut Vec<Row>, depth: u16, key: String, l: Option<String>, r: Option<String>, status: Status, kids: bool) -> usize {
    rows.push(Row { depth, key, left: l, right: r, status, kids });
    rows.len() - 1
}

/// A subtree present on one side only.
fn one_side(rows: &mut Vec<Row>, depth: u16, key: String, v: &Val, status: Status) {
    let text = Some(scalar(v));
    let (l, r) = if status == Status::Removed { (text, None) } else { (None, text) };
    push(rows, depth, key, l, r, status, is_container(v) && !empty(v));
    match v {
        Val::Map(entries) => {
            for (k, child) in entries {
                one_side(rows, depth + 1, k.clone(), child, status);
            }
        }
        Val::List(items) => {
            for (i, child) in items.iter().enumerate() {
                one_side(rows, depth + 1, format!("[{i}]"), child, status);
            }
        }
        _ => {}
    }
}

fn empty(v: &Val) -> bool {
    match v {
        Val::Map(m) => m.is_empty(),
        Val::List(l) => l.is_empty(),
        _ => true,
    }
}

/// One keyed entry on both sides (either may be absent). Returns whether it differs.
fn entry(rows: &mut Vec<Row>, depth: u16, key: String, l: Option<&Val>, r: Option<&Val>) -> bool {
    match (l, r) {
        (None, None) => false,
        (Some(l), None) => {
            one_side(rows, depth, key, l, Status::Removed);
            true
        }
        (None, Some(r)) => {
            one_side(rows, depth, key, r, Status::Added);
            true
        }
        (Some(l), Some(r)) => {
            let same_kind = matches!((l, r), (Val::Map(_), Val::Map(_)) | (Val::List(_), Val::List(_)));
            if same_kind {
                let at = push(rows, depth, key, Some(scalar(l)), Some(scalar(r)), Status::Same, false);
                let changed = children(rows, depth + 1, Some(l), Some(r));
                rows[at].kids = rows.len() > at + 1;
                if changed {
                    rows[at].status = Status::Changed;
                }
                return changed;
            }
            if is_container(l) || is_container(r) {
                // A value that changed shape: the old one out, the new one in.
                let at = push(rows, depth, key, Some(scalar(l)), Some(scalar(r)), Status::Changed, true);
                let (lk, rk) = (rows[at].key.clone(), rows[at].key.clone());
                one_side(rows, depth + 1, lk, l, Status::Removed);
                one_side(rows, depth + 1, rk, r, Status::Added);
                return true;
            }
            if l == r {
                push(rows, depth, key, Some(scalar(l)), Some(scalar(r)), Status::Same, false);
                return false;
            }
            let (mut lt, mut rt) = (scalar(l), scalar(r));
            if lt == rt {
                (lt, rt) = (exact(l), exact(r));
            }
            push(rows, depth, key, Some(lt), Some(rt), Status::Changed, false);
            true
        }
    }
}

/// The children of two containers of the same kind.
fn children(rows: &mut Vec<Row>, depth: u16, l: Option<&Val>, r: Option<&Val>) -> bool {
    match (l, r) {
        (Some(Val::Map(a)), Some(Val::Map(b))) => maps(rows, depth, a, b),
        (Some(Val::List(a)), Some(Val::List(b))) => lists(rows, depth, a, b),
        _ => false,
    }
}

/// Keys in the left order, with keys only the right has slotted in after the
/// key that precedes them there.
fn maps(rows: &mut Vec<Row>, depth: u16, a: &[(String, Val)], b: &[(String, Val)]) -> bool {
    let left: std::collections::HashMap<&str, usize> = a.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
    let right: std::collections::HashMap<&str, usize> = b.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
    // Right-only entries, grouped by the left index they follow (None: first).
    let mut after: std::collections::HashMap<Option<usize>, Vec<usize>> = std::collections::HashMap::new();
    let mut anchor: Option<usize> = None;
    for (j, (k, _)) in b.iter().enumerate() {
        match left.get(k.as_str()) {
            Some(&i) => anchor = Some(i),
            None => after.entry(anchor).or_default().push(j),
        }
    }
    let mut changed = false;
    for j in after.remove(&None).unwrap_or_default() {
        changed |= keyed(rows, depth, &b[j].0, None, Some(&b[j].1));
    }
    for (i, (k, v)) in a.iter().enumerate() {
        let r = right.get(k.as_str()).map(|&j| &b[j].1);
        changed |= keyed(rows, depth, k, Some(v), r);
        for j in after.remove(&Some(i)).unwrap_or_default() {
            changed |= keyed(rows, depth, &b[j].0, None, Some(&b[j].1));
        }
    }
    changed
}

/// A map entry. A package saves a bool property only when it differs from the
/// default, so a bool missing on one side holds the opposite value there:
/// "false → true", not "added true".
fn keyed(rows: &mut Vec<Row>, depth: u16, key: &str, l: Option<&Val>, r: Option<&Val>) -> bool {
    match (l, r) {
        (Some(Val::Bool(b)), None) => entry(rows, depth, key.to_string(), l, Some(&Val::Bool(!b))),
        (None, Some(Val::Bool(b))) => entry(rows, depth, key.to_string(), Some(&Val::Bool(!b)), r),
        _ => entry(rows, depth, key.to_string(), l, r),
    }
}

/// Elements matched by content (longest common subsequence); in each gap the
/// removed and added elements are paired up as edits of one another, which is
/// what an edited struct in an array is.
fn lists(rows: &mut Vec<Row>, depth: u16, a: &[Val], b: &[Val]) -> bool {
    let pairs = match_elements(a, b);
    let mut changed = false;
    let (mut i, mut j) = (0usize, 0usize);
    let gap = |rows: &mut Vec<Row>, i0: usize, i1: usize, j0: usize, j1: usize, changed: &mut bool| {
        let paired = (i1 - i0).min(j1 - j0);
        for n in 0..paired {
            *changed |= entry(rows, depth, format!("[{}]", j0 + n), Some(&a[i0 + n]), Some(&b[j0 + n]));
        }
        for n in i0 + paired..i1 {
            *changed |= entry(rows, depth, format!("[{n}]"), Some(&a[n]), None);
        }
        for n in j0 + paired..j1 {
            *changed |= entry(rows, depth, format!("[{n}]"), None, Some(&b[n]));
        }
    };
    for (pi, pj) in pairs {
        gap(rows, i, pi, j, pj, &mut changed);
        entry(rows, depth, format!("[{pj}]"), Some(&a[pi]), Some(&b[pj]));
        i = pi + 1;
        j = pj + 1;
    }
    gap(rows, i, a.len(), j, b.len(), &mut changed);
    changed
}

/// Index pairs of equal elements, in order. Common ends are matched directly;
/// the middle by LCS when small enough, else not at all (paired by position).
fn match_elements(a: &[Val], b: &[Val]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut lo = 0;
    while lo < a.len() && lo < b.len() && a[lo] == b[lo] {
        out.push((lo, lo));
        lo += 1;
    }
    let (mut ea, mut eb) = (a.len(), b.len());
    let mut tail = Vec::new();
    while ea > lo && eb > lo && a[ea - 1] == b[eb - 1] {
        ea -= 1;
        eb -= 1;
        tail.push((ea, eb));
    }
    let (n, m) = (ea - lo, eb - lo);
    if n > 0 && m > 0 && n * m <= 4_000_000 {
        // table[i][j] = LCS length of a[lo+i..ea] and b[lo+j..eb]
        let w = m + 1;
        let mut table = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i * w + j] = if a[lo + i] == b[lo + j] {
                    table[(i + 1) * w + j + 1] + 1
                } else {
                    table[(i + 1) * w + j].max(table[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a[lo + i] == b[lo + j] {
                out.push((lo + i, lo + j));
                i += 1;
                j += 1;
            } else if table[(i + 1) * w + j] >= table[i * w + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    tail.reverse();
    out.extend(tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(entries: Vec<(&str, Val)>) -> Val {
        Val::map(entries)
    }

    fn statuses(rows: &[Row]) -> Vec<(String, Status)> {
        rows.iter().map(|r| (r.key.clone(), r.status)).collect()
    }

    #[test]
    fn an_unchanged_tree_is_all_same() {
        let a = m(vec![("X", Val::Int(1)), ("S", m(vec![("Y", Val::str("a"))]))]);
        let rows = diff(&a, &a.clone());
        assert!(rows.iter().all(|r| r.status == Status::Same));
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn a_changed_leaf_marks_its_parents_changed() {
        let a = m(vec![("S", m(vec![("Y", Val::Int(1)), ("Z", Val::Int(2))]))]);
        let b = m(vec![("S", m(vec![("Y", Val::Int(1)), ("Z", Val::Int(3))]))]);
        let rows = diff(&a, &b);
        assert_eq!(
            statuses(&rows),
            vec![("S".into(), Status::Changed), ("Y".into(), Status::Same), ("Z".into(), Status::Changed)]
        );
        assert_eq!(rows[2].left.as_deref(), Some("2"));
        assert_eq!(rows[2].right.as_deref(), Some("3"));
    }

    #[test]
    fn a_key_only_the_right_has_lands_after_its_predecessor() {
        let a = m(vec![("A", Val::Int(1)), ("C", Val::Int(3))]);
        let b = m(vec![("A", Val::Int(1)), ("B", Val::Int(2)), ("C", Val::Int(3))]);
        let keys: Vec<String> = diff(&a, &b).iter().map(|r| r.key.clone()).collect();
        assert_eq!(keys, vec!["A", "B", "C"]);
    }

    #[test]
    fn an_inserted_array_element_does_not_shift_the_rest_into_edits() {
        let list = |xs: &[i64]| Val::List(xs.iter().map(|x| Val::Int(*x)).collect());
        let rows = diff(&m(vec![("L", list(&[1, 2, 3]))]), &m(vec![("L", list(&[1, 9, 2, 3]))]));
        let changed: Vec<_> = rows.iter().filter(|r| r.status != Status::Same).map(|r| (r.key.clone(), r.status)).collect();
        assert_eq!(changed, vec![("L".into(), Status::Changed), ("[1]".into(), Status::Added)]);
    }

    #[test]
    fn an_edited_struct_in_an_array_diffs_field_by_field() {
        let item = |v: i64| m(vec![("Name", Val::str("a")), ("Value", Val::Int(v))]);
        let rows = diff(&m(vec![("L", Val::List(vec![item(1)]))]), &m(vec![("L", Val::List(vec![item(2)]))]));
        let changed: Vec<_> = rows.iter().filter(|r| r.status == Status::Changed).map(|r| r.key.clone()).collect();
        assert_eq!(changed, vec!["L", "[0]", "Value"]);
    }

    #[test]
    fn a_bool_saved_on_one_side_only_reads_as_flipped() {
        let rows = diff(&m(vec![]), &m(vec![("TwoSided", Val::Bool(true))]));
        assert_eq!(rows[0].status, Status::Changed);
        assert_eq!((rows[0].left.as_deref(), rows[0].right.as_deref()), (Some("false"), Some("true")));
    }

    #[test]
    fn floats_equal_at_display_precision_still_show_their_difference() {
        let rows = diff(&m(vec![("F", Val::Float(0.1))]), &m(vec![("F", Val::Float(0.1 + 1e-9))]));
        assert_eq!(rows[0].status, Status::Changed);
        assert_ne!(rows[0].left, rows[0].right);
    }
}
