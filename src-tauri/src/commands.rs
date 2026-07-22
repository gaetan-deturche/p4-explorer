//! Tauri commands exposed to the front-end.
//!
//! These are `async` on purpose: a synchronous `#[tauri::command]` runs on the
//! main thread, so a blocking `p4` subprocess call would serialize every invoke
//! and freeze the webview until all of them finish (making concurrent front-end
//! requests pointless). Each command instead offloads the blocking `p4::run` to
//! a blocking thread pool via `spawn_blocking`, so multiple invokes run in
//! parallel and each result returns — and paints — independently.

use crate::p4::{self, P4Conn, Record};

type Res = Result<Vec<Record>, String>;

/// Run `p4` off the main thread so commands don't serialize / block the UI.
async fn run(conn: P4Conn, args: Vec<String>) -> Res {
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        p4::run(&conn, &refs)
    })
    .await
    .map_err(|e| format!("p4 task failed: {e}"))?
}

fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

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

/// Pending changelists for the connection's client/user.
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

/// Run `p4 sync [-n] [path]`, streaming stdout line-by-line and counting files.
/// Stores the child PID in `pid_slot` (for cancellation). Emits throttled
/// `sync-progress` events when `window` is provided (the real sync, not preview).
fn sync_run(
    conn: &P4Conn,
    pids: &std::sync::Mutex<Vec<u32>>,
    path: Option<&str>,
    preview: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    use tauri::Emitter;

    let mut cmd = p4::base_command(conn);
    cmd.arg("sync");
    if preview {
        cmd.arg("-n");
    }
    if let Some(p) = path {
        if !p.is_empty() {
            cmd.arg(p);
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("failed to launch p4: {e}"))?;
    let id = child.id();
    pids.lock().unwrap().push(id);

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut count = 0usize;
    let mut last = String::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        count += 1;
        last = line;
        if let Some(w) = window {
            if count % 10 == 0 {
                let _ = w.emit("sync-progress", serde_json::json!({ "count": count, "line": last }));
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    pids.lock().unwrap().retain(|&p| p != id);
    if let Some(w) = window {
        let _ = w.emit("sync-progress", serde_json::json!({ "count": count, "line": last }));
    }
    if !status.success() {
        let mut err = String::new();
        if let Some(mut se) = child.stderr.take() {
            let _ = se.read_to_string(&mut err);
        }
        if !err.trim().is_empty() {
            return Err(err.trim().to_string());
        }
    }
    Ok(count)
}

/// Streaming sync: runs `p4 sync [path]` and emits throttled `sync-progress`
/// events (running file count + current file). Returns files synced.
/// Cancellable via `sync_cancel`. (No estimate pass — a concurrent `sync -n`
/// contends on the client lock and stalls the real sync.)
#[tauri::command]
pub async fn p4_sync_stream(
    window: tauri::Window,
    state: tauri::State<'_, crate::index::AppState>,
    conn: P4Conn,
    path: Option<String>,
) -> Result<usize, String> {
    use std::sync::atomic::Ordering;

    let pids = state.sync_pids.clone();
    let abort = state.sync_abort.clone();
    abort.store(false, Ordering::SeqCst);

    let count = tauri::async_runtime::spawn_blocking(move || {
        sync_run(&conn, &pids, path.as_deref(), false, Some(&window))
    })
    .await
    .map_err(|e| format!("sync task failed: {e}"))??;

    if abort.load(Ordering::SeqCst) {
        return Err("Sync cancelled.".into());
    }
    Ok(count)
}

/// Cancel a running sync: flag the abort and kill all `p4` sync children.
#[tauri::command]
pub async fn sync_cancel(state: tauri::State<'_, crate::index::AppState>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    state.sync_abort.store(true, Ordering::SeqCst);
    let ids: Vec<u32> = state.sync_pids.lock().unwrap().clone();
    #[cfg(windows)]
    for pid in ids {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .output();
    }
    Ok(())
}

/// Sync to a revision. With no `path`, syncs the whole workspace to head
/// (`p4 sync`). With a `path` spec (e.g. `//depot/dir/...@1234` or a file with
/// `@1234`), syncs just that — forward OR backward. A workspace write, invoked
/// only from explicit user actions (Global Sync / "update to this changelist").
#[tauri::command]
pub async fn p4_sync(conn: P4Conn, path: Option<String>) -> Res {
    let mut args = vec!["sync".to_string()];
    if let Some(p) = path {
        if !p.is_empty() {
            args.push(p);
        }
    }
    run(conn, args).await
}

/// Depot-wide filename search under `root`: `p4 files //root/.../*term*`.
/// Case-sensitive (the server's case handling). Capped at `max` results.
#[tauri::command]
pub async fn p4_search(conn: P4Conn, root: String, term: String, max: u32) -> Res {
    let pattern = format!("{}/.../*{}*", root.trim_end_matches('/'), term);
    run(conn, v(&["files", "-m", &max.to_string(), &pattern])).await
}

/// Files open (in the workspace) for a pending changelist (`opened -c`).
#[tauri::command]
pub async fn p4_opened(conn: P4Conn, change: String) -> Res {
    run(conn, v(&["opened", "-c", &change])).await
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

/// The configured Swarm base URL (`p4 property -l -n P4.Swarm.URL`), or empty.
#[tauri::command]
pub async fn swarm_url(conn: P4Conn) -> Result<String, String> {
    let out = p4::run_raw(&conn, &["property", "-l", "-n", "P4.Swarm.URL"])?;
    let url = out
        .lines()
        .next()
        .and_then(|l| l.splitn(2, '=').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Ok(url)
}

/// All streams on the server (`p4 streams`).
#[tauri::command]
pub async fn p4_streams(conn: P4Conn) -> Res {
    run(conn, v(&["streams"])).await
}

/// Switch the connection's client to a different stream (`p4 switch <stream>`).
/// A workspace write; p4 refuses if files are open (surfaced as an error).
#[tauri::command]
pub async fn p4_switch(conn: P4Conn, stream: String) -> Res {
    run(conn, v(&["switch", &stream])).await
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

/// True only for tagged release builds: the release workflow compiles with the
/// AUGER_RELEASE env set. Dev/local (`--no-bundle`) builds return false so the
/// front-end skips the auto-update check (they carry a placeholder version).
#[tauri::command]
pub fn is_release_build() -> bool {
    option_env!("AUGER_RELEASE").is_some()
}
