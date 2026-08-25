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
    paths: &[String],
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
    let mut log_args: Vec<String> = vec!["sync".into()];
    if preview {
        cmd.arg("-n");
        log_args.push("-n".into());
    } else if parallel {
        // Parallel file transfer, like P4V. The server caps threads at
        // net.parallel.max; where it's disabled p4 may error, and the caller
        // then retries without it.
        //
        // batch/batchsize matter far more than threads for UE-sized trees: the
        // defaults (8 files / 512 KB per batch) mean a round trip every few
        // small files, and at ~100 ms RTT to a remote server (e.g. Epic in
        // us-east) that latency dominates a million-file sync. Bigger batches
        // cut the round trips; `min` keeps tiny syncs sequential (parallelism
        // costs more than it saves below a handful of files).
        // (batchsize is in BYTES — `2M` is rejected with a usage error.)
        const PARALLEL: &str = "--parallel=threads=8,batch=64,batchsize=2097152,min=8";
        cmd.arg(PARALLEL);
        log_args.push(PARALLEL.into());
    }
    for p in paths.iter().filter(|p| !p.is_empty()) {
        cmd.arg(p);
        log_args.push(p.clone());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let started = std::time::Instant::now();
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
                if line.trim().is_empty() || is_benign_sync_message(&line) {
                    continue; // informational, not an issue (see is_benign_sync_message)
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
    // The streaming sync bypasses p4::run — log it to the Commands view too,
    // with the stderr tail so a failed sync is diagnosable there.
    {
        let refs: Vec<&str> = log_args.iter().map(String::as_str).collect();
        let log_err = if status.success() { String::new() } else { err.trim().to_string() };
        p4::log_command_err(&refs, started.elapsed().as_millis(), status.success(), &log_err);
    }
    // Fatal only when nothing synced (parallel-not-enabled, auth, connection).
    // Per-file issues (e.g. locked files) still synced the rest and are shown
    // live via `sync-issue` + summarised by the caller, so they aren't fatal.
    if !status.success() && count == 0 && !err.trim().is_empty() {
        return Err(err.trim().to_string());
    }
    Ok(count)
}

/// p4 writes informational results to stderr too — "up-to-date", "no such
/// file(s)", "not on client" (an unsync with nothing to remove). Those are NOT
/// problems: reporting them as sync issues turns a successful no-op into an
/// error dialog, so they're dropped from the issue stream.
fn is_benign_sync_message(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("up-to-date")
        || l.contains("no such file")
        || l.contains("file(s) not on client")
        || l.contains("no file(s) to")
        || l.contains("not in client view")
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
    paths: Vec<String>,
) -> Result<usize, String> {
    use std::sync::atomic::Ordering;

    let pids = state.sync_pids.clone();
    let abort = state.sync_abort.clone();
    abort.store(false, Ordering::SeqCst);

    let count = tauri::async_runtime::spawn_blocking(move || {
        // Try a parallel sync; if the server rejects parallel, retry sequentially.
        match sync_run(&conn, &pids, &paths, false, true, Some(&window)) {
            Err(e) if e.to_lowercase().contains("parallel") => {
                sync_run(&conn, &pids, &paths, false, false, Some(&window))
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
///
/// A non-force retry additionally EXCLUDES files that carry offline
/// modifications (per `p4 reconcile -n`): "noclobber" only refuses WRITABLE
/// files, so a modified-but-read-only file — or any file on a clobber client —
/// would be silently replaced by a plain sync, losing the local work. Excluded
/// files come back as `{action:"protected", depotFile}` records so the caller
/// keeps them listed; overwriting them requires `force`.
#[tauri::command]
pub async fn p4_resync(conn: P4Conn, files: Vec<String>, force: bool) -> Res {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let run_ref = |args: &[String]| {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            p4::run(&conn, &refs)
        };
        // Sync the batch surfacing PARTIAL failures: successes' data records
        // would otherwise mask error records (lenient parse), silently clearing
        // a still-broken file from the error dialog. Each error is paired back
        // to its file (the message text carries the path) and returned as an
        // {action:"failed"} record so the caller keeps it listed.
        let sync_batch = |targets: Vec<String>, force: bool| -> Res {
            let mut args = vec!["sync".to_string()];
            if force {
                args.push("-f".to_string());
            }
            args.extend(targets.iter().cloned());
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let (mut out, errs) = p4::run_full(&conn, &refs)?;
            let norm = |s: &str| s.replace('\\', "/").to_lowercase();
            for e in errs {
                let en = norm(&e);
                let file = targets.iter().find(|f| en.contains(&norm(f))).cloned();
                let mut r = crate::p4::Record::new();
                r.insert("action".into(), "failed".into());
                r.insert("depotFile".into(), file.unwrap_or_default().into());
                r.insert("message".into(), e.into());
                out.push(r);
            }
            Ok(out)
        };
        if force {
            return sync_batch(files, true);
        }
        // Which of the targets have offline edits? Fail CLOSED — if the check
        // itself fails, sync nothing rather than risk overwriting local work.
        let mut rec = vec!["reconcile".into(), "-n".into(), "-e".into(), "-m".into()];
        rec.extend(files.iter().cloned());
        let modified = run_ref(&rec)
            .map_err(|e| format!("offline-change check failed — nothing was synced: {e}"))?;
        let norm = |s: &str| s.replace('\\', "/").to_lowercase();
        let mut protected: Vec<String> = Vec::new();
        for r in &modified {
            for key in ["depotFile", "clientFile"] {
                if let Some(v) = r.get(key).and_then(|v| v.as_str()) {
                    protected.push(norm(v));
                }
            }
        }
        let (kept, to_sync): (Vec<String>, Vec<String>) =
            files.into_iter().partition(|f| protected.contains(&norm(f)));
        let mut out = Vec::new();
        if !to_sync.is_empty() {
            out.extend(sync_batch(to_sync, false)?);
        }
        for f in kept {
            let mut r = crate::p4::Record::new();
            r.insert("action".into(), "protected".into());
            r.insert("depotFile".into(), f.into());
            out.push(r);
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("resync task failed: {e}"))?
}

/// Repair a have/disk desync: `p4 flush file#head` updates the have record to
/// head WITHOUT touching the file on disk — the right fix when the disk content
/// already matches head but the record lags (see `mark_desyncs`).
#[tauri::command]
pub async fn p4_flush(conn: P4Conn, files: Vec<String>) -> Res {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["flush".to_string()];
    args.extend(files.into_iter().map(|f| format!("{f}#head")));
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

/// Check out specific offline-modified files: `p4 reconcile <files…>` (exact
/// paths, no wildcard) opens each as edit/add/delete — reconcile picks the action
/// per file, which is why this and not `p4 edit`. `change` targets a changelist;
/// empty means the default one.
#[tauri::command]
pub async fn p4_reconcile_files(conn: P4Conn, files: Vec<String>, change: String) -> Res {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["reconcile".to_string()];
    if !change.is_empty() && change != "default" {
        args.push("-c".to_string());
        args.push(change);
    }
    args.extend(files);
    run(conn, args).await
}

/// Revert offline changes on specific files: `p4 clean <files…>` restores each
/// to its depot state (re-syncs modified files, removes added ones, restores
/// deleted ones). DESTRUCTIVE for the local edits — caller must confirm.
#[tauri::command]
pub async fn p4_clean(conn: P4Conn, files: Vec<String>) -> Res {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["clean".to_string()];
    args.extend(files);
    run(conn, args).await
}

/// Read-only preview of offline changes across the whole workspace: tracked
/// files modified or missing on disk but not open in any changelist
/// (`p4 reconcile -n -e -d -m //<client>/...`). `-m` compares modification times
/// instead of digesting every file (~24s vs minutes), and `-a` (new-file
/// discovery, a full filesystem walk) is omitted — both are what make reconcile
/// slow. The cost is disk-stat-bound and can't be parallelized meaningfully, so
/// callers scan on a low-rate background timer. Each record carries `action`
/// (edit/delete) plus depotFile/clientFile.
#[tauri::command]
pub async fn p4_status(
    state: tauri::State<'_, crate::index::AppState>,
    conn: P4Conn,
) -> Res {
    use std::sync::atomic::Ordering;
    let spec = if conn.client.is_empty() {
        "...".to_string()
    } else {
        format!("//{}/...", conn.client)
    };
    let pid_slot = state.offline_pid.clone();
    let abort = state.offline_abort.clone();
    abort.store(false, Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        let res = p4::run_killable(&conn, &["reconcile", "-n", "-e", "-d", "-m", &spec], &pid_slot);
        // If it was cancelled (killed by an interactive write), report an error
        // so the caller keeps the previous list instead of clearing it.
        if abort.swap(false, Ordering::SeqCst) {
            return Err("offline scan cancelled".to_string());
        }
        let mut recs = res?;
        mark_desyncs(&conn, &mut recs);
        Ok(recs)
    })
    .await
    .map_err(|e| format!("status task failed: {e}"))?
}

/// Distinguish REAL offline edits from have/disk desyncs: a file whose disk
/// content matches HEAD while the have record lags behind (e.g. an interrupted
/// sync that wrote the file but never recorded it) diffs against have exactly
/// like an offline edit. For edit-flagged files with have != head, diff the
/// client file against #head (`p4 diff -f -sa file#head` lists only files that
/// DIFFER); the ones that don't differ are marked `desync: true` so the UI can
/// present them as a repairable record problem, not local work. Best effort —
/// on any failure the records stay as plain edits.
fn mark_desyncs(conn: &P4Conn, recs: &mut [p4::Record]) {
    let edits: Vec<String> = recs
        .iter()
        .filter(|r| r.get("action").and_then(|v| v.as_str()) == Some("edit"))
        .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(String::from))
        .collect();
    if edits.is_empty() {
        return;
    }
    // have vs head for the flagged files (one fstat).
    let mut args: Vec<String> = vec!["fstat".into(), "-T".into(), "depotFile,haveRev,headRev".into()];
    args.extend(edits.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Ok(stats) = p4::run(conn, &refs) else { return };
    let behind: Vec<String> = stats
        .iter()
        .filter(|r| {
            let have = r.get("haveRev").and_then(|v| v.as_str()).unwrap_or("");
            let head = r.get("headRev").and_then(|v| v.as_str()).unwrap_or("");
            !have.is_empty() && !head.is_empty() && have != head
        })
        .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(String::from))
        .collect();
    if behind.is_empty() {
        return;
    }
    // Which of those actually differ from head on disk? (-sa lists differing.)
    let mut args: Vec<String> = vec!["diff".into(), "-f".into(), "-sa".into()];
    args.extend(behind.iter().map(|f| format!("{f}#head")));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Ok(differing) = p4::run(conn, &refs) else { return };
    let differs: Vec<String> = differing
        .iter()
        .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()))
        .map(|s| s.to_lowercase())
        .collect();
    for r in recs.iter_mut() {
        let Some(df) = r.get("depotFile").and_then(|v| v.as_str()) else { continue };
        let dfl = df.to_lowercase();
        if behind.iter().any(|b| b.to_lowercase() == dfl) && !differs.contains(&dfl) {
            r.insert("desync".into(), true.into());
        }
    }
}

/// A depot path that differs from another only in case.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaseTwin {
    pub file: String,
    pub twin: String,
}

/// The sibling that matches `name` ignoring case but is not `name` itself.
fn case_twin_name(name: &str, siblings: &[String]) -> Option<String> {
    siblings
        .iter()
        .find(|s| s.as_str() != name && s.eq_ignore_ascii_case(name))
        .cloned()
}

/// Split `//depot/a/b/c.uasset` into ("//depot", ["a", "b", "c.uasset"]).
fn split_depot(path: &str) -> Option<(String, Vec<String>)> {
    let rest = path.strip_prefix("//")?;
    let mut parts = rest.split('/');
    let depot = parts.next()?;
    let tail: Vec<String> = parts.map(str::to_string).collect();
    if tail.is_empty() {
        return None;
    }
    Some((format!("//{depot}"), tail))
}

/// Rebuild a path with component `idx` replaced.
fn with_component(root: &str, parts: &[String], idx: usize, name: &str) -> String {
    let mut out = root.to_string();
    for (i, p) in parts.iter().enumerate() {
        out.push('/');
        out.push_str(if i == idx { name } else { p.as_str() });
    }
    out
}

/// For each path, a depot path differing from it only in case — if one exists AND
/// really holds that file.
///
/// The clash can be at any level, so every ancestor is checked as well as the
/// file name itself; listings are cached, so a set of files under one directory
/// costs one walk rather than one per file.
#[tauri::command]
pub async fn p4_case_twins(conn: P4Conn, files: Vec<String>) -> Result<Vec<CaseTwin>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut dirs_cache: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut files_cache: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut out = Vec::new();

        for file in &files {
            let Some((depot, parts)) = split_depot(file) else { continue };
            let mut found: Option<String> = None;

            // The file name itself, then each directory from the deepest up.
            for idx in (0..parts.len()).rev() {
                let parent = with_component(&depot, &parts[..idx], usize::MAX, "");
                let parent = parent.trim_end_matches('/').to_string();
                let siblings = if idx == parts.len() - 1 {
                    files_cache
                        .entry(parent.clone())
                        .or_insert_with(|| list_leaf_names(&conn, &parent))
                        .clone()
                } else {
                    dirs_cache
                        .entry(parent.clone())
                        .or_insert_with(|| list_dir_names(&conn, &parent))
                        .clone()
                };
                if let Some(twin) = case_twin_name(&parts[idx], &siblings) {
                    let candidate = with_component(&depot, &parts, idx, &twin);
                    // A twin directory does not have to contain our file.
                    if p4::run(&conn, &["files", &candidate]).map(|r| !r.is_empty()).unwrap_or(false) {
                        found = Some(candidate);
                        break;
                    }
                }
            }
            if let Some(twin) = found {
                out.push(CaseTwin { file: file.clone(), twin });
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Sub-directory names of a depot directory.
fn list_dir_names(conn: &P4Conn, parent: &str) -> Vec<String> {
    let spec = format!("{parent}/*");
    p4::run(conn, &["dirs", &spec])
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.get("dir").and_then(|v| v.as_str()))
        .filter_map(|d| d.rsplit('/').next().map(str::to_string))
        .collect()
}

/// File names in a depot directory.
fn list_leaf_names(conn: &P4Conn, parent: &str) -> Vec<String> {
    let spec = format!("{parent}/*");
    p4::run(conn, &["files", &spec])
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()))
        .filter_map(|d| d.rsplit('/').next().map(str::to_string))
        .collect()
}

/// Path of this session's command log, for the Commands view ("" if unavailable).
#[tauri::command]
pub fn session_log_path() -> String {
    p4::session_log_path()
}

/// What actually happened to one file in a revert.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RevertOutcome {
    pub file: String,
    pub ok: bool,
    /// "reverted" (was open) | "cleaned" (offline change) | "" when nothing worked.
    pub how: String,
    pub message: String,
}

/// Revert a mixed selection: open files are `p4 revert`ed, files with offline
/// changes are `p4 clean`ed, and the result of each is CHECKED.
///
/// Both halves of that used to be guesses. Which command a file got was decided
/// from the cached offline list, so a file the cache did not know about was sent
/// to `p4 revert` — which for a file that is not open is a warning with exit
/// status 0, i.e. nothing at all. And no result was read back, so the rows were
/// removed from the UI either way and reappeared at the next scan with no
/// explanation. Now p4 says which files are open, and p4 says afterwards whether
/// they are clean.
#[tauri::command]
pub async fn p4_revert_local(
    conn: P4Conn,
    files: Vec<String>,
) -> Result<Vec<RevertOutcome>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();

        // Who is actually open? The authoritative split.
        let mut args = vec!["opened"];
        args.extend(refs.iter());
        let opened_recs = p4::run(&conn, &args).unwrap_or_default();
        let open: Vec<String> = opened_recs
            .iter()
            .filter(|r| r.contains_key("action"))
            .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(str::to_string))
            .collect();

        let mut notes: Vec<String> = Vec::new();
        if !open.is_empty() {
            let mut a = vec!["revert".to_string()];
            a.extend(open.clone());
            let argv: Vec<&str> = a.iter().map(String::as_str).collect();
            if let Ok((_recs, mut n)) = p4::run_notes(&conn, &argv) {
                notes.append(&mut n);
            }
        }
        let offline: Vec<String> = files.iter().filter(|f| !open.contains(f)).cloned().collect();
        if !offline.is_empty() {
            let mut a = vec!["clean".to_string()];
            a.extend(offline.clone());
            let argv: Vec<&str> = a.iter().map(String::as_str).collect();
            if let Ok((_recs, mut n)) = p4::run_notes(&conn, &argv) {
                notes.append(&mut n);
            }
        }

        // Verify: still open, or still differing from the depot?
        let mut args = vec!["opened"];
        args.extend(refs.iter());
        let still_open: Vec<String> = p4::run(&conn, &args)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        let mut args = vec!["reconcile", "-n", "-e", "-d"];
        args.extend(refs.iter());
        let still_dirty: Vec<String> = p4::run(&conn, &args)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(str::to_string))
            .collect();

        Ok(files
            .iter()
            .map(|f| {
                outcome(
                    f,
                    open.contains(f),
                    still_open.contains(f),
                    still_dirty.contains(f),
                    &notes,
                )
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The verdict for one file. `notes` are p4's warnings for the whole run — the
/// only place a per-file refusal is explained, since those come back with exit
/// status 0.
fn outcome(
    file: &str,
    was_open: bool,
    still_open: bool,
    still_dirty: bool,
    notes: &[String],
) -> RevertOutcome {
    let leaf = file.rsplit('/').next().unwrap_or(file);
    let mine: Vec<&str> = notes
        .iter()
        .map(String::as_str)
        .filter(|n| n.contains(file) || n.contains(leaf))
        .collect();
    if !still_open && !still_dirty {
        return RevertOutcome {
            file: file.to_string(),
            ok: true,
            how: if was_open { "reverted".into() } else { "cleaned".into() },
            message: String::new(),
        };
    }
    let why = if !mine.is_empty() {
        mine.join(" — ")
    } else if still_open {
        "p4 still reports the file as open.".to_string()
    } else {
        "the file on disk still differs from the depot.".to_string()
    };
    RevertOutcome {
        file: file.to_string(),
        ok: false,
        how: String::new(),
        message: why,
    }
}

/// Why one file is standing in the way of a sync.
///
/// `kind` is the state, and it is what decides the remedy:
///   untracked  a local file sits where a depot revision belongs, and this
///              workspace has no have-record for it. NOTHING else reports this:
///              the offline scan (`reconcile -e -d`) looks for edits and deletes
///              of files p4 believes you hold, and this is neither.
///   modified   held and then changed on disk without being checked out — an
///              offline change. Forcing discards it.
///   writable   identical to the revision held; only the read-only flag is
///              cleared, which is all p4 needs to refuse. Forcing loses nothing.
///   gone       nothing there any more; a retry will work.
///   unknown    p4 knows no such path.
///
/// Content is compared by `p4 diff -se` rather than by hashing the file here:
/// that is p4's own comparison, so text translation and keyword expansion cannot
/// make an identical file look modified.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SyncBlocker {
    pub file: String,
    pub depot_file: String,
    pub client_file: String,
    pub kind: String,
    pub reason: String,
    pub have_rev: String,
    pub head_rev: String,
    pub local_size: u64,
    pub depot_size: u64,
}

fn field(rec: &p4::Record, key: &str) -> String {
    rec.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// Digit grouping, so 40415 reads as 40 415 in a sentence.
fn grouped(n: u64) -> String {
    let mut s = n.to_string();
    let mut i = s.len() as i64 - 3;
    while i > 0 {
        s.insert(i as usize, ' ');
        i -= 3;
    }
    s
}

/// Does p4 consider this unopened file different from the revision held?
/// `diff -se` prints the file when it differs and nothing when it matches.
fn differs_unopened(conn: &P4Conn, file: &str) -> bool {
    match p4::run_full(conn, &["diff", "-se", file]) {
        Ok((recs, _notes)) => recs.iter().any(|r| r.contains_key("depotFile")),
        Err(_) => false,
    }
}

/// Classify each path a sync refused to overwrite.
#[tauri::command]
pub async fn p4_sync_blockers(conn: P4Conn, files: Vec<String>) -> Result<Vec<SyncBlocker>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for file in files {
            let recs = p4::run(&conn, &["fstat", "-Ol", &file]).unwrap_or_default();
            let Some(rec) = recs.first() else {
                out.push(SyncBlocker {
                    file: file.clone(),
                    depot_file: String::new(),
                    client_file: file.clone(),
                    kind: "unknown".into(),
                    reason: "p4 has no record of this path.".into(),
                    have_rev: String::new(),
                    head_rev: String::new(),
                    local_size: 0,
                    depot_size: 0,
                });
                continue;
            };
            let depot_file = field(rec, "depotFile");
            let client_file = {
                let c = field(rec, "clientFile");
                if c.is_empty() { file.clone() } else { c }
            };
            let have_rev = field(rec, "haveRev");
            let head_rev = field(rec, "headRev");
            let depot_size = field(rec, "fileSize").parse::<u64>().unwrap_or(0);
            let local_size = std::fs::metadata(&client_file).map(|m| m.len()).unwrap_or(0);
            let exists = std::fs::metadata(&client_file).is_ok();

            let (kind, reason) = if !exists {
                ("gone", "The file is no longer on disk — a retry will work.".to_string())
            } else if have_rev.is_empty() {
                (
                    "untracked",
                    format!(
                        "This workspace has never synced this file, so Perforce has no record of the {} bytes on disk.                          The depot has #{} ({} bytes). The offline scan cannot see it either: it reports edits and deletes                          of files you hold, and this is neither. Force replaces the local copy with the depot's — move it                          aside first if you might want it.",
                        grouped(local_size),
                        head_rev,
                        grouped(depot_size)
                    ),
                )
            } else if differs_unopened(&conn, &client_file) {
                (
                    "modified",
                    format!(
                        "Changed on disk without being checked out — an offline change ({} bytes here against #{}).                          Force discards it; check it out instead to keep it.",
                        grouped(local_size),
                        have_rev
                    ),
                )
            } else {
                (
                    "writable",
                    format!(
                        "Identical to the revision you hold (#{have_rev}); only the read-only flag is cleared, which is                          all p4 needs to refuse. Force loses nothing."
                    ),
                )
            };
            out.push(SyncBlocker {
                file,
                depot_file,
                client_file,
                kind: kind.into(),
                reason,
                have_rev,
                head_rev,
                local_size,
                depot_size,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Kill the in-flight offline-changes scan, called before an interactive write.
/// Killing the client is sufficient: p4d abandons its own `reconcile` about a
/// second later (watched in `p4 monitor show`). Asking the server directly, with
/// `p4 monitor terminate`, is refused for our own process here even though the
/// help offers it to the owner of the process id. No-op if none is running.
#[tauri::command]
pub async fn cancel_offline_scan(
    state: tauri::State<'_, crate::index::AppState>,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let pid = *state.offline_pid.lock().unwrap();
    if let Some(pid) = pid {
        state.offline_abort.store(true, Ordering::SeqCst);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(0x0800_0000)
                .output();
        }
    }
    Ok(())
}

#[cfg(test)]
mod revert_tests {
    use super::*;

    #[test]
    fn a_file_that_is_clean_afterwards_says_how_it_got_there() {
        let r = outcome("//d/a.uasset", true, false, false, &[]);
        assert!(r.ok && r.how == "reverted");
        let r = outcome("//d/a.uasset", false, false, false, &[]);
        assert!(r.ok && r.how == "cleaned");
    }

    #[test]
    fn a_file_still_dirty_is_a_failure_with_a_reason() {
        // The case that used to pass silently: p4 revert on a file that is not
        // open does nothing, and the row came back at the next scan.
        let r = outcome("//d/a.uasset", false, false, true, &[]);
        assert!(!r.ok);
        assert!(r.message.contains("still differs"));
        assert_eq!(r.how, "");
    }

    #[test]
    fn p4s_own_warning_is_preferred_over_ours() {
        let notes = vec!["//d/a.uasset - file(s) not opened on this client.".to_string()];
        let r = outcome("//d/a.uasset", false, false, true, &notes);
        assert!(!r.ok);
        assert!(r.message.contains("not opened on this client"));
    }

    #[test]
    fn a_note_about_another_file_is_not_borrowed() {
        let notes = vec!["//d/other.uasset - no such file(s).".to_string()];
        let r = outcome("//d/a.uasset", false, true, false, &notes);
        assert!(!r.ok);
        assert!(r.message.contains("still reports the file as open"));
    }
}

#[cfg(test)]
mod case_tests {
    use super::*;

    #[test]
    fn finds_the_sibling_that_differs_only_in_case() {
        let sibs = vec!["JellyClip".to_string(), "Jellyclip".to_string(), "Fluff".to_string()];
        assert_eq!(case_twin_name("JellyClip", &sibs), Some("Jellyclip".to_string()));
        assert_eq!(case_twin_name("Jellyclip", &sibs), Some("JellyClip".to_string()));
    }

    #[test]
    fn an_exact_match_is_not_a_twin() {
        let sibs = vec!["Meshes".to_string(), "Textures".to_string()];
        assert_eq!(case_twin_name("Meshes", &sibs), None);
    }

    #[test]
    fn a_path_is_rebuilt_with_one_component_swapped() {
        let (depot, parts) = split_depot("//Curiosity/main/Content/JellyClip/M/A.uasset").unwrap();
        assert_eq!(depot, "//Curiosity");
        assert_eq!(parts.len(), 5);
        assert_eq!(
            with_component(&depot, &parts, 2, "jellyclip"),
            "//Curiosity/main/Content/jellyclip/M/A.uasset"
        );
    }

    #[test]
    fn a_depot_root_alone_is_not_a_path() {
        assert!(split_depot("//Curiosity").is_none());
        assert!(split_depot("not-a-depot-path").is_none());
    }
}
