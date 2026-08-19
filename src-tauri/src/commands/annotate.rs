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

        let lines: Vec<BlameLine> = recs
            .iter()
            // The header record has no `data`; every line record does.
            .filter(|r| r.contains_key("data"))
            .map(|r| {
                let get = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
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
            .collect();
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
