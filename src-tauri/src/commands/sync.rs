//! Syncing: streaming sync (parallel, live progress, lock-aware), cancel,
//! force/retry re-sync, and reconcile.

use super::{run, v, Res};
use crate::p4::{self, P4Conn};

/// Run `p4 sync [-n] [path]`, streaming stdout line-by-line and counting files.
/// Stores the child PID in `pids` (for cancellation). Emits throttled
/// `sync-progress` events (and `sync-issue` per stderr line) when `window` is set.
fn sync_run(
    conn: &P4Conn,
    pids: &std::sync::Mutex<Vec<u32>>,
    path: Option<&str>,
    preview: bool,
    parallel: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use tauri::Emitter;

    let mut cmd = p4::base_command(conn);
    // Bound network waits (seconds of silence) so a hung connection — a real
    // risk with parallel transfer on a flaky link — aborts with an error
    // instead of stalling the sync forever, like P4V's timeout.
    cmd.arg("-vnet.maxwait=120");
    cmd.arg("sync");
    if preview {
        cmd.arg("-n");
    } else if parallel {
        // Parallel file transfer, like P4V. The server caps it at
        // net.parallel.max; where it's disabled p4 may error, and the caller
        // then retries without it.
        cmd.arg("--parallel=threads=4");
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

    // Drain stderr on its own thread (so a full stderr pipe can't deadlock the
    // stdout loop) and surface each error line live via a `sync-issue` event —
    // so problems like files locked by the editor show up as they happen
    // instead of looking like a hang.
    let stderr = child.stderr.take();
    let issue_win = window.cloned();
    let err_handle = std::thread::spawn(move || {
        let mut all = String::new();
        let mut n = 0usize;
        if let Some(se) = stderr {
            for line in BufReader::new(se).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                n += 1;
                all.push_str(&line);
                all.push('\n');
                if let Some(w) = &issue_win {
                    let file = error_file(&line);
                    let _ = w.emit(
                        "sync-issue",
                        serde_json::json!({ "count": n, "line": line, "file": file }),
                    );
                }
            }
        }
        (n, all)
    });

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
    let (_issues, err) = err_handle.join().unwrap_or((0, String::new()));
    // Fatal only when nothing synced (parallel-not-enabled, auth, connection).
    // Per-file issues (e.g. locked files) still synced the rest and are shown
    // live via `sync-issue` + summarised by the caller, so they aren't fatal.
    if !status.success() && count == 0 && !err.trim().is_empty() {
        return Err(err.trim().to_string());
    }
    Ok(count)
}

/// Best-effort extract of the file path from a p4 sync error line, for the
/// "fix" (force-sync). Prefers a depot path (`//…`), else a Windows client
/// path (`X:\…`), trimmed of any trailing `: <error text>`.
fn error_file(line: &str) -> Option<String> {
    if let Some(i) = line.find("//") {
        let rest = &line[i..];
        let end = rest.find(|c: char| c.is_whitespace() || c == '#').unwrap_or(rest.len());
        if end > 2 {
            return Some(rest[..end].to_string());
        }
    }
    let b = line.as_bytes();
    for i in 0..b.len().saturating_sub(2) {
        if b[i].is_ascii_alphabetic() && b[i + 1] == b':' && b[i + 2] == b'\\' {
            let rest = &line[i..];
            let path = match rest.find(": ") {
                Some(j) => &rest[..j],
                None => rest,
            }
            .trim();
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    None
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
        let p = path.as_deref();
        // Try a parallel sync; if the server rejects parallel, retry sequentially.
        match sync_run(&conn, &pids, p, false, true, Some(&window)) {
            Err(e) if e.to_lowercase().contains("parallel") => {
                sync_run(&conn, &pids, p, false, false, Some(&window))
            }
            other => other,
        }
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
/// `@1234`), syncs just that — forward OR backward.
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

/// Re-sync specific files. Plain retry (for files that were locked and are now
/// free) unless `force` is set, in which case `-f` overwrites writable/stuck
/// files — DISCARDING local changes (caller must confirm).
#[tauri::command]
pub async fn p4_resync(conn: P4Conn, files: Vec<String>, force: bool) -> Res {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["sync".to_string()];
    if force {
        args.push("-f".to_string());
    }
    args.extend(files);
    run(conn, args).await
}

/// Reconcile offline work under `path` (`p4 reconcile <path>/...`): open files
/// that were changed / added / deleted outside Perforce, into the default
/// changelist. Returns the opened files (empty when there's nothing to do).
#[tauri::command]
pub async fn p4_reconcile(conn: P4Conn, path: String) -> Res {
    let spec = if path.is_empty() {
        "...".to_string()
    } else {
        format!("{}/...", path.trim_end_matches('/'))
    };
    run(conn, v(&["reconcile", &spec])).await
}
