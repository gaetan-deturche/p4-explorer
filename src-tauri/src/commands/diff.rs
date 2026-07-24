//! Diffs: in-app unified diffs (changelist, shelved, local-vs-server) and
//! launching the configured external P4DIFF tool.

use crate::p4::{self, P4Conn};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// Generate a unified-diff `.patch` from `files` (or all opened files of
/// `change` when `files` is empty), prompt a Save-As dialog, and write it.
/// Returns the saved path, or None if the user cancelled.
#[tauri::command]
pub async fn export_patch(
    app: AppHandle,
    conn: P4Conn,
    change: String,
    files: Vec<String>,
    default_name: String,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // The given selection, else all opened files of the changelist.
        let targets: Vec<String> = if !files.is_empty() {
            files
        } else if !change.is_empty() {
            p4::run(&conn, &["opened", "-c", &change])
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(String::from))
                .collect()
        } else {
            Vec::new()
        };
        if targets.is_empty() {
            return Err("No modified files to include in the patch.".into());
        }
        let mut args: Vec<&str> = vec!["diff", "-du"];
        for t in &targets {
            args.push(t.as_str());
        }
        let patch = p4::run_raw_stdout_diff(&conn, &args)?;
        if patch.trim().is_empty() {
            return Err("No textual diff to export (files may be adds or binaries).".into());
        }
        let picked = app
            .dialog()
            .file()
            .set_file_name(&default_name)
            .add_filter("Patch", &["patch"])
            .blocking_save_file();
        match picked {
            Some(fp) => {
                let path = fp.into_path().map_err(|e| e.to_string())?;
                std::fs::write(&path, patch).map_err(|e| e.to_string())?;
                Ok(Some(path.display().to_string()))
            }
            None => Ok(None),
        }
    })
    .await
    .map_err(|e| format!("export-patch task failed: {e}"))?
}

/// Unified diff of an opened file: local workspace vs the depot (have) — the
/// "local vs server" diff. P4DIFF cleared so it prints instead of launching.
#[tauri::command]
pub async fn p4_diff_local(conn: P4Conn, depot_file: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        p4::run_raw_stdout_diff(&conn, &["diff", "-du", &depot_file])
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// Open an opened file's local-vs-server diff in the external tool (depot #have
/// on the left, the live workspace file on the right).
#[tauri::command]
pub async fn open_diff_local(conn: P4Conn, depot_file: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let recs = p4::run(&conn, &["fstat", "-T", "clientFile", &depot_file])?;
        let local = recs
            .first()
            .and_then(|r| r.get("clientFile"))
            .and_then(|v| v.as_str())
            .ok_or("file is not in this workspace")?
            .to_string();
        let name = depot_file.rsplit('/').next().unwrap_or("file");
        let tmp = std::env::temp_dir().join(format!("p4gui_have_{name}"));
        let tmp_s = tmp.to_str().ok_or("bad temp path")?;
        p4::run_raw(&conn, &["print", "-q", "-o", tmp_s, &format!("{depot_file}#have")])?;
        let cmdline = p4diff_cmd(&conn)
            .ok_or("No external diff tool configured. Set P4DIFF (e.g. `p4 set P4DIFF=...`).")?;
        launch_diff(&cmdline, tmp_s, &local)
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// Unified diff of a shelved file vs its base revision: the pending change's
/// contribution. `rev` is the base revision from the shelved file list.
#[tauri::command]
pub async fn p4_diff_shelved(
    conn: P4Conn,
    depot_file: String,
    rev: i64,
    change: String,
) -> Result<String, String> {
    let a = format!("{depot_file}#{rev}");
    let b = format!("{depot_file}@={change}");
    tauri::async_runtime::spawn_blocking(move || p4::run_raw(&conn, &["diff2", "-u", &a, &b]))
        .await
        .map_err(|e| format!("diff task failed: {e}"))?
}

/// Open a shelved file's diff (base revision vs shelved) in the external tool.
#[tauri::command]
pub async fn open_diff_shelved(
    conn: P4Conn,
    depot_file: String,
    rev: i64,
    change: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = depot_file.rsplit('/').next().unwrap_or("file");
        let tmp = std::env::temp_dir();
        let prev = tmp.join(format!("p4gui_base{rev}_{name}"));
        let cur = tmp.join(format!("p4gui_shelf{change}_{name}"));
        let prev_s = prev.to_str().ok_or("bad temp path")?;
        let cur_s = cur.to_str().ok_or("bad temp path")?;
        p4::run_raw(&conn, &["print", "-q", "-o", prev_s, &format!("{depot_file}#{rev}")])?;
        p4::run_raw(&conn, &["print", "-q", "-o", cur_s, &format!("{depot_file}@={change}")])?;
        let cmdline = p4diff_cmd(&conn)
            .ok_or("No external diff tool configured. Set P4DIFF (e.g. `p4 set P4DIFF=...`).")?;
        launch_diff(&cmdline, prev_s, cur_s)
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// Unified diff of a file at a changelist vs its previous revision (in-app).
/// Empty string when the file was added (rev 1 — no prior revision).
#[tauri::command]
pub async fn p4_diff2(conn: P4Conn, depot_file: String, rev: i64) -> Result<String, String> {
    if rev <= 1 {
        return Ok(String::new());
    }
    let a = format!("{depot_file}#{}", rev - 1);
    let b = format!("{depot_file}#{rev}");
    tauri::async_runtime::spawn_blocking(move || p4::run_raw(&conn, &["diff2", "-u", &a, &b]))
        .await
        .map_err(|e| format!("diff task failed: {e}"))?
}

/// Open the file's diff (this revision vs the previous) in the external diff
/// tool configured as P4DIFF. Prints both revisions to temp files and launches
/// P4DIFF on them.
#[tauri::command]
pub async fn open_diff(conn: P4Conn, depot_file: String, rev: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_diff_blocking(&conn, &depot_file, rev))
        .await
        .map_err(|e| format!("diff task failed: {e}"))?
}

fn open_diff_blocking(conn: &P4Conn, depot_file: &str, rev: i64) -> Result<(), String> {
    if rev <= 1 {
        return Err("File was added in this changelist — no previous revision to diff.".into());
    }
    let base = depot_file.rsplit('/').next().unwrap_or("file");
    let tmp = std::env::temp_dir();
    let cur = tmp.join(format!("p4gui_r{rev}_{base}"));
    let prev = tmp.join(format!("p4gui_r{}_{base}", rev - 1));
    let cur_s = cur.to_str().ok_or("bad temp path")?;
    let prev_s = prev.to_str().ok_or("bad temp path")?;

    p4::run_raw(conn, &["print", "-q", "-o", cur_s, &format!("{depot_file}#{rev}")])?;
    p4::run_raw(conn, &["print", "-q", "-o", prev_s, &format!("{depot_file}#{}", rev - 1)])?;

    let cmdline = p4diff_cmd(conn)
        .ok_or("No external diff tool configured. Set P4DIFF (e.g. `p4 set P4DIFF=...`).")?;
    launch_diff(&cmdline, prev_s, cur_s)
}

/// The configured P4DIFF command string (from `p4 set P4DIFF`), if any.
fn p4diff_cmd(conn: &P4Conn) -> Option<String> {
    let out = p4::run_raw(conn, &["set", "P4DIFF"]).ok()?;
    let line = out.lines().next()?;
    let v = line.strip_prefix("P4DIFF=")?.trim();
    // Values look like: `code --wait --diff (set)` — drop the trailing origin tag.
    let v = match v.rfind(" (") {
        Some(i) => v[..i].trim(),
        None => v,
    };
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Launch `<P4DIFF> <prev> <cur>` via `cmd /c` so PATHEXT resolves launchers
/// like `code.cmd`. Fire-and-forget (does not block on the tool closing).
fn launch_diff(cmdline: &str, prev: &str, cur: &str) -> Result<(), String> {
    let mut c = std::process::Command::new("cmd");
    c.arg("/c");
    for part in cmdline.split_whitespace() {
        c.arg(part);
    }
    c.arg(prev).arg(cur);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.spawn().map_err(|e| format!("failed to launch diff tool: {e}"))?;
    Ok(())
}
