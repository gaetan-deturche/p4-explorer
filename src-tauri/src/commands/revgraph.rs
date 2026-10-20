//! Revision graph: every depot path a file has lived at, its revisions, and the
//! branches, copies and moves between them — P4V's timeline view.
//!
//! Two `filelog` calls build it. `filelog -i` follows the file back through
//! every branch and move it came from and returns each of those paths with its
//! revisions and integration records (114 ms for a file with 4 such paths and 49
//! revisions). Those records also name files OUTSIDE that ancestry: the sources
//! a revision was copied from, and the streams it was branched into. One more
//! `filelog` over those names fetches their revisions (202 ms for 8 of them).

use crate::p4::{self, P4Conn, Record};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphRev {
    pub rev: u32,
    pub change: String,
    pub action: String,
    /// Seconds since the epoch.
    pub time: i64,
    pub user: String,
    pub desc: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphLane {
    pub file: String,
    /// Oldest first.
    pub revs: Vec<GraphRev>,
    /// Not in the file's own ancestry: a source it was copied from, or a branch
    /// it went into.
    pub related: bool,
}

/// One integration, oriented the way the content travelled.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from_file: String,
    /// The last source revision taken.
    pub from_rev: u32,
    /// The first source revision taken (0 = from the start).
    pub from_start: u32,
    pub to_file: String,
    pub to_rev: u32,
    /// branch, copy, merge, move, edit, add, delete, ignore, undo, …
    pub how: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RevGraph {
    pub focus: String,
    pub lanes: Vec<GraphLane>,
    pub edges: Vec<GraphEdge>,
    /// Files this one was branched or copied INTO, left out of `lanes` unless
    /// asked for — P4V hides them too, and they are most of the related files.
    pub hidden_into: usize,
}

/// `#none` → 0, `#30` → 30.
fn rev_num(s: &str) -> u32 {
    s.trim_start_matches('#').parse().unwrap_or(0)
}

/// "branch from" → ("branch", false); "copy into" → ("copy", true): the verb,
/// and whether the record points AWAY from the file it is listed under.
fn how_of(how: &str) -> (String, bool) {
    let outward = how.ends_with(" into") || how.ends_with(" by");
    let verb = how.split(' ').next().unwrap_or(how);
    let verb = match verb {
        "moved" => "move",
        "ignored" => "ignore",
        "undid" | "undone" => "undo",
        v => v,
    };
    (verb.to_string(), outward)
}

/// One `filelog` record: its file, its revisions, and its integration records as
/// edges.
fn parse_record(rec: &Record) -> Option<(String, Vec<GraphRev>, Vec<GraphEdge>)> {
    let file = rec.get("depotFile")?.as_str()?.to_string();
    let s = |k: &str| rec.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut revs = Vec::new();
    let mut edges = Vec::new();
    let mut i = 0;
    while rec.contains_key(&format!("rev{i}")) {
        let rev = rev_num(&s(&format!("rev{i}")));
        revs.push(GraphRev {
            rev,
            change: s(&format!("change{i}")),
            action: s(&format!("action{i}")),
            time: s(&format!("time{i}")).parse().unwrap_or(0),
            user: s(&format!("user{i}")),
            desc: s(&format!("desc{i}")).trim().to_string(),
        });
        let mut j = 0;
        while rec.contains_key(&format!("how{i},{j}")) {
            let (how, outward) = how_of(&s(&format!("how{i},{j}")));
            let other = s(&format!("file{i},{j}"));
            let start = rev_num(&s(&format!("srev{i},{j}")));
            let end = rev_num(&s(&format!("erev{i},{j}")));
            // Both directions name the OTHER end's revision in `erev`: a source
            // revision for "from", the revision created for "into".
            edges.push(if outward {
                GraphEdge { from_file: file.clone(), from_rev: rev, from_start: rev, to_file: other, to_rev: end, how }
            } else {
                GraphEdge { from_file: other, from_rev: end, from_start: start, to_file: file.clone(), to_rev: rev, how }
            });
            j += 1;
        }
        i += 1;
    }
    revs.sort_by_key(|r| r.rev);
    Some((file, revs, edges))
}

/// Lanes and edges from the records of both calls. `ancestry` are the files of
/// the `filelog -i` call; anything else is related.
fn assemble(focus: &str, ancestry: &BTreeSet<String>, recs: &[Record], include_into: bool) -> RevGraph {
    let mut lanes: BTreeMap<String, GraphLane> = BTreeMap::new();
    // Keyed by its two ends: each file lists the same integration, and only the
    // receiving end knows the first source revision ("into" records do not).
    let mut by_ends: BTreeMap<(String, u32, String, u32), GraphEdge> = BTreeMap::new();
    for rec in recs {
        let Some((file, revs, es)) = parse_record(rec) else { continue };
        let related = !ancestry.contains(&file);
        // A path can come twice: `filelog -i` lists an ancestor only up to the
        // revision that was branched from it, its own filelog lists the rest.
        let lane = lanes.entry(file.clone()).or_insert(GraphLane { file, revs: Vec::new(), related });
        for r in revs {
            if !lane.revs.iter().any(|known| known.rev == r.rev) {
                lane.revs.push(r);
            }
        }
        lane.revs.sort_by_key(|r| r.rev);
        for e in es {
            let key = (e.from_file.clone(), e.from_rev, e.to_file.clone(), e.to_rev);
            by_ends
                .entry(key)
                .and_modify(|known| known.from_start = known.from_start.min(e.from_start))
                .or_insert(e);
        }
    }
    let edges: Vec<GraphEdge> = by_ends.into_values().collect();
    // Which related files were sources and which were destinations of the
    // ancestry: a source stays, a destination only when asked for.
    let mut into: BTreeSet<String> = BTreeSet::new();
    let mut from: BTreeSet<String> = BTreeSet::new();
    for e in &edges {
        if ancestry.contains(&e.from_file) && !ancestry.contains(&e.to_file) {
            into.insert(e.to_file.clone());
        }
        if ancestry.contains(&e.to_file) && !ancestry.contains(&e.from_file) {
            from.insert(e.from_file.clone());
        }
    }
    let hidden: BTreeSet<String> = into.difference(&from).cloned().collect();
    let shown: BTreeSet<String> =
        lanes.keys().filter(|f| include_into || !hidden.contains(*f)).cloned().collect();
    let edges: Vec<GraphEdge> =
        edges.into_iter().filter(|e| shown.contains(&e.from_file) && shown.contains(&e.to_file)).collect();
    let mut lanes: Vec<GraphLane> = lanes.into_values().filter(|l| shown.contains(&l.file)).collect();
    // Oldest path on top, as the content flowed.
    lanes.sort_by_key(|l| (l.revs.first().map_or(i64::MAX, |r| r.time), l.file.clone()));
    RevGraph { focus: focus.to_string(), lanes, edges, hidden_into: if include_into { 0 } else { hidden.len() } }
}

/// The revision graph of `depot_file`. `include_into` adds the files it was
/// branched or copied into.
#[tauri::command]
pub async fn revision_graph(conn: P4Conn, depot_file: String, include_into: bool) -> Result<RevGraph, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut recs = p4::run(&conn, &["filelog", "-i", "-L", &depot_file])?;
        let ancestry: BTreeSet<String> =
            recs.iter().filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(String::from)).collect();
        // Every file the integration records name that the first call did not
        // return — and the ancestry itself again: `-i` stops each ancestor at the
        // revision branched from it, so the revisions after that come from here.
        let mut others: BTreeSet<String> = ancestry.clone();
        for rec in &recs {
            if let Some((_, _, es)) = parse_record(rec) {
                for e in es {
                    others.insert(e.from_file);
                    others.insert(e.to_file);
                }
            }
        }
        let others: Vec<String> = others.into_iter().collect();
        for chunk in others.chunks(50) {
            let mut args: Vec<&str> = vec!["filelog", "-L"];
            args.extend(chunk.iter().map(String::as_str));
            // A file on a depot this user cannot read answers with an error for
            // that file alone; the rest still come back.
            if let Ok((more, _notes)) = p4::run_notes(&conn, &args) {
                recs.extend(more);
            }
        }
        Ok(assemble(&depot_file, &ancestry, &recs, include_into))
    })
    .await
    .map_err(|e| format!("revision-graph task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(v: serde_json::Value) -> Record {
        v.as_object().unwrap().clone()
    }

    /// The shapes filelog returned for HurtboxComponent.cpp, trimmed: Main#1 was
    /// branched from SloClap#30, Main#2 copied from it and branched into Weekly
    /// and into Merge_5.8, and SloClap#13 was copied from MergeUnreal#1.
    fn sample() -> Vec<Record> {
        vec![
            rec(json!({
                "depotFile": "//C/Main/H.cpp",
                "rev0": "2", "change0": "201323", "action0": "integrate", "time0": "1787744312", "user0": "a", "desc0": "x",
                "how0,0": "branch into", "file0,0": "//C/Weekly/H.cpp", "srev0,0": "#none", "erev0,0": "#1",
                "how0,1": "branch into", "file0,1": "//C/Merge/H.cpp", "srev0,1": "#none", "erev0,1": "#1",
                "how0,2": "copy from", "file0,2": "//U/SloClap/H.cpp", "srev0,2": "#30", "erev0,2": "#36",
                "rev1": "1", "change1": "188094", "action1": "branch", "time1": "1781533270", "user1": "a", "desc1": "y",
                "how1,0": "branch from", "file1,0": "//U/SloClap/H.cpp", "srev1,0": "#none", "erev1,0": "#30",
            })),
            rec(json!({
                "depotFile": "//U/SloClap/H.cpp",
                "rev0": "36", "change0": "200000", "action0": "edit", "time0": "1787000000", "user0": "b", "desc0": "z",
                "rev1": "30", "change1": "185576", "action1": "edit", "time1": "1780066068", "user1": "b", "desc1": "z",
                "how1,0": "branch into", "file1,0": "//C/Main/H.cpp", "srev1,0": "#none", "erev1,0": "#1",
                "rev2": "13", "change2": "71179", "action2": "integrate", "time2": "1730830715", "user2": "b", "desc2": "z",
                "how2,0": "copy from", "file2,0": "//U/MergeUnreal/H.cpp", "srev2,0": "#none", "erev2,0": "#1",
            })),
            rec(json!({
                "depotFile": "//U/MergeUnreal/H.cpp",
                "rev0": "1", "change0": "70000", "action0": "branch", "time0": "1730000000", "user0": "c", "desc0": "m",
            })),
            rec(json!({
                "depotFile": "//C/Weekly/H.cpp",
                "rev0": "1", "change0": "202860", "action0": "branch", "time0": "1788276393", "user0": "a", "desc0": "w",
                "how0,0": "branch from", "file0,0": "//C/Main/H.cpp", "srev0,0": "#none", "erev0,0": "#2",
            })),
            rec(json!({
                "depotFile": "//C/Merge/H.cpp",
                "rev0": "1", "change0": "203000", "action0": "branch", "time0": "1788300000", "user0": "d", "desc0": "q",
            })),
        ]
    }

    fn ancestry() -> BTreeSet<String> {
        ["//C/Main/H.cpp", "//U/SloClap/H.cpp", "//C/Weekly/H.cpp"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn records_become_edges_pointing_the_way_the_content_went() {
        let (_, revs, edges) = parse_record(&sample()[0]).unwrap();
        assert_eq!(revs.iter().map(|r| r.rev).collect::<Vec<_>>(), vec![1, 2]);
        let e = |f: &str, fr: u32, t: &str, tr: u32, how: &str| (f.to_string(), fr, t.to_string(), tr, how.to_string());
        let got: Vec<_> = edges.iter().map(|x| (x.from_file.clone(), x.from_rev, x.to_file.clone(), x.to_rev, x.how.clone())).collect();
        assert!(got.contains(&e("//C/Main/H.cpp", 2, "//C/Weekly/H.cpp", 1, "branch")));
        assert!(got.contains(&e("//U/SloClap/H.cpp", 36, "//C/Main/H.cpp", 2, "copy")));
        assert!(got.contains(&e("//U/SloClap/H.cpp", 30, "//C/Main/H.cpp", 1, "branch")));
        let copy = edges.iter().find(|x| x.how == "copy").unwrap();
        assert_eq!(copy.from_start, 30);
    }

    #[test]
    fn the_same_integration_seen_from_both_ends_is_one_edge() {
        let g = assemble("//C/Weekly/H.cpp", &ancestry(), &sample(), true);
        let main_to_weekly = g
            .edges
            .iter()
            .filter(|e| e.from_file == "//C/Main/H.cpp" && e.to_file == "//C/Weekly/H.cpp")
            .count();
        assert_eq!(main_to_weekly, 1);
        let sloclap_to_main_1 =
            g.edges.iter().filter(|e| e.from_file == "//U/SloClap/H.cpp" && e.to_rev == 1).count();
        assert_eq!(sloclap_to_main_1, 1);
    }

    #[test]
    fn a_source_is_shown_and_a_branch_into_another_stream_is_hidden_unless_asked() {
        let g = assemble("//C/Weekly/H.cpp", &ancestry(), &sample(), false);
        let files: Vec<&str> = g.lanes.iter().map(|l| l.file.as_str()).collect();
        assert!(files.contains(&"//U/MergeUnreal/H.cpp"), "the copy source stays");
        assert!(!files.contains(&"//C/Merge/H.cpp"), "the branch into Merge is hidden");
        assert_eq!(g.hidden_into, 1);
        assert!(g.edges.iter().all(|e| e.to_file != "//C/Merge/H.cpp"));
        let all = assemble("//C/Weekly/H.cpp", &ancestry(), &sample(), true);
        assert!(all.lanes.iter().any(|l| l.file == "//C/Merge/H.cpp"));
        assert_eq!(all.hidden_into, 0);
    }

    #[test]
    fn an_ancestor_listed_twice_keeps_every_revision_once() {
        let mut recs = sample();
        // Its own filelog, after `-i` stopped it at #30: #31..#36 too.
        recs.push(rec(json!({
            "depotFile": "//U/SloClap/H.cpp",
            "rev0": "36", "change0": "200000", "action0": "edit", "time0": "1787000000", "user0": "b", "desc0": "z",
            "rev1": "31", "change1": "186000", "action1": "edit", "time1": "1780100000", "user1": "b", "desc1": "z",
            "rev2": "30", "change2": "185576", "action2": "edit", "time2": "1780066068", "user2": "b", "desc2": "z",
        })));
        let g = assemble("//C/Weekly/H.cpp", &ancestry(), &recs, false);
        let lane = g.lanes.iter().find(|l| l.file == "//U/SloClap/H.cpp").unwrap();
        assert_eq!(lane.revs.iter().map(|r| r.rev).collect::<Vec<_>>(), vec![13, 30, 31, 36]);
    }

    #[test]
    fn lanes_run_oldest_first() {
        let g = assemble("//C/Weekly/H.cpp", &ancestry(), &sample(), false);
        let files: Vec<&str> = g.lanes.iter().map(|l| l.file.as_str()).collect();
        assert_eq!(files, vec!["//U/MergeUnreal/H.cpp", "//U/SloClap/H.cpp", "//C/Main/H.cpp", "//C/Weekly/H.cpp"]);
        assert!(g.lanes[0].related && !g.lanes[1].related);
    }
}
