//! Three-way resolve: prepare a merge job (from a `p4 resolve` conflict or a
//! rejected patch hunk), hand it to the in-app resolve window or to P4MERGE,
//! and write the settled result back.
//!
//! Jobs live in a registry keyed by id: the resolve window is a separate
//! webview, so it fetches its own data by id rather than through query params.

use crate::merge3::{merge3, Region};
use crate::p4::{self, P4Conn};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

/// A prepared merge, waiting for the window (or P4MERGE) to settle it.
pub(crate) struct MergeJob {
    /// "resolve" (a p4 resolve conflict) or "patch" (a rejected hunk).
    pub(crate) kind: String,
    /// The connection this was prepared with — the resolve window has none.
    pub(crate) conn: P4Conn,
    pub(crate) depot: String,
    /// The workspace file the result is written to.
    pub(crate) target: String,
    pub(crate) name: String,
    pub(crate) base_label: String,
    pub(crate) theirs_label: String,
    pub(crate) yours_label: String,
    pub(crate) base: Vec<String>,
    pub(crate) ours: Vec<String>,
    pub(crate) theirs: Vec<String>,
    /// Line range of `target` the result replaces. None = the whole file.
    pub(crate) splice: Option<(usize, usize)>,
    /// A `.rej` to prune the resolved hunk from, with that hunk's raw text.
    pub(crate) rej: Option<(String, String)>,
    /// What this job was prepared FROM, when it came from a patch: the patch
    /// file and the hunk. Kept so the job can be prepared again — a merge whose
    /// file changed on disk has to be rebuilt from the file, not patched up.
    pub(crate) patch_src: Option<(String, usize)>,
}

fn registry() -> &'static Mutex<HashMap<String, MergeJob>> {
    static R: OnceLock<Mutex<HashMap<String, MergeJob>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register(job: MergeJob) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = format!("m{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    registry().lock().unwrap().insert(id.clone(), job);
    id
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeData {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub target: String,
    pub base_label: String,
    pub theirs_label: String,
    pub yours_label: String,
    pub regions: Vec<Region>,
    pub conflicts: usize,
}

/// Prepare `id` again from the file as it stands now, keeping the same id so
/// the window that holds it can simply re-read its data.
///
/// This is what "reload" means for a merge: the workspace file is one of the
/// three inputs, so a change to it invalidates the whole comparison, not just
/// one pane. Whatever was settled in the window is lost — which is why nothing
/// here happens without the user asking for it.
#[tauri::command]
pub async fn merge_reload(id: String) -> Result<(), String> {
    let (kind, conn, depot, src) = {
        let reg = registry().lock().unwrap();
        let job = reg.get(&id).ok_or("this merge is no longer available")?;
        (job.kind.clone(), job.conn.clone(), job.depot.clone(), job.patch_src.clone())
    };
    let fresh = tauri::async_runtime::spawn_blocking(move || match (kind.as_str(), src) {
        ("patch", Some((path, hunk))) => {
            super::patch::prepare_patch_merge(&conn, &path, &depot, hunk)
        }
        ("patch", None) => Err("this merge cannot be rebuilt: its patch is not recorded".into()),
        _ => prepare_resolve_merge(&conn, &depot),
    })
    .await
    .map_err(|e| format!("merge-reload task failed: {e}"))??;

    // Move the fresh job onto the id the window already holds, so nothing has to
    // be re-navigated; the temporary id is dropped with it.
    let mut reg = registry().lock().unwrap();
    let job = reg.remove(&fresh).ok_or("the rebuilt merge went missing")?;
    reg.insert(id, job);
    Ok(())
}

/// The prepared merge for `id` — what the resolve window renders.
#[tauri::command]
pub async fn merge_data(id: String) -> Result<MergeData, String> {
    let reg = registry().lock().unwrap();
    let job = reg.get(&id).ok_or("this merge is no longer available")?;
    let regions = merge3(&job.base, &job.ours, &job.theirs);
    Ok(MergeData {
        id: id.clone(),
        kind: job.kind.clone(),
        name: job.name.clone(),
        target: job.target.clone(),
        base_label: job.base_label.clone(),
        theirs_label: job.theirs_label.clone(),
        yours_label: job.yours_label.clone(),
        conflicts: regions.iter().filter(|r| r.is_conflict()).count(),
        regions,
    })
}

/// Files in `path` that p4 says still need resolving.
#[tauri::command]
pub async fn resolve_needed(conn: P4Conn, path: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spec = if path.is_empty() { "//...".to_string() } else { path };
        let recs = p4::run(&conn, &["fstat", "-Ru", &spec]).unwrap_or_default();
        Ok(recs
            .iter()
            .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(String::from))
            .collect())
    })
    .await
    .map_err(|e| format!("resolve-needed task failed: {e}"))?
}

/// Prepare a merge for a file that `p4 resolve` is waiting on: base and theirs
/// come from the depot, yours is the workspace file as it stands.
#[tauri::command]
pub async fn merge_start_resolve(conn: P4Conn, depot_file: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || prepare_resolve_merge(&conn, &depot_file))
        .await
        .map_err(|e| format!("merge-start task failed: {e}"))?
}

pub(crate) fn prepare_resolve_merge(conn: &P4Conn, depot_file: &str) -> Result<String, String> {
    let recs = p4::run(conn, &["fstat", "-Ru", "-Or", depot_file])?;
    let rec = recs.first().ok_or("p4 reports nothing to resolve for this file")?;
    let client = rec
        .get("clientFile")
        .and_then(|v| v.as_str())
        .ok_or("this file is not in the current workspace")?
        .to_string();
    let subs = p4::explode_indexed(rec, "resolveBaseFile");
    let sub = subs.first().ok_or("p4 gave no resolve record for this file")?;
    let get = |k: &str| sub.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (bf, br) = (get("resolveBaseFile"), get("resolveBaseRev"));
    let (ff, fr) = (get("resolveFromFile"), get("resolveEndFromRev"));
    // The REVISIONS matter as much as the paths: an empty one builds the spec
    // "//depot/file#", which p4 rejects outright — but a merge must never be
    // attempted against a revision we could not name.
    if bf.is_empty() || ff.is_empty() || br.is_empty() || fr.is_empty() {
        return Err("this file needs a resolve p4 cannot describe (binary or branch)".into());
    }

    let name = depot_file.rsplit('/').next().unwrap_or("file").to_string();
    let base = print_lines(conn, &format!("{bf}#{br}"), &format!("base_{name}"))?;
    let theirs = print_lines(conn, &format!("{ff}#{fr}"), &format!("theirs_{name}"))?;
    let ours = read_lines(&client)?;

    Ok(register(MergeJob {
        kind: "resolve".into(),
        conn: conn.clone(),
        depot: depot_file.to_string(),
        target: client,
        name,
        base_label: format!("base #{br}"),
        theirs_label: format!("depot #{fr}"),
        yours_label: "workspace".into(),
        base,
        ours,
        theirs,
        splice: None,
        rej: None,
        patch_src: None,
    }))
}

/// Write the settled text back. For a resolve that also marks the file resolved
/// (`p4 resolve -ay` — accept the file as it now stands); for a patch hunk it
/// splices the result in and prunes the hunk from the `.rej`.
#[tauri::command]
pub async fn merge_save(app: AppHandle, id: String, text: String) -> Result<String, String> {
    let out = tauri::async_runtime::spawn_blocking(move || merge_save_inner(&id, &text))
        .await
        .map_err(|e| format!("merge-save task failed: {e}"))?;
    if out.is_ok() {
        // Let the main window refresh: it has no idea this window saved.
        use tauri::Emitter;
        let _ = app.emit("merge-done", ());
    }
    out
}

/// Drop this merge without writing anything (the window was closed).
#[tauri::command]
pub async fn merge_cancel(id: String) -> Result<(), String> {
    registry().lock().unwrap().remove(&id);
    Ok(())
}

/// Hand the job to P4MERGE (`p4merge base theirs yours merged`), wait for it to
/// close, and take the merged file if the tool wrote one.
#[tauri::command]
pub async fn merge_external(id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = {
            let reg = registry().lock().unwrap();
            reg.get(&id).ok_or("this merge is no longer available")?.conn.clone()
        };
        let cmdline = p4_set_var(&conn, "P4MERGE").ok_or(
            "No external merge tool configured. Set P4MERGE (e.g. `p4 set P4MERGE=p4merge`).",
        )?;
        let (name, base, theirs, ours) = {
            let reg = registry().lock().unwrap();
            let job = reg.get(&id).ok_or("this merge is no longer available")?;
            (job.name.clone(), job.base.clone(), job.theirs.clone(), job.ours.clone())
        };
        let tmp = std::env::temp_dir();
        let bp = tmp.join(format!("p4gui_{id}_base_{name}"));
        let tp = tmp.join(format!("p4gui_{id}_theirs_{name}"));
        let yp = tmp.join(format!("p4gui_{id}_yours_{name}"));
        let mp = tmp.join(format!("p4gui_{id}_merged_{name}"));
        write_lines(&bp, &base)?;
        write_lines(&tp, &theirs)?;
        write_lines(&yp, &ours)?;
        write_lines(&mp, &ours)?; // seed the result so a plain save is a no-op

        let mut c = std::process::Command::new("cmd");
        c.arg("/c");
        for part in cmdline.split_whitespace() {
            c.arg(part);
        }
        c.arg(&bp).arg(&tp).arg(&yp).arg(&mp);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        let status = c.status().map_err(|e| format!("failed to launch P4MERGE: {e}"))?;
        if !status.success() {
            // P4Merge itself exits non-zero when closed without saving.
            return Ok("cancelled".to_string());
        }
        let merged = std::fs::read_to_string(&mp).map_err(|e| e.to_string())?;
        // Wrappers around editors (`code --wait`) exit 0 either way, and some
        // seed the result with the base — so a result still sitting at the base
        // means no decision was made, not "take the base".
        let merged_lines = super::patch::split_lines(&merged);
        if merged_lines == base && base != ours && base != theirs {
            return Ok("unchanged".to_string());
        }
        merge_save_inner(&id, &merged)
    })
    .await
    .map_err(|e| format!("merge-external task failed: {e}"))?
}

/// Open the in-app resolve window on the `/merge` route for job `id`.
#[tauri::command]
pub async fn open_merge_window(app: AppHandle, id: String, name: String) -> Result<(), String> {
    let label = format!("merge-{id}");
    let url = format!("merge?id={id}");
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(format!("Resolve — {name}"))
        .inner_size(1400.0, 880.0)
        .min_inner_size(800.0, 480.0)
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open the resolve window: {e}"))?;
    crate::wingeom::apply(&win, "merge");
    Ok(())
}

/// The write-back half of `merge_save`, shared with the external-tool path.
pub(crate) fn merge_save_inner(id: &str, text: &str) -> Result<String, String> {
    let (target, kind, depot, splice, rej, conn) = {
        let reg = registry().lock().unwrap();
        let job = reg.get(id).ok_or("this merge is no longer available")?;
        (
            job.target.clone(),
            job.kind.clone(),
            job.depot.clone(),
            job.splice,
            job.rej.clone(),
            job.conn.clone(),
        )
    };
    write_result(&target, text, splice)?;
    if kind == "resolve" {
        p4::run(&conn, &["resolve", "-ay", &depot])
            .map_err(|e| format!("the merge was written but p4 resolve failed: {e}"))?;
    }
    if let Some((path, hunk)) = rej {
        prune_rej(&path, &hunk);
    }
    registry().lock().unwrap().remove(id);
    Ok(if kind == "resolve" { "resolved".to_string() } else { "written".to_string() })
}

/// Write `text` over the whole file, or over just `splice` of it, keeping the
/// target's own EOL style, BOM and trailing-newline state.
fn write_result(target: &str, text: &str, splice: Option<(usize, usize)>) -> Result<(), String> {
    let raw = std::fs::read(target).map_err(|e| format!("cannot read {target}: {e}"))?;
    let (bom, body) = super::patch::split_bom(&raw);
    let body = String::from_utf8(body.to_vec()).map_err(|_| "the target is not UTF-8")?;
    let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_with_eol = body.ends_with('\n');

    let new_lines: Vec<String> = super::patch::split_lines(text);
    let out_lines: Vec<String> = match splice {
        None => new_lines,
        Some((from, to)) => {
            let mut lines = super::patch::split_lines(&body);
            let from = from.min(lines.len());
            let to = to.clamp(from, lines.len());
            lines.splice(from..to, new_lines);
            lines
        }
    };

    super::patch::make_writable(target).map_err(|e| format!("cannot make {target} writable: {e}"))?;
    let mut out = bom.to_vec();
    let mut joined = out_lines.join(eol);
    if ends_with_eol {
        joined.push_str(eol);
    }
    out.extend_from_slice(joined.as_bytes());
    std::fs::write(target, &out).map_err(|e| format!("cannot write {target}: {e}"))
}

/// Remove one resolved hunk from a `.rej`, deleting the file once it is empty.
fn prune_rej(path: &str, hunk: &str) {
    let Ok(body) = std::fs::read_to_string(path) else { return };
    let left = body.replace(hunk, "");
    if left.lines().any(|l| l.starts_with("@@")) {
        let _ = std::fs::write(path, left);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Print one revision and read it back.
///
/// Two properties matter here, because getting them wrong silently corrupts a
/// merge: `p4 print` EXITS 0 while writing nothing when it cannot produce the
/// revision (verified: `print -o tmp //f#999` prints "no file(s) at that
/// revision" and returns 0, leaving the target file untouched), and the temp
/// path used to be fixed per file name. Together those meant a leftover file
/// from an earlier resolve could become a side of this merge — and a stale
/// "theirs" that matches the base makes the merge keep OURS, which quietly
/// reverts the depot change on save.
///
/// So: a fresh unique path per call, deleted before AND after, and the file must
/// actually exist afterwards.
fn print_lines(conn: &P4Conn, spec: &str, tag: &str) -> Result<Vec<String>, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("p4gui_{}_{n}_{tag}", std::process::id()));
    let path = tmp.to_str().ok_or("bad temp path")?.to_string();
    let _ = std::fs::remove_file(&tmp); // never read a previous run's bytes
    p4::run_raw(conn, &["print", "-q", "-o", &path, spec])?;
    if !tmp.is_file() {
        return Err(format!(
            "p4 produced nothing for {spec} — refusing to merge against unknown content"
        ));
    }
    let lines = read_lines(&path);
    let _ = std::fs::remove_file(&tmp); // and leave nothing to be reused
    lines
}

fn read_lines(path: &str) -> Result<Vec<String>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let (_, body) = super::patch::split_bom(&raw);
    let text = String::from_utf8(body.to_vec())
        .map_err(|_| format!("{path} is not a UTF-8 text file — resolve it in P4Merge instead"))?;
    Ok(super::patch::split_lines(&text))
}

fn write_lines(path: &std::path::Path, lines: &[String]) -> Result<(), String> {
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// A `p4 set` variable's value, with p4's trailing origin tag stripped.
fn p4_set_var(conn: &P4Conn, key: &str) -> Option<String> {
    let out = p4::run_raw(conn, &["set", key]).ok()?;
    let line = out.lines().next()?;
    let v = line.strip_prefix(&format!("{key}="))?.trim();
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

#[cfg(test)]
mod resolve_tests {
    use crate::p4::explode_indexed;

    /// A REAL `p4 -ztag fstat -Ru -Or` record for a file that needs a content
    /// resolve after a sync (captured live: base #30, incoming #31). The three
    /// sides of the merge come from these fields, so a parsing slip here is a
    /// silent wrong merge.
    fn record() -> crate::p4::Record {
        let pairs = [
            ("depotFile", "//d/AI/CYAISubsystem.cpp"),
            ("clientFile", "H:\\ws\\AI\\CYAISubsystem.cpp"),
            ("headRev", "31"),
            ("haveRev", "31"),
            ("action", "edit"),
            ("workRev", "31"),
            ("unresolved", ""),
            ("resolveAction0", "unresolved"),
            ("resolveBaseFile0", "//d/AI/CYAISubsystem.cpp"),
            ("resolveBaseRev0", "30"),
            ("resolveFromFile0", "//d/AI/CYAISubsystem.cpp"),
            ("resolveStartFromRev0", "31"),
            ("resolveEndFromRev0", "31"),
            ("resolveType0", "content"),
        ];
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn the_resolve_row_names_base_and_incoming() {
        let rows = explode_indexed(&record(), "resolveBaseFile");
        assert_eq!(rows.len(), 1, "one pending resolve => one row");
        let get = |k: &str| rows[0].get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        assert_eq!(get("resolveBaseRev"), "30", "base is the revision we edited from");
        assert_eq!(get("resolveEndFromRev"), "31", "theirs is the new depot revision");
        assert_eq!(get("resolveBaseFile"), "//d/AI/CYAISubsystem.cpp");
        assert_eq!(get("resolveFromFile"), "//d/AI/CYAISubsystem.cpp");
        // The header fields must survive onto the row: the workspace file is read
        // from clientFile.
        assert!(get("clientFile").ends_with("CYAISubsystem.cpp"));
    }

    #[test]
    fn base_and_incoming_are_different_revisions() {
        // The bug that reverted a colleague's change looked exactly like this:
        // both sides resolving to the same content. They are different REVISIONS
        // here, so any equality downstream can only come from a bad fetch.
        let rows = explode_indexed(&record(), "resolveBaseFile");
        let get = |k: &str| rows[0].get(k).and_then(|v| v.as_str()).unwrap_or("");
        assert_ne!(get("resolveBaseRev"), get("resolveEndFromRev"));
    }
}
