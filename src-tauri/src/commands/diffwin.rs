//! In-app diff window: materialize the two sides of a diff to files on disk
//! (`p4 print` for server revisions; the live workspace file for "local") and
//! open a separate webview window on the `/diff` route that renders them
//! side-by-side. The window reads the files back via `read_text_file`.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use crate::p4::{self, P4Conn};

/// The two sides of a diff, ready to open (paths on disk + display labels).
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiffPair {
    pub left: String,
    pub right: String,
    pub left_label: String,
    pub right_label: String,
    pub title: String,
}

fn base_name(depot_file: &str) -> &str {
    depot_file.rsplit('/').next().unwrap_or("file")
}

/// Print `spec` to a temp file named for the diff window; empty `spec` creates
/// an EMPTY temp file (the left side of an added file's diff).
fn print_side(conn: &P4Conn, spec: &str, file_tag: &str) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!("p4gui_diff_{file_tag}"));
    let tmp_s = tmp.to_str().ok_or("bad temp path")?.to_string();
    if spec.is_empty() {
        std::fs::write(&tmp, "").map_err(|e| e.to_string())?;
        return Ok(tmp_s);
    }
    p4::run_raw(conn, &["print", "-q", "-o", &tmp_s, spec])?;
    if !tmp.is_file() {
        return Err(format!("p4 print produced no file for {spec}"));
    }
    Ok(tmp_s)
}

fn sane(s: &str) -> String {
    s.replace(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '_', "_")
}

/// A revision vs its predecessor (history/changelist details). Rev 1 (added)
/// diffs against an empty left side.
#[tauri::command]
pub async fn diff_pair_rev(conn: P4Conn, depot_file: String, rev: i64) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = base_name(&depot_file).to_string();
        let prev = if rev > 1 { format!("{depot_file}#{}", rev - 1) } else { String::new() };
        let left = print_side(&conn, &prev, &sane(&format!("{}_{}", rev - 1, name)))?;
        let right = print_side(&conn, &format!("{depot_file}#{rev}"), &sane(&format!("{rev}_{name}")))?;
        Ok(DiffPair {
            left,
            right,
            left_label: if rev > 1 { format!("{name}#{}", rev - 1) } else { format!("{name} (added)") },
            right_label: format!("{name}#{rev}"),
            title: format!("{name}  #{}{rev}", if rev > 1 { format!("{} → #", rev - 1) } else { String::new() }),
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// A shelved file vs its base revision.
#[tauri::command]
pub async fn diff_pair_shelved(
    conn: P4Conn,
    depot_file: String,
    rev: i64,
    change: String,
) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = base_name(&depot_file).to_string();
        let base = if rev >= 1 { format!("{depot_file}#{rev}") } else { String::new() };
        let left = print_side(&conn, &base, &sane(&format!("base{rev}_{name}")))?;
        let right = print_side(&conn, &format!("{depot_file}@={change}"), &sane(&format!("shelf{change}_{name}")))?;
        Ok(DiffPair {
            left,
            right,
            left_label: if rev >= 1 { format!("{name}#{rev}") } else { format!("{name} (added)") },
            right_label: format!("{name} (shelved @{change})"),
            title: format!("{name}  shelf @{change}"),
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// The live workspace file vs the synced (have) revision.
#[tauri::command]
pub async fn diff_pair_local(conn: P4Conn, depot_file: String) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let recs = p4::run(&conn, &["fstat", "-T", "clientFile,haveRev", &depot_file])?;
        let first = recs.first().ok_or("file is not in this workspace")?;
        let local = first
            .get("clientFile")
            .and_then(|v| v.as_str())
            .ok_or("file is not in this workspace")?
            .to_string();
        let have = first.get("haveRev").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name = base_name(&depot_file).to_string();
        // No have revision (add in progress) → empty left side.
        let spec = if have.is_empty() { String::new() } else { format!("{depot_file}#have") };
        let left = print_side(&conn, &spec, &sane(&format!("have_{name}")))?;
        Ok(DiffPair {
            left,
            right: local,
            left_label: if have.is_empty() { format!("{name} (new)") } else { format!("{name}#{have}") },
            right_label: format!("{name} (workspace)"),
            title: format!("{name}  local changes"),
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// Read a text file for the diff window (lossy UTF-8; size-capped so a huge or
/// binary file can't hang the renderer).
#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        const CAP: u64 = 16 * 1024 * 1024; // 16 MB
        let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if meta.len() > CAP {
            return Err(format!("File too large to diff in-app ({} MB).", meta.len() / (1024 * 1024)));
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
    .await
    .map_err(|e| format!("read task failed: {e}"))?
}

/// Minimal percent-encoding for query-string values (RFC 3986 unreserved kept).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Open the in-app diff window on the `/diff` route. Each call gets its own
/// window (unique label), like an external diff tool would.
#[tauri::command]
pub async fn open_diff_window(app: AppHandle, pair: DiffPair) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("diff-{n}");
    let url = format!(
        "diff?left={}&right={}&ll={}&rl={}&title={}",
        enc(&pair.left),
        enc(&pair.right),
        enc(&pair.left_label),
        enc(&pair.right_label),
        enc(&pair.title),
    );
    let title = format!("Diff — {}", pair.title);
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(&title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(700.0, 400.0)
        .build()
        .map_err(|e| format!("failed to open diff window: {e}"))?;
    Ok(())
}
