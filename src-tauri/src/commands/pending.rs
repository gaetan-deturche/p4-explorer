//! Pending changelists and their file actions: list, shelved files, submit,
//! shelve, review, rename (description), revert, reopen, new changelist.

use super::{run, v, Res};
use crate::p4::{self, P4Conn};

/// Pending changelists for the connection's client.
#[tauri::command]
pub async fn p4_pending(conn: P4Conn, max: u32) -> Res {
    // Pending is workspace-scoped. Without a client, don't fall back to a
    // user-wide `-u` listing — that showed every workspace's pending CLs even
    // when no workspace was selected.
    if conn.client.is_empty() {
        return Ok(Vec::new());
    }
    let max = max.to_string();
    let mut args = v(&["changes", "-l", "-m", &max, "-s", "pending", "-c"]);
    args.push(conn.client.clone());
    run(conn, args).await
}

/// Files open (in the workspace) for a pending changelist (`opened -c`).
#[tauri::command]
pub async fn p4_opened(conn: P4Conn, change: String) -> Res {
    // Empty change = every opened file of the client (one cheap command; the
    // change-detection poll fingerprints it).
    if change.is_empty() {
        run(conn, v(&["opened"])).await
    } else {
        run(conn, v(&["opened", "-c", &change])).await
    }
}

/// Shelved files of a pending changelist (`describe -S -s`), one row per file.
#[tauri::command]
pub async fn p4_describe_shelved(conn: P4Conn, change: String) -> Res {
    let recs = run(conn, v(&["describe", "-S", "-s", &change])).await?;
    let mut out = Vec::new();
    for rec in &recs {
        let rows = p4::explode_indexed(rec, "depotFile");
        if rows.is_empty() {
            out.push(rec.clone());
        } else {
            out.extend(rows);
        }
    }
    Ok(out)
}

/// Submit a pending changelist (`p4 submit -c <change>`, or the default CL).
/// Depot-modifying — call only from an explicit, confirmed user action. Uses the
/// strict runner so a blocked submit (e.g. shelved files still in the CL)
/// surfaces its error instead of being masked by submit's progress records.
#[tauri::command]
pub async fn p4_submit(conn: P4Conn, change: String) -> Res {
    tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<&str> = if change == "default" {
            vec!["submit"]
        } else {
            vec!["submit", "-c", &change]
        };
        p4::run_strict(&conn, &args)
    })
    .await
    .map_err(|e| format!("submit task failed: {e}"))?
}

/// The client's pending changelists that HAVE shelved files
/// (`p4 changes -s shelved -c <client>`, ~0.1s).
///
/// Without this the app cannot tell an empty changelist from a shelved one
/// without describing each of them, so it offered "Delete shelf" on changelists
/// that had no shelf.
#[tauri::command]
pub async fn p4_shelved_changes(conn: P4Conn) -> Res {
    if conn.client.is_empty() {
        return Ok(Vec::new());
    }
    let client = conn.client.clone();
    let mut args = v(&["changes", "-s", "shelved", "-c"]);
    args.push(client);
    run(conn, args).await
}

/// Revert every file open in a changelist (`p4 revert -c <change> //...`),
/// discarding their local edits. The changelist itself stays — an empty
/// changelist is deleted separately, so a revert never silently takes the
/// description with it.
#[tauri::command]
pub async fn p4_revert_change(conn: P4Conn, change: String) -> Res {
    run(conn, v(&["revert", "-c", &change, "//..."])).await
}

/// Delete an EMPTY pending changelist (`p4 change -d <change>`). p4 refuses
/// while it still holds opened or shelved files, and says which — that refusal
/// is the guard, so nothing is reverted or unshelved on the user's behalf here.
///
/// The refusal cannot be read from the command's own result: p4 reports it as an
/// ordinary info line with NO severity and exit status 0 — verified,
/// `{"data":"Change N has 1 open file(s) ... can't be deleted.","level":0}`
/// against the same shape as success, `{"data":"Change N deleted.","level":0}`.
/// Every runner therefore takes it for a data record and reports success, which
/// is how a refused delete produced a "Changelist @N deleted." notification. So
/// the outcome is read from the STATE afterwards, not from the message.
#[tauri::command]
pub async fn p4_delete_change(conn: P4Conn, change: String) -> Res {
    tauri::async_runtime::spawn_blocking(move || {
        let out = p4::run(&conn, &["change", "-d", &change])?;
        let status = p4::run(&conn, &["change", "-o", &change])
            .ok()
            .and_then(|r| {
                r.first()
                    .and_then(|r| r.get("Status"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
        if still_exists(status.as_deref()) {
            let why: String = out
                .iter()
                .filter_map(|r| r.get("data").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join(" ");
            return Err(if why.trim().is_empty() {
                format!("changelist @{change} could not be deleted")
            } else {
                why.trim().to_string()
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("delete-changelist task failed: {e}"))?
}

/// Does `change -o` still describe a real changelist? A deleted (or never-used)
/// number answers "Change N unknown." — no record, hence no status — while a
/// surviving one reports `pending` or `shelved`.
fn still_exists(status: Option<&str>) -> bool {
    matches!(status, Some(s) if s != "new")
}

/// Delete the shelved files of a changelist (`p4 shelve -d -c <change>`).
#[tauri::command]
pub async fn p4_shelve_delete(conn: P4Conn, change: String) -> Res {
    run(conn, v(&["shelve", "-d", "-c", &change])).await
}

/// Restore a changelist's shelved files into the workspace
/// (`p4 unshelve -s <change> -c <change>`), leaving the shelf in place — the
/// server copy stays as a safety net until "Delete shelf" removes it.
///
/// The result is reported rather than assumed. p4 permits unshelving over an
/// already-open file only when both sides are `edit`, and then flags the file
/// UNRESOLVED (`p4 resolve` required); unshelving an `add` over an open file is
/// not supported at all. So the notice has to be able to say "restored, and some
/// of it needs a resolve" instead of a flat success.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnshelveResult {
    pub restored: u32,
    pub needs_resolve: bool,
    /// p4's own remarks (warnings included) — shown when there is something to say.
    pub notes: Vec<String>,
}

#[tauri::command]
pub async fn p4_unshelve(conn: P4Conn, change: String) -> Result<UnshelveResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // run_notes, not run: a per-file refusal is a WARNING with exit status 0
        // (see commands/undo.rs), so `run` would drop the reason and report a
        // success that never happened.
        let (recs, notes) = p4::run_notes(&conn, &["unshelve", "-s", &change, "-c", &change])?;
        if recs.is_empty() {
            return Err(if notes.is_empty() {
                format!("nothing was unshelved from @{change}")
            } else {
                notes.join("; ")
            });
        }
        // Ask the STATE, never the message. p4 reports an unshelve as
        // "...#10 - unshelved, opened for edit" whether or not it left a merge to
        // settle, and it says nothing about a resolve — verified live. `resolve -n
        // -c <change>` is exact and scoped to this changelist: files to resolve
        // come back as records, and "No file(s) to resolve." is a severity-2
        // warning that `run` drops, so an empty result means genuinely nothing.
        let needs_resolve = p4::run(&conn, &["resolve", "-n", "-c", &change])
            .map(|r| !r.is_empty())
            .unwrap_or(false);
        Ok(UnshelveResult { restored: recs.len() as u32, needs_resolve, notes })
    })
    .await
    .map_err(|e| format!("unshelve task failed: {e}"))?
}

/// (Re)shelve a changelist's files (`p4 shelve -f -c <change>`) — used to update
/// an existing Swarm review (Swarm picks up the new shelf).
#[tauri::command]
pub async fn p4_shelve(conn: P4Conn, change: String) -> Res {
    run(conn, v(&["shelve", "-f", "-c", &change])).await
}

/// Request a Swarm review: ensure `#review` is in the changelist description,
/// then shelve. Swarm's trigger creates the review from the shelf.
#[tauri::command]
pub async fn p4_request_review(conn: P4Conn, change: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let recs = p4::run(&conn, &["change", "-o", &change])?;
        let desc = recs
            .first()
            .and_then(|r| r.get("Description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !desc.contains("#review") {
            let newdesc = format!("{}\n#review", desc.trim_end());
            // Emit the form with the new Description, then feed it to `change -i`.
            let field = format!("Description={newdesc}");
            let form = p4::run_raw(&conn, &["--field", &field, "change", "-o", &change])?;
            p4::run_raw_stdin(&conn, &["change", "-i"], &form)?;
        }
        p4::run_raw(&conn, &["shelve", "-f", "-c", &change])?;
        Ok(())
    })
    .await
    .map_err(|e| format!("review task failed: {e}"))?
}

/// Set a numbered pending changelist's description (`change -o | change -i`).
/// This is the closest thing to "renaming" a changelist in Perforce.
#[tauri::command]
pub async fn p4_set_description(
    conn: P4Conn,
    change: String,
    description: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let field = format!("Description={description}");
        let form = p4::run_raw(&conn, &["--field", &field, "change", "-o", &change])?;
        p4::run_raw_stdin(&conn, &["change", "-i"], &form)?;
        Ok(())
    })
    .await
    .map_err(|e| format!("rename task failed: {e}"))?
}

/// Revert an opened file, discarding local changes (`p4 revert <file>`).
#[tauri::command]
pub async fn p4_revert(conn: P4Conn, depot_file: String) -> Res {
    run(conn, v(&["revert", &depot_file])).await
}

/// Un-open a file while keeping the workspace content (`p4 revert -k <file>`):
/// drops it from its changelist but leaves your local edits on disk.
#[tauri::command]
pub async fn p4_revert_keep(conn: P4Conn, depot_file: String) -> Res {
    run(conn, v(&["revert", "-k", &depot_file])).await
}

/// Move an opened file to another pending changelist (`p4 reopen -c <change>`);
/// `change` may be "default".
#[tauri::command]
pub async fn p4_reopen(conn: P4Conn, depot_file: String, change: String) -> Res {
    run(conn, v(&["reopen", "-c", &change, &depot_file])).await
}

/// Create a new empty pending changelist with `description`; returns its number.
/// The `change -o` form's Files section is stripped so currently-open files are
/// NOT swept into the new changelist.
#[tauri::command]
pub async fn p4_new_changelist(conn: P4Conn, description: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let field = format!("Description={description}");
        let form = p4::run_raw(&conn, &["--field", &field, "change", "-o"])?;
        let form = match form.find("\nFiles:") {
            Some(i) => format!("{}\n", &form[..i]),
            None => form,
        };
        let out = p4::run_raw_stdin(&conn, &["change", "-i"], &form)?;
        // Output looks like: "Change 12345 created."
        out.split_whitespace()
            .skip_while(|w| *w != "Change")
            .nth(1)
            .map(|s| s.trim_end_matches('.').to_string())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| format!("could not parse new change number from: {}", out.trim()))
    })
    .await
    .map_err(|e| format!("new-changelist task failed: {e}"))?
}

#[cfg(test)]
mod delete_tests {
    use super::still_exists;

    #[test]
    fn a_deleted_changelist_reports_no_status() {
        // "Change N unknown." yields no record at all.
        assert!(!still_exists(None));
        // A number p4 offers as a fresh form is not an existing changelist.
        assert!(!still_exists(Some("new")));
    }

    #[test]
    fn a_surviving_changelist_is_detected() {
        assert!(still_exists(Some("pending")));
        assert!(still_exists(Some("shelved")));
    }
}

/// Who has a file open, and who — if anyone — holds it exclusively.
///
/// Perforce answers this in two different ways and the difference matters:
///
/// * an explicit `p4 lock` shows up as `otherLock` (someone else) or `ourLock`;
/// * a file whose type carries `+l` is exclusive-open, so ANY open on it locks
///   everyone else out — and fstat reports NO lock field for that case, only
///   `otherOpen0`. Verified on a live `binary+l` asset held by another user:
///   `otherOpen0`/`otherAction0`/`otherChange0` came back, `otherLock` did not.
///
/// So "who owns the lock" is `otherLock`, else the holder of an open on a `+l`
/// file. Everything else is a plain shared checkout, which blocks nobody.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    /// User name alone, and the workspace they hold it in.
    pub user: String,
    pub client: String,
    pub action: String,
    pub change: String,
    /// This holder blocks everyone else: an explicit lock, or an open on a `+l`
    /// file.
    pub blocking: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHolders {
    pub depot_file: String,
    pub head_type: String,
    /// The type carries `+l`: one open at a time, whoever gets there first.
    pub exclusive_type: bool,
    /// We have it open — the reason nobody ELSE can, on an exclusive type.
    pub our_action: String,
    /// We hold an explicit `p4 lock`.
    pub our_lock: bool,
    /// Someone else holds an explicit `p4 lock`.
    pub other_lock: bool,
    /// Everyone else holding it open, in p4's order.
    pub others: Vec<Holder>,
}

#[tauri::command]
pub async fn p4_file_holders(conn: P4Conn, depot_file: String) -> Result<FileHolders, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // No -T: the other-user fields are indexed (otherOpen0, otherOpen1, …) and
        // their count is not known in advance, so ask for the whole record.
        let recs = p4::run(&conn, &["fstat", &depot_file])?;
        let rec = recs.first().ok_or_else(|| format!("{depot_file} is not in the depot"))?;
        let get = |k: &str| rec.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();

        let head_type = get("headType");
        // A file type's modifiers follow the base type after '+': binary+l, text+lw.
        let exclusive_type = head_type
            .split_once('+')
            .map(|(_, m)| m.contains('l'))
            .unwrap_or(false);
        let other_lock = rec.contains_key("otherLock") || rec.contains_key("otherLock0");
        let our_lock = rec.contains_key("ourLock");

        let others: Vec<Holder> = p4::explode_indexed(rec, "otherOpen")
            .iter()
            .map(|r| {
                let who = r.get("otherOpen").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let (user, client) = match who.split_once('@') {
                    Some((u, c)) => (u.to_string(), c.to_string()),
                    None => (who.clone(), String::new()),
                };
                Holder {
                    user,
                    client,
                    action: r.get("otherAction").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    change: r.get("otherChange").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    blocking: exclusive_type || other_lock,
                }
            })
            .collect();

        Ok(FileHolders {
            depot_file: get("depotFile"),
            head_type,
            exclusive_type,
            our_action: get("action"),
            our_lock,
            other_lock,
            others,
        })
    })
    .await
    .map_err(|e| format!("file-holders task failed: {e}"))?
}
