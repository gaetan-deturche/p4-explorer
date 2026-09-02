//! Diffs: in-app unified diffs (changelist, shelved, local-vs-server) and
//! launching the configured external P4DIFF tool.

use crate::p4::{self, P4Conn};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// Prompt the native folder-picker and return the chosen directory, or None if
/// the user cancelled. `start` seeds the initial directory when it exists.
#[tauri::command]
pub async fn pick_folder(app: AppHandle, start: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dlg = app.dialog().file();
        if !start.is_empty() && std::path::Path::new(&start).is_dir() {
            dlg = dlg.set_directory(&start);
        }
        match dlg.blocking_pick_folder() {
            Some(fp) => Ok(Some(fp.into_path().map_err(|e| e.to_string())?.display().to_string())),
            None => Ok(None),
        }
    })
    .await
    .map_err(|e| format!("pick-folder task failed: {e}"))?
}

/// One file a patch carries, with the revision it is a change against.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PatchedFile {
    pub depot_file: String,
    pub action: String,
    /// The `have` revision the diff was taken against ("" for an add).
    pub rev: String,
    /// Carried as a git binary literal section rather than as hunks.
    pub binary: bool,
}

/// Build a unified-diff patch for `files`, or for all opened files of `change`
/// when `files` is empty. Returns the patch, what it carries, and the files it
/// could NOT carry (deletes — a unified diff has no way to say "remove this").
///
/// Shared by the Save-As export and by the stash: a stashed change is byte for
/// byte the `.patch` the export would have written, so it applies through the
/// same preview, the same fuzz placement and the same conflict handling.
pub(crate) fn build_patch(
    conn: &P4Conn,
    change: &str,
    files: Vec<String>,
) -> Result<(String, Vec<PatchedFile>, Vec<String>), String> {
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
    // Sort the targets: text files go through `p4 diff`; binaries and adds
    // carry no unified diff, so their WHOLE content is embedded as a
    // standard `GIT binary patch` literal section instead (adds included —
    // p4 has no diff for those either, whatever their type).
    let meta = p4::run(
        &conn,
        &{
            let mut a: Vec<&str> =
                vec!["fstat", "-T", "depotFile,headType,type,action,clientFile,haveRev,movedFile"];
            for t in &targets {
                a.push(t.as_str());
            }
            a
        },
    )
    .unwrap_or_default();
    let mut text_targets: Vec<String> = Vec::new();
    let mut embed: Vec<(String, String, bool)> = Vec::new(); // (depot, local, is_add)
    let mut deletes: Vec<String> = Vec::new();
    let mut carried: Vec<PatchedFile> = Vec::new();
    // depot path -> where p4 says it moved to/from. A move is a `move/delete` at
    // one path and a `move/add` at the other; pairing them is what turns two
    // half-sections into one rename the apply can carry out with `p4 move`.
    let mut moved: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for t in &targets {
        let r = meta
            .iter()
            .find(|r| r.get("depotFile").and_then(|v| v.as_str()) == Some(t.as_str()));
        let get = |k: &str| {
            r.and_then(|r| r.get(k)).and_then(|v| v.as_str()).unwrap_or("").to_string()
        };
        let action = get("action");
        let moved_to = get("movedFile");
        if !moved_to.is_empty() {
            moved.insert(t.clone(), moved_to);
        }
        if action == "delete" || action == "move/delete" {
            deletes.push(t.clone());
            continue;
        }
        let is_add = action == "add" || action == "move/add" || get("headType").is_empty();
        let binary = get("type").contains("binary") || get("headType").contains("binary");
        let local = get("clientFile");
        // What the patch will carry, and the revision it is a change AGAINST —
        // which is the only thing that lets a later apply say "this was taken
        // at #12 and you are on #17" instead of finding out hunk by hunk.
        carried.push(PatchedFile {
            depot_file: t.clone(),
            action: action.clone(),
            rev: get("haveRev"),
            binary,
        });
        if (is_add || binary) && !local.is_empty() {
            embed.push((t.clone(), local, is_add));
        } else {
            text_targets.push(t.clone());
        }
    }

    // Every path in the patch is written relative to the deepest folder all of
    // them share, and that folder is named in the preamble. `git apply` run from
    // the matching directory then needs no flags at all, while Auger reads the
    // preamble back and rebuilds the depot paths.
    let all: Vec<&str> = targets.iter().map(String::as_str).collect();
    let root = common_root(&all);

    // -f: force the diff even for files that aren't opened, so a patch can
    // be exported from OFFLINE-modified files too (no effect on opened ones).
    // #have is REQUIRED: for a file that isn't opened, a bare `p4 diff -f`
    // compares against #head, so on a workspace that is behind, the patch
    // would also carry the depot changes not yet synced.
    let mut patch = if text_targets.is_empty() {
        String::new()
    } else {
        let specs: Vec<String> = text_targets.iter().map(|t| format!("{t}#have")).collect();
        let mut args: Vec<&str> = vec!["diff", "-f", "-du"];
        for s in &specs {
            args.push(s.as_str());
        }
        gitify(conn, &p4::run_raw_stdout_diff(conn, &args)?, &root)
    };

    for (depot, local, is_add) in &embed {
        let data = std::fs::read(local)
            .map_err(|e| format!("cannot read {local} for the patch: {e}"))?;
        // `git apply` verifies the OLD blob id against the file it patches,
        // so a modified file's section hashes the have revision (one print
        // per binary; an export is an explicit action). Failing that, an
        // all-zero id still applies fine in Auger.
        let old_sha = if *is_add {
            None
        } else {
            let tmp = std::env::temp_dir().join(format!(
                "auger-export-have-{}",
                std::process::id()
            ));
            let tmp_s = tmp.to_string_lossy().to_string();
            let spec = format!("{depot}#have");
            let sha = p4::run_raw(&conn, &["print", "-q", "-o", &tmp_s, &spec])
                .ok()
                .and_then(|_| std::fs::read(&tmp).ok())
                .map(|old| super::gitbin::blob_sha(&old));
            let _ = std::fs::remove_file(&tmp);
            Some(sha.unwrap_or_else(|| "0".repeat(40)))
        };
        patch.push_str(&super::gitbin::encode_section(
            &relative(depot, &root),
            &data,
            old_sha.as_deref(),
        ));
    }

    // Deletes and moves. A unified diff describes CONTENT, so neither fits in
    // one — git says the rest in its header lines, and that is what is written
    // here (and read back by `parse_patch`).
    //
    // A move is emitted as a rename against the file that MOVED, so the pair is
    // one section rather than a delete and an unrelated add: the apply can then
    // `p4 move` and keep the two halves one operation on the server. The other
    // half is dropped, since the rename names both ends.
    let target_set: std::collections::HashSet<&str> = targets.iter().map(String::as_str).collect();
    let mut renamed_away: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (from, to) in &moved {
        // Only when BOTH ends are in this patch. A half-move would move a file
        // the patch never says anything else about.
        if deletes.iter().any(|d| d == from) && target_set.contains(to.as_str()) {
            renamed_away.insert(from.clone());
            patch.push_str(&rename_section(&relative(from, &root), &relative(to, &root)));
            carried.push(PatchedFile {
                depot_file: to.clone(),
                action: "move".into(),
                rev: String::new(),
                binary: false,
            });
        }
    }
    for d in &deletes {
        if renamed_away.contains(d) {
            continue; // its rename section already says where it went
        }
        let body = deleted_body(conn, d);
        patch.push_str(&delete_section(&relative(d, &root), &body));
        carried.push(PatchedFile {
            depot_file: d.clone(),
            action: "delete".into(),
            rev: String::new(),
            binary: body.is_empty(),
        });
    }
    // p4 prints `--- / +++` headers even for an unchanged file, so emptiness
    // has to be judged on hunks/sections, not on the output being blank.
    if !patch.lines().any(|l| l.starts_with("@@")) && embed.is_empty() && carried.is_empty() {
        return Err("No diff to export (files may be unchanged).".into());
    }
    // Anything before the first `diff --git` is preamble, which git skips — the
    // same slot a commit message occupies in `git format-patch` output. It is
    // where the depot root goes, so the patch stays self-describing without
    // costing git compatibility.
    let preamble = format!(
        "Auger patch\n\
         auger-root: {root}\n\
         Paths are relative to that depot folder: apply from the matching\n\
         directory in a workspace or a git tree (`git apply <file>`).\n\n"
    );
    patch.insert_str(0, &preamble);
    // Nothing is left behind any more: a delete travels as a delete section and a
    // move as a rename. The slot stays because the CALLER's contract is "what
    // this patch could not carry", and a future format gap belongs here.
    Ok((patch, carried, Vec::new()))
}

/// A file that is gone: git's header lines, then the removal hunk (which may be
/// empty for a binary, where the header alone carries the meaning).
pub(crate) fn delete_section(rel: &str, body: &str) -> String {
    format!("diff --git a/{rel} b/{rel}\ndeleted file mode 100644\n--- a/{rel}\n+++ /dev/null\n{body}")
}

/// A file that moved. Both ends are named, which is what lets the apply do it as
/// one `p4 move` rather than as a delete and an unrelated add.
pub(crate) fn rename_section(from: &str, to: &str) -> String {
    format!("diff --git a/{from} b/{to}\nrename from {from}\nrename to {to}\n")
}

/// The deepest depot folder every path shares ("" when they share none).
///
/// Compared component by component: a common PREFIX is not enough, or
/// `//d/Main/Foo` and `//d/Main/Foobar` would share "//d/Main/Foo".
pub(crate) fn common_root(paths: &[&str]) -> String {
    let mut it = paths.iter().filter(|p| p.starts_with("//"));
    let Some(first) = it.next() else { return String::new() };
    let mut root: Vec<&str> = first.split('/').collect();
    root.pop(); // the file name is not part of the folder
    for p in it {
        let parts: Vec<&str> = p.split('/').collect();
        let keep = root
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count()
            .min(parts.len().saturating_sub(1));
        root.truncate(keep);
    }
    if root.len() <= 3 {
        // "//depot" and shallower: not worth stripping, and stripping the depot
        // name would make the paths ambiguous.
        return String::new();
    }
    root.join("/")
}

/// A depot path as written in the patch: relative to `root`, else the depot path
/// minus its leading slashes (which is still a valid, if deep, git path).
pub(crate) fn relative(depot: &str, root: &str) -> String {
    if !root.is_empty() {
        if let Some(rest) = depot.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
            return rest.to_string();
        }
    }
    depot.trim_start_matches('/').to_string()
}

/// `//depot/file.cpp#12` -> `//depot/file.cpp`.
fn without_rev(spec: &str) -> &str {
    match spec.rsplit_once('#') {
        Some((f, r)) if !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()) => f,
        _ => spec,
    }
}

/// p4's unified diff, rewritten into git's.
///
/// p4 names the two sides `//depot/path#rev` and the absolute local path, which
/// no git tool can resolve. Each file gets a proper `diff --git` header, paths
/// relative to `root`, and an `index` line carrying the real blob ids of the two
/// sides — which is what lets `git apply --3way` fall back to a merge instead of
/// refusing a hunk that has drifted. The hunks themselves are p4's, untouched.
fn gitify(conn: &P4Conn, diff: &str, root: &str) -> String {
    let lines: Vec<&str> = diff.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let is_header = |k: usize| {
        lines.get(k).is_some_and(|l| l.starts_with("--- "))
            && lines.get(k + 1).is_some_and(|l| l.starts_with("+++ "))
    };
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_header(i) {
            i += 1;
            continue;
        }
        let depot = without_rev(strip_tab(&lines[i][4..])).to_string();
        let local = strip_tab(&lines[i + 1][4..]).to_string();
        // Everything up to the next file header belongs to this one.
        let mut j = i + 2;
        while j < lines.len() && !is_header(j) {
            j += 1;
        }
        // p4 prints the headers even for a file that turned out to be unchanged.
        if lines[i + 2..j].iter().any(|l| l.starts_with("@@")) {
            let rel = relative(&depot, root);
            out.push_str(&format!("diff --git a/{rel} b/{rel}\n"));
            if let Some(idx) = index_line(conn, &depot, &local) {
                out.push_str(&idx);
            }
            out.push_str(&format!("--- a/{rel}\n+++ b/{rel}\n"));
            for l in &lines[i + 2..j] {
                out.push_str(l);
                out.push('\n');
            }
        }
        i = j;
    }
    out
}

/// `index <old>..<new> 100644` for a text file: the blob ids of the depot
/// revision the diff was taken against and of the file as it stands. Best
/// effort — without it a patch still applies, it just cannot be 3-way merged.
fn index_line(conn: &P4Conn, depot: &str, local: &str) -> Option<String> {
    let new = std::fs::read(local).ok().map(|b| super::gitbin::blob_sha(&b))?;
    let tmp = std::env::temp_dir().join(format!("auger-idx-{}", std::process::id()));
    let tmp_s = tmp.to_string_lossy().to_string();
    let spec = format!("{depot}#have");
    let old = p4::run_raw(conn, &["print", "-q", "-o", &tmp_s, &spec])
        .ok()
        .and_then(|_| std::fs::read(&tmp).ok())
        .map(|b| super::gitbin::blob_sha(&b));
    let _ = std::fs::remove_file(&tmp);
    Some(format!("index {}..{new} 100644\n", old?))
}

/// Everything before a tab (p4 puts a timestamp there), else the trimmed line.
fn strip_tab(s: &str) -> &str {
    match s.find('\t') {
        Some(k) => &s[..k],
        None => s.trim_end(),
    }
}

/// The removal hunk for a deleted file: every line of the revision it is being
/// deleted FROM, as `-` lines.
///
/// Written so the section is a real unified diff — `git apply` accepts it, and
/// our own applier can show what is going away — rather than a bare header. A
/// file that is binary or unreadable gets no body: the header alone still says
/// it is gone, which is the part that matters.
fn deleted_body(conn: &P4Conn, depot: &str) -> String {
    let tmp = std::env::temp_dir().join(format!("auger-del-{}", std::process::id()));
    let tmp_s = tmp.to_string_lossy().to_string();
    let spec = format!("{depot}#have");
    let text = p4::run_raw(conn, &["print", "-q", "-o", &tmp_s, &spec])
        .ok()
        .and_then(|_| std::fs::read(&tmp).ok())
        .and_then(|b| String::from_utf8(b).ok());
    let _ = std::fs::remove_file(&tmp);
    let Some(text) = text else { return String::new() };
    let body = text.strip_suffix('\n').unwrap_or(&text);
    if body.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut out = format!("@@ -1,{} +0,0 @@\n", lines.len());
    for l in lines {
        out.push('-');
        out.push_str(l);
        out.push('\n');
    }
    out
}

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
        let (patch, _, _) = build_patch(&conn, &change, files)?;
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

/// As `p4_diff_local`, but `-f` forces the diff for a file that is NOT open —
/// used for offline-modified files. Text files give a unified diff; binaries
/// give a "files differ" line. Pinned to #have: without it p4 diffs an unopened
/// file against #head, which on a workspace that is behind reports revisions
/// the user never synced as if they were local edits.
#[tauri::command]
pub async fn p4_diff_local_forced(conn: P4Conn, depot_file: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spec = format!("{depot_file}#have");
        p4::run_raw_stdout_diff(&conn, &["diff", "-f", "-du", &spec])
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

#[cfg(test)]
mod patch_format_tests {
    use super::*;
    use crate::commands::patch::{parse_patch, FileOp};

    /// No server: `p4 print` fails, so no index line is written — which is the
    /// documented degradation, and what makes these runnable offline.
    fn offline_conn() -> P4Conn {
        P4Conn {
            port: "0.0.0.0:1".into(),
            user: String::new(),
            client: String::new(),
            cwd: String::new(),
            charset: String::new(),
            ticket: String::new(),
        }
    }

    #[test]
    fn the_root_is_the_deepest_shared_folder() {
        assert_eq!(
            common_root(&["//d/Main/Games/A/x.cpp", "//d/Main/Games/A/y.cpp"]),
            "//d/Main/Games/A"
        );
        assert_eq!(
            common_root(&["//d/Main/Games/A/x.cpp", "//d/Main/Games/B/y.cpp"]),
            "//d/Main/Games"
        );
        // A shared prefix is not a shared FOLDER: comparing text would give
        // "//d/Main/Foo" here and write paths nothing can resolve.
        assert_eq!(
            common_root(&["//d/Main/Foo/x.cpp", "//d/Main/Foobar/y.cpp"]),
            "//d/Main"
        );
        // One file: its own folder, which is where `git apply` would be run.
        assert_eq!(common_root(&["//d/Main/Games/A/x.cpp"]), "//d/Main/Games/A");
        // Too shallow to strip — the depot name has to stay.
        assert_eq!(common_root(&["//d/a.txt", "//d/b.txt"]), "");
    }

    #[test]
    fn paths_are_written_against_the_root() {
        assert_eq!(relative("//d/Main/Games/A/x.cpp", "//d/Main"), "Games/A/x.cpp");
        // Outside the root (or no root at all): the full depot path, still a
        // usable git path, just a deep one.
        assert_eq!(relative("//other/x.cpp", "//d/Main"), "other/x.cpp");
        assert_eq!(relative("//d/Main/x.cpp", ""), "d/Main/x.cpp");
    }

    /// p4 names the sides `//depot/path#rev` and an absolute local path; neither
    /// means anything to a git tool.
    #[test]
    fn p4_headers_become_git_headers() {
        let p4diff = "--- //d/Main/Games/A/x.cpp#5\t2026-01-01\n\
+++ H:\\Dev\\ws\\Games\\A\\x.cpp\t2026-01-02\n\
@@ -1,2 +1,2 @@\n a\n-b\n+B\n";
        let out = gitify(&offline_conn(), p4diff, "//d/Main");
        let mut l = out.lines();
        assert_eq!(l.next().unwrap(), "diff --git a/Games/A/x.cpp b/Games/A/x.cpp");
        assert_eq!(l.next().unwrap(), "--- a/Games/A/x.cpp");
        assert_eq!(l.next().unwrap(), "+++ b/Games/A/x.cpp");
        assert_eq!(l.next().unwrap(), "@@ -1,2 +1,2 @@");
    }

    /// p4 prints the header pair even for a file that turned out to be
    /// unchanged; a section with no hunk would be a `diff --git` promising a
    /// change that isn't there.
    #[test]
    fn a_file_with_no_hunks_is_not_written() {
        let p4diff = "--- //d/Main/a.cpp#5\t0\n+++ H:\\a.cpp\t0\n\
--- //d/Main/b.cpp#5\t0\n+++ H:\\b.cpp\t0\n@@ -1 +1 @@\n-x\n+y\n";
        let out = gitify(&offline_conn(), p4diff, "//d/Main");
        assert!(!out.contains("a.cpp"), "unchanged file was written: {out}");
        assert!(out.contains("diff --git a/b.cpp b/b.cpp"));
    }

    /// The round trip that matters: what the exporter writes, the applier reads
    /// back as the same depot paths — including through the preamble's root.
    #[test]
    fn we_read_our_own_paths_back() {
        let body = "@@ -1,1 +0,0 @@\n-gone\n";
        let patch = format!(
            "Auger patch\nauger-root: //d/Main\nblurb\n\n{}{}{}",
            gitify(
                &offline_conn(),
                "--- //d/Main/Games/A/x.cpp#5\t0\n+++ H:\\x.cpp\t0\n@@ -1 +1 @@\n-a\n+b\n",
                "//d/Main",
            ),
            delete_section("Games/A/gone.txt", body),
            rename_section("Games/A/old.txt", "Games/A/new.txt"),
        );
        let files = parse_patch(&patch);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].depot, "//d/Main/Games/A/x.cpp");
        assert_eq!(files[0].op, FileOp::Change);
        assert_eq!(files[1].depot, "//d/Main/Games/A/gone.txt");
        assert_eq!(files[1].op, FileOp::Delete);
        assert_eq!(files[2].depot, "//d/Main/Games/A/new.txt");
        assert_eq!(files[2].op, FileOp::Rename("//d/Main/Games/A/old.txt".into()));
    }

    /// Without the preamble — someone else's git patch — a relative path is all
    /// there is, and it must not be mistaken for a depot path under our root.
    #[test]
    fn a_foreign_patch_keeps_its_own_paths() {
        let files = parse_patch("--- a/src/x.c\n+++ b/src/x.c\n@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(files[0].depot, "//src/x.c");
        assert_eq!(files[0].local_hint, "src/x.c");
    }

    /// The whole reason for the format: git must accept a complete patch of
    /// ours — preamble, a modified file, a deleted one and a move — with no
    /// flags, run from the directory the preamble names. Skipped where git is
    /// not installed.
    #[test]
    fn git_apply_accepts_a_whole_auger_patch() {
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !git_ok {
            return;
        }
        let dir = std::env::temp_dir().join(format!("auger-fmt-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Games/A")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git").current_dir(&dir).args(args).output().unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        // The machine's own autocrlf would rewrite the line endings on the
        // way in, which says nothing about the patch.
        git(&["config", "core.autocrlf", "false"]);
        std::fs::write(dir.join("Games/A/x.cpp"), "a\nb\n").unwrap();
        std::fs::write(dir.join("Games/A/gone.txt"), "gone\n").unwrap();
        std::fs::write(dir.join("Games/A/old.txt"), "moved\n").unwrap();

        let patch = format!(
            "Auger patch\nauger-root: //d/Main\n\n{}{}{}",
            gitify(
                &offline_conn(),
                "--- //d/Main/Games/A/x.cpp#5\t0\n+++ H:\\x.cpp\t0\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n",
                "//d/Main",
            ),
            delete_section("Games/A/gone.txt", "@@ -1 +0,0 @@\n-gone\n"),
            rename_section("Games/A/old.txt", "Games/A/new.txt"),
        );
        std::fs::write(dir.join("auger.patch"), &patch).unwrap();

        let out = git(&["apply", "auger.patch"]);
        assert!(
            out.status.success(),
            "git apply refused our patch: {}\n{patch}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read_to_string(dir.join("Games/A/x.cpp")).unwrap(), "a\nB\n");
        assert!(!dir.join("Games/A/gone.txt").exists());
        assert!(!dir.join("Games/A/old.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("Games/A/new.txt")).unwrap(), "moved\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
