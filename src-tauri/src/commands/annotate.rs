//! Blame: which changelist put each line of a file there.
//!
//! `p4 annotate -u -c` answers it in one call. The tagged form gives a header
//! record, then one record per line — verified live:
//!
//! ```text
//! {"action":"edit","change":"197204","depotFile":"//…/NavigationModeComponent.cpp","rev":"10",…}
//! {"data":"// Copyright …\n","lower":"134476","upper":"197204","user":"julien.chevallay","time":"2025/09/24 12:59:19",…}
//! ```
//!
//! With `-c`, `lower` is the changelist that INTRODUCED the line (`upper` is the
//! one it survives through); without `-c` both are revisions instead. The line
//! text arrives in `data`, newline included.
//!
//! `-I` (follow integrations) is deliberately not used: it is the same walk that
//! makes `filelog` unusable on heavily branched depots — history.svelte.ts
//! already carries a per-server fallback for exactly that — and blame is far
//! more useful fast than exhaustive.

use crate::p4::{self, P4Conn};
use serde::Serialize;

/// One line of a file, with the change that introduced it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    /// The changelist that introduced this line.
    pub change: String,
    /// The file revision that changelist produced, when it can be resolved from
    /// filelog — empty otherwise, and then the line offers no diff.
    pub rev: String,
    pub user: String,
    pub date: String,
    pub text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blame {
    pub depot_file: String,
    /// The revision blamed, as p4 resolved it ("10", "" when unknown).
    pub rev: String,
    pub lines: Vec<BlameLine>,
}

/// Turn annotate's records into blame lines. Split out from the command so it
/// can be tested against captured output: which record is the header, and which
/// of `lower`/`upper` is the author's change, are exactly the things that would
/// otherwise be wrong on every line of the view — silently.
fn to_lines(
    recs: &[p4::Record],
    rev_of: &std::collections::HashMap<String, String>,
) -> Vec<BlameLine> {
    recs.iter()
        // The header record has no `data`; every line record does.
        .filter(|r| r.contains_key("data"))
        .map(|r| {
            let get = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            // `lower` is the change that INTRODUCED the line; `upper` is merely
            // the one it still survives in, which is the head for most lines.
            let change = get("lower");
            BlameLine {
                rev: rev_of.get(&change).cloned().unwrap_or_default(),
                change,
                user: get("user"),
                date: get("time"),
                // p4 keeps the newline on each line; the view adds its own.
                text: get("data").trim_end_matches(['\n', '\r']).to_string(),
            }
        })
        .collect()
}

/// Blame `file`, optionally at `rev_spec` ("#8", "@=1234", "" = head).
#[tauri::command]
pub async fn p4_annotate(
    conn: P4Conn,
    depot_file: String,
    rev_spec: String,
) -> Result<Blame, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spec = format!("{depot_file}{rev_spec}");
        let recs = p4::run(&conn, &["annotate", "-u", "-c", &spec])?;
        if recs.is_empty() {
            return Err(format!("no annotation for {spec}"));
        }
        // change -> the revision it produced, so a line can offer "diff this".
        // One extra call; annotate itself only ever names changelists.
        let mut rev_of: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Ok(log) = p4::run(&conn, &["filelog", "-m", "500", &depot_file]) {
            for r in &log {
                for row in p4::explode_indexed(r, "rev") {
                    let c = row.get("change").and_then(|v| v.as_str()).unwrap_or("");
                    let v = row.get("rev").and_then(|v| v.as_str()).unwrap_or("");
                    if !c.is_empty() && !v.is_empty() {
                        rev_of.entry(c.to_string()).or_insert_with(|| v.to_string());
                    }
                }
            }
        }

        let head = recs.first().filter(|r| r.contains_key("depotFile"));
        let file = head
            .and_then(|r| r.get("depotFile"))
            .and_then(|v| v.as_str())
            .unwrap_or(&depot_file)
            .to_string();
        let rev = head
            .and_then(|r| r.get("rev"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let lines = to_lines(&recs, &rev_of);
        if lines.is_empty() {
            return Err(format!(
                "{spec} has no text to annotate — binary files cannot be blamed"
            ));
        }
        Ok(Blame { depot_file: file, rev, lines })
    })
    .await
    .map_err(|e| format!("annotate task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::to_lines;
    use crate::p4::Record;
    use std::collections::HashMap;

    fn rec(pairs: &[(&str, &str)]) -> Record {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }

    /// A real `p4 -ztag -Mj annotate -u -c` answer: one header record, then one
    /// per line. Captured from NavigationModeComponent.cpp#10, where the first
    /// line is original and the other arrived in a later change.
    fn captured() -> Vec<Record> {
        vec![
            rec(&[
                ("action", "edit"),
                ("change", "197204"),
                ("depotFile", "//d/NavigationModeComponent.cpp"),
                ("rev", "10"),
                ("type", "text"),
            ]),
            rec(&[
                ("data", "// Copyright xxxx SloClap, Inc. All Rights Reserved.\n"),
                ("lower", "134476"),
                ("upper", "197204"),
                ("user", "julien.chevallay"),
                ("time", "2025/09/24 12:59:19"),
            ]),
            rec(&[
                ("data", "#include \"Characters/CYBaseAI.h\"\n"),
                ("lower", "142419"),
                ("upper", "197204"),
                ("user", "julien.chevallay"),
                ("time", "2025/11/07 09:12:03"),
            ]),
        ]
    }

    #[test]
    fn the_header_record_is_not_a_line() {
        let lines = to_lines(&captured(), &HashMap::new());
        assert_eq!(lines.len(), 2, "the header carries no data and is not blamed");
        assert!(lines[0].text.starts_with("// Copyright"));
    }

    #[test]
    fn the_change_is_lower_not_upper() {
        // The trap: `upper` is the head change on nearly every line, so reading
        // it would credit one person with authoring the entire file.
        let lines = to_lines(&captured(), &HashMap::new());
        assert_eq!(lines[0].change, "134476");
        assert_eq!(lines[1].change, "142419", "a later line keeps its own change");
        assert_ne!(lines[1].change, "197204", "never the head change");
    }

    #[test]
    fn the_newline_goes_but_the_text_survives() {
        let lines = to_lines(&captured(), &HashMap::new());
        assert!(!lines[0].text.ends_with('\n'));
        // Quotes and punctuation must survive intact — this is source code.
        assert_eq!(lines[1].text, "#include \"Characters/CYBaseAI.h\"");
    }

    #[test]
    fn revisions_come_from_the_filelog_map() {
        let mut rev_of = HashMap::new();
        rev_of.insert("134476".to_string(), "1".to_string());
        let lines = to_lines(&captured(), &rev_of);
        assert_eq!(lines[0].rev, "1", "mapped to the revision that change produced");
        assert_eq!(lines[1].rev, "", "an unmapped change offers no diff, never a wrong one");
    }
}
