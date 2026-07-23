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
    run(conn, v(&["opened", "-c", &change])).await
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
/// Depot-modifying — call only from an explicit, confirmed user action.
#[tauri::command]
pub async fn p4_submit(conn: P4Conn, change: String) -> Res {
    let args = if change == "default" {
        v(&["submit"])
    } else {
        v(&["submit", "-c", &change])
    };
    run(conn, args).await
}

/// Delete the shelved files of a changelist (`p4 shelve -d -c <change>`).
#[tauri::command]
pub async fn p4_shelve_delete(conn: P4Conn, change: String) -> Res {
    run(conn, v(&["shelve", "-d", "-c", &change])).await
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
