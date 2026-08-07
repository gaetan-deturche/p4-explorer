//! Undoing a SUBMITTED change: the whole changelist, or one file out of it.
//!
//! `p4 undo <file>@=<change>` opens the file at the revision before that change
//! — it does not submit anything, so the undo lands as an ordinary pending
//! changelist the user reviews and submits (or trims first, or discards).
//!
//! Three things about the raw command make it unusable on its own, all verified
//! against P4D 2025.1:
//!
//! * the workspace file it leaves behind is READ-ONLY, because the file is
//!   opened for `integrate`, not for edit;
//! * that integrate carries a resolve digest, so editing the file anyway and
//!   submitting fails with "tampered with after resolve - edit or revert";
//! * a file it REFUSES is a warning with exit status 0 (`undo -n //nope@=1`
//!   prints "no such file(s)." and succeeds), so per-file success has to be read
//!   from the records, never from the exit code — hence `p4::run_notes`.
//!
//! The first two disappear by reopening each file for edit right after the undo,
//! and `p4 resolved` still reports "undid //file#rev" afterwards, so the undo
//! keeps its integration record. Since the whole point is to be able to KEEP
//! part of the change (the DefaultEngine.ini case: undo the file, then put back
//! the hunks that were fine), that reopen is unconditional.

use super::Res;
use crate::p4::{self, P4Conn};
use serde::Serialize;

/// What happened to one file of an undo.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UndoFile {
    pub depot_file: String,
    pub ok: bool,
    /// p4's own words — why it refused, or what it noted while accepting.
    pub message: String,
}

/// The outcome of an undo: where the files landed, and what each one did.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    /// The new pending changelist holding the undone files.
    pub change: String,
    pub files: Vec<UndoFile>,
    pub undone: u32,
    pub failed: u32,
    /// Set when at least one file still needs a `p4 resolve` before it can be
    /// submitted — the file moved on after the change being undone.
    pub needs_resolve: bool,
}

/// The files a submitted change touched, in depot order.
fn files_of(conn: &P4Conn, change: &str) -> Result<Vec<String>, String> {
    let recs = p4::run(conn, &["describe", "-s", change])?;
    let rec = recs.first().ok_or(format!("@{change} is not a changelist"))?;
    let files: Vec<String> = p4::explode_indexed(rec, "depotFile")
        .iter()
        .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(String::from))
        .collect();
    if files.is_empty() {
        return Err(format!("@{change} has no submitted files"));
    }
    Ok(files)
}

/// First line of a submitted change's description, for naming the undo.
fn subject(conn: &P4Conn, change: &str) -> String {
    p4::run(conn, &["describe", "-s", "-m", "0", change])
        .ok()
        .and_then(|r| {
            r.first().and_then(|r| r.get("desc")).and_then(|v| v.as_str()).map(String::from)
        })
        .unwrap_or_default()
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Run one `p4` file operation and say whether THAT FILE was accepted.
///
/// A record back means p4 acted on it; no record means it refused, and the
/// reason is in the notes (a warning, which is why `run_notes` and not `run`).
fn one(conn: &P4Conn, args: &[&str]) -> (bool, String) {
    match p4::run_notes(conn, args) {
        Ok((recs, notes)) if !recs.is_empty() => (true, notes.join("; ")),
        Ok((_, notes)) if !notes.is_empty() => (false, notes.join("; ")),
        Ok(_) => (false, "p4 did not act on this file".into()),
        Err(e) => (false, e),
    }
}

/// True for p4's "you must resolve before you can submit this" family.
fn blocked_on_resolve(msg: &str) -> bool {
    msg.contains("must resolve") || msg.contains("must sync")
}

/// Preview an undo without touching anything (`p4 undo -n`): one row per file,
/// `ok` false for the ones p4 would refuse, with its reason. The confirm dialog
/// is built from this, so the user agrees to what will actually happen rather
/// than to an intention.
#[tauri::command]
pub async fn p4_undo_preview(conn: P4Conn, change: String, files: Vec<String>) -> Res {
    tauri::async_runtime::spawn_blocking(move || {
        let files = if files.is_empty() { files_of(&conn, &change)? } else { files };
        let mut out = Vec::new();
        for f in files {
            let spec = format!("{f}@={change}");
            let (ok, msg) = one(&conn, &["undo", "-n", &spec]);
            let mut rec = p4::Record::new();
            rec.insert("depotFile".into(), f.into());
            rec.insert("ok".into(), ok.into());
            rec.insert("message".into(), msg.into());
            out.push(rec);
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("undo preview failed: {e}"))?
}

/// Undo a submitted change — every file of it, or just `files` — into a fresh
/// pending changelist. Depot state is untouched until the user submits that
/// changelist.
///
/// A file p4 refuses (already open elsewhere, outside this client's view, no
/// such revision…) is reported and skipped; the rest still land, because a
/// partial undo the user can see beats an all-or-nothing failure they cannot act
/// on. When nothing lands, the empty changelist is deleted again.
#[tauri::command]
pub async fn p4_undo_change(
    conn: P4Conn,
    change: String,
    files: Vec<String>,
) -> Result<UndoResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let files = if files.is_empty() { files_of(&conn, &change)? } else { files };
        let subject = subject(&conn, &change);
        let one_name = if files.len() == 1 {
            format!(" ({})", files[0].rsplit('/').next().unwrap_or_default())
        } else {
            String::new()
        };
        let desc = if subject.is_empty() {
            format!("Undo of @{change}{one_name}")
        } else {
            format!("Undo of @{change}{one_name}: {subject}")
        };
        let new_change = new_changelist(&conn, &desc)?;

        let mut out: Vec<UndoFile> = Vec::new();
        let mut needs_resolve = false;
        for f in &files {
            let spec = format!("{f}@={change}");
            let (ok, msg) = one(&conn, &["undo", "-c", &new_change, &spec]);
            if !ok {
                out.push(UndoFile { depot_file: f.clone(), ok: false, message: msg });
                continue;
            }
            // Until this reopen the file is read-only and digest-guarded; see
            // the module comment. It is already ours here, so a refusal would be
            // a real surprise — report it rather than leave an untouchable file.
            let (eok, emsg) = one(&conn, &["edit", "-c", &new_change, f]);
            needs_resolve |= blocked_on_resolve(&msg) || blocked_on_resolve(&emsg);
            out.push(UndoFile {
                depot_file: f.clone(),
                ok: eok,
                message: if eok { msg } else { emsg },
            });
        }

        let undone = out.iter().filter(|f| f.ok).count() as u32;
        let failed = out.len() as u32 - undone;
        if undone == 0 {
            // Nothing was opened, so the changelist is empty — don't leave a
            // stray CL behind for the user to clean up after a failure.
            let _ = p4::run_raw(&conn, &["change", "-d", &new_change]);
            let why = out.first().map(|f| f.message.clone()).unwrap_or_default();
            return Err(if why.is_empty() {
                format!("nothing of @{change} could be undone")
            } else {
                why
            });
        }
        Ok(UndoResult { change: new_change, files: out, undone, failed, needs_resolve })
    })
    .await
    .map_err(|e| format!("undo task failed: {e}"))?
}

/// `p4 change -i` with a description and no files — the same form-stripping as
/// `p4_new_changelist`, callable from inside a blocking task.
fn new_changelist(conn: &P4Conn, description: &str) -> Result<String, String> {
    let field = format!("Description={description}");
    let form = p4::run_raw(conn, &["--field", &field, "change", "-o"])?;
    // Keep currently-open files out of the new changelist.
    let form = match form.find("\nFiles:") {
        Some(i) => format!("{}\n", &form[..i]),
        None => form,
    };
    let out = p4::run_raw_stdin(conn, &["change", "-i"], &form)?;
    out.split_whitespace()
        .skip_while(|w| *w != "Change")
        .nth(1)
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .ok_or_else(|| format!("could not parse new change number from: {}", out.trim()))
}

#[cfg(test)]
mod tests {
    use super::blocked_on_resolve;

    #[test]
    fn resolve_blockers_are_recognised() {
        // Real p4 lines from an undo whose file moved on since the change.
        assert!(blocked_on_resolve(
            "//d/f.cpp - must resolve #11 before submitting"
        ));
        assert!(blocked_on_resolve("//d/f.cpp - must sync/resolve #10 before submitting"));
        assert!(!blocked_on_resolve("//d/f.cpp#10 - opened for integrate"));
        assert!(!blocked_on_resolve(""));
    }
}
