//! Opening files: check out, mark for add, mark for delete, and move/rename.
//!
//! Until now the app could only ever NOTICE work already done on disk
//! (`reconcile`, the offline scan). These are the four verbs that claim a file
//! before you touch it — the ones that matter most on an exclusive-open (`+l`)
//! depot, where checking out IS how you reserve an asset.
//!
//! Every one of them has the same reporting problem, verified live against P4D
//! 2025.1 with a `binary+l` asset another user held:
//!
//! ```text
//! $ p4 -ztag -Mj edit -n //…/AM_LazerPlant_Combat_PrepShoot.uasset   # exit 0
//! {"data":"… - can't edit exclusive file already opened","severity":2}
//! {"data":"… - also opened by thomas.bardet@thomas.bardet_SLOCLAP-167_…","level":1}
//! ```
//!
//! The exit status is 0 for the refusal, the first line is a WARNING (which
//! `p4::run` drops), and the second — the one naming who is in the way — carries
//! no severity at all, so it parses as a data RECORD. Success therefore cannot
//! mean "records came back": it means a record describing an actual open, i.e.
//! one carrying `action`. Everything else is explanation, and it is worth
//! keeping, because "also opened by <user>" is exactly what the user needs.

use crate::p4::{self, P4Conn};
use serde::Serialize;

/// What happened to one file.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    pub file: String,
    pub ok: bool,
    /// The action p4 opened it for, or why it refused — p4's own words,
    /// including the "also opened by …" line that names the blocker.
    pub message: String,
}

/// Decide what p4's answer means for one file. Split out from `open_one` so it
/// can be tested against captured output — it is pure record-reading, and the
/// records are the part that is easy to misread.
fn classify(recs: &[p4::Record], notes: Vec<String>, file: &str) -> OpenResult {
    // A real open reports an action; everything else is commentary.
    if let Some(r) = recs.iter().find(|r| r.contains_key("action")) {
        return OpenResult {
            file: file.to_string(),
            ok: true,
            message: r.get("action").and_then(|v| v.as_str()).unwrap_or("opened").to_string(),
        };
    }
    // Messages that arrived as records because they carry no severity — the
    // "also opened by <user>" line is one, and it is the useful one.
    let asides = recs
        .iter()
        .filter_map(|r| r.get("data").and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string());
    let mut why: Vec<String> = notes;
    why.extend(asides);
    OpenResult {
        file: file.to_string(),
        ok: false,
        message: if why.is_empty() {
            "p4 did not open this file".to_string()
        } else {
            why.join(" — ")
        },
    }
}

/// Run one file operation and report whether THAT FILE was opened.
fn open_one(conn: &P4Conn, args: &[&str], file: &str) -> OpenResult {
    match p4::run_notes(conn, args) {
        Ok((recs, notes)) => classify(&recs, notes, file),
        Err(e) => OpenResult { file: file.to_string(), ok: false, message: e },
    }
}

/// `p4 edit` / `p4 add` / `p4 delete` on each file, into `change` (empty = the
/// default changelist).
///
/// One p4 call per file, deliberately: a batched call reports refusals as loose
/// text that would have to be matched back to paths, and a refusal here is the
/// interesting case, not the exception.
#[tauri::command]
pub async fn p4_open_files(
    conn: P4Conn,
    verb: String,
    files: Vec<String>,
    change: String,
) -> Result<Vec<OpenResult>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let verb = match verb.as_str() {
            "edit" | "add" | "delete" => verb,
            other => return Err(format!("unknown file action {other:?}")),
        };
        let mut out = Vec::new();
        for f in &files {
            let mut args: Vec<&str> = vec![&verb];
            if !change.is_empty() {
                args.push("-c");
                args.push(&change);
            }
            args.push(f);
            out.push(open_one(&conn, &args, f));
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("file-action task failed: {e}"))?
}

/// Rename or move a file, keeping its history (`p4 move`).
///
/// `p4 move` requires the source to be open for edit or add first — "The
/// fromFile must be a file open for add or edit unless invoked with '-r'" — so
/// this opens it, then moves. Reconcile cannot express a move at all: it sees a
/// delete plus an add, which loses the link and stops the history at the new
/// name.
#[tauri::command]
pub async fn p4_move_file(
    conn: P4Conn,
    from: String,
    to: String,
    change: String,
) -> Result<OpenResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if to.trim().is_empty() || to == from {
            return Err("give the file a different path to move it to".into());
        }
        // Already open? Then keep that open as it is; p4 edit on an opened file
        // is harmless but would move it into `change`.
        let opened = p4::run(&conn, &["opened", &from])
            .map(|r| r.iter().any(|x| x.contains_key("depotFile")))
            .unwrap_or(false);
        if !opened {
            let mut args: Vec<&str> = vec!["edit"];
            if !change.is_empty() {
                args.push("-c");
                args.push(&change);
            }
            args.push(&from);
            let r = open_one(&conn, &args, &from);
            if !r.ok {
                return Ok(r); // can't open it → can't move it; say why
            }
        }
        let mut args: Vec<&str> = vec!["move"];
        if !change.is_empty() {
            args.push("-c");
            args.push(&change);
        }
        args.push(&from);
        args.push(&to);
        Ok(open_one(&conn, &args, &from))
    })
    .await
    .map_err(|e| format!("move task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::classify;
    use crate::p4::Record;

    fn rec(pairs: &[(&str, &str)]) -> Record {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn an_action_record_is_a_real_open() {
        // `p4 -ztag -Mj edit -n <free file>`, captured live.
        let recs = [rec(&[
            ("action", "edit"),
            ("clientFile", "H:/ws/NavigationModeComponent.cpp"),
            ("depotFile", "//d/NavigationModeComponent.cpp"),
            ("type", "text+D"),
            ("workRev", "10"),
        ])];
        let r = classify(&recs, vec![], "//d/NavigationModeComponent.cpp");
        assert!(r.ok);
        assert_eq!(r.message, "edit");
    }

    #[test]
    fn an_exclusive_refusal_names_the_holder() {
        // The trap this test exists for. `p4 edit` on a binary+l file another
        // user holds EXITS 0 and answers with two messages: a severity-2 warning
        // (which run_notes hands over as a note) and a `level:1` line with NO
        // severity — so that one arrives as a data RECORD. Reading "records came
        // back" as success would call this refusal an open.
        let recs = [rec(&[(
            "data",
            "//d/AM_LazerPlant.uasset - also opened by thomas.bardet@thomas.bardet_SLOCLAP-167",
        )])];
        let notes = vec!["//d/AM_LazerPlant.uasset - can't edit exclusive file already opened".to_string()];
        let r = classify(&recs, notes, "//d/AM_LazerPlant.uasset");
        assert!(!r.ok, "a data-only record is not an open");
        assert!(r.message.contains("can't edit exclusive file"), "keeps p4's reason");
        assert!(r.message.contains("thomas.bardet"), "and names who is in the way");
    }

    #[test]
    fn add_on_an_existing_file_is_a_refusal() {
        // `p4 add -n` on a file already in the depot: a level-0 data record, no
        // severity, no action — same shape as success apart from the missing
        // action, and no warning to fall back on.
        let recs = [rec(&[("data", "//d/f.cpp - can't add existing file")])];
        let r = classify(&recs, vec![], "//d/f.cpp");
        assert!(!r.ok);
        assert_eq!(r.message, "//d/f.cpp - can't add existing file");
    }

    #[test]
    fn silence_still_reports_a_failure() {
        let r = classify(&[], vec![], "//d/f.cpp");
        assert!(!r.ok);
        assert!(!r.message.is_empty(), "never a blank reason");
    }
}
