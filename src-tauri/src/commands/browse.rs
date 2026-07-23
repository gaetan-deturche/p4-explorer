//! Read-only browsing: info, workspaces, depot tree, history, search.

use super::{run, v, Res};
use crate::p4::{self, P4Conn};

/// `p4 info` — server / client / user context.
#[tauri::command]
pub async fn p4_info(conn: P4Conn) -> Res {
    run(conn, v(&["info"])).await
}

/// Client workspaces, optionally filtered to `conn.user`.
#[tauri::command]
pub async fn p4_clients(conn: P4Conn) -> Res {
    let args = if conn.user.is_empty() {
        v(&["clients"])
    } else {
        v(&["clients", "-u", &conn.user])
    };
    run(conn, args).await
}

/// Sub-directories of a depot path (`p4 dirs <path>/*`).
#[tauri::command]
pub async fn p4_dirs(conn: P4Conn, path: String) -> Res {
    let pattern = format!("{}/*", path.trim_end_matches('/'));
    run(conn, v(&["dirs", &pattern])).await
}

/// Files directly under a depot path, with have/head status for the synced
/// marker (`p4 fstat <path>/*`).
#[tauri::command]
pub async fn p4_files(conn: P4Conn, path: String) -> Res {
    let pattern = format!("{}/*", path.trim_end_matches('/'));
    run(
        conn,
        v(&[
            "fstat",
            "-T",
            "depotFile,headRev,haveRev,headAction,headChange,headType,headTime",
            &pattern,
        ]),
    )
    .await
}

/// Submitted changelists affecting a depot path (newest first). `before`, when
/// set, pages backward: only changelists <= that number are returned, so the
/// caller can fetch history in chunks (`before = smallest_seen - 1`).
#[tauri::command]
pub async fn p4_changes(conn: P4Conn, path: String, max: u32, before: Option<u32>) -> Res {
    let max = max.to_string();
    let base = if path.ends_with("...") || path.contains('*') {
        path
    } else {
        format!("{}/...", path.trim_end_matches('/'))
    };
    let spec = match before {
        Some(b) => format!("{base}@{b}"),
        None => base,
    };
    run(conn, v(&["changes", "-l", "-m", &max, "-s", "submitted", &spec])).await
}

/// The changelist a depot path is currently synced to (highest CL among the
/// have revisions). One record, or empty if nothing is synced.
#[tauri::command]
pub async fn p4_have_change(conn: P4Conn, path: String) -> Res {
    let spec = format!("{}/...#have", path.trim_end_matches('/'));
    run(conn, v(&["changes", "-m", "1", &spec])).await
}

/// Full description of a changelist, exploded to one row per affected file.
#[tauri::command]
pub async fn p4_describe(conn: P4Conn, change: String) -> Res {
    let recs = run(conn, v(&["describe", "-s", &change])).await?;
    let mut out = Vec::new();
    for rec in &recs {
        let rows = p4::explode_indexed(rec, "depotFile");
        if rows.is_empty() {
            out.push(rec.clone()); // empty changelist: keep the header row
        } else {
            out.extend(rows);
        }
    }
    Ok(out)
}

/// Revision history of a single file, one row per revision (newest first).
#[tauri::command]
pub async fn p4_filelog(conn: P4Conn, file: String, max: u32) -> Res {
    let max = max.to_string();
    let recs = run(conn, v(&["filelog", "-l", "-m", &max, &file])).await?;
    let mut out = Vec::new();
    for rec in &recs {
        out.extend(p4::explode_indexed(rec, "rev"));
    }
    Ok(out)
}

/// fstat for a single file (have/head revisions).
#[tauri::command]
pub async fn p4_fstat(conn: P4Conn, file: String) -> Res {
    run(conn, v(&["fstat", &file])).await
}

/// Depot-wide filename search under `root`: `p4 files //root/.../*term*`.
/// Case-sensitive (the server's case handling). Capped at `max` results.
#[tauri::command]
pub async fn p4_search(conn: P4Conn, root: String, term: String, max: u32) -> Res {
    let pattern = format!("{}/.../*{}*", root.trim_end_matches('/'), term);
    run(conn, v(&["files", "-m", &max.to_string(), &pattern])).await
}

/// All depots on the server (`p4 depots`).
#[tauri::command]
pub async fn p4_depots(conn: P4Conn) -> Res {
    run(conn, v(&["depots"])).await
}

/// A local-filesystem directory listing (names only).
#[derive(serde::Serialize)]
pub struct LocalDir {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
}

/// List a local directory via the OS filesystem. Used as an instant provisional
/// view of a workspace folder while the (cold, slow) `p4 dirs`/`fstat` scan runs
/// — the client root maps depot paths to local files, so this shows the synced
/// contents immediately, then gets replaced by the authoritative depot listing.
#[tauri::command]
pub async fn list_local_dir(path: String) -> Result<LocalDir, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => dirs.push(name),
                Ok(_) => files.push(name),
                Err(_) => {}
            }
        }
        dirs.sort_unstable();
        files.sort_unstable();
        Ok(LocalDir { dirs, files })
    })
    .await
    .map_err(|e| format!("list_local_dir task failed: {e}"))?
}
