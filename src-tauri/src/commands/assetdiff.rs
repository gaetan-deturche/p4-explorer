//! The in-app asset diff: two revisions of a .uasset / .umap compared object by
//! object and property by property, read straight from the files (no editor).

use serde::Serialize;
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use super::diffwin::{enc, DiffPair};
use crate::p4::P4Conn;
use crate::uasset::archive::Val;
use crate::uasset::diff::{diff, Row, Status};
use crate::uasset::doc::{document, main_class, thumbnail_url, ALL_DATA};
use crate::uasset::package::Package;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetDiff {
    rows: Vec<Row>,
    /// Rows that differ, leaves only, outside "All data"...
    changes: usize,
    /// ...and inside it, when a summary leads.
    raw_changes: usize,
    /// Why a side could not be read; empty when it was (or is absent).
    left_error: String,
    right_error: String,
    left_thumb: Option<String>,
    right_thumb: Option<String>,
    class: String,
}

struct Side {
    doc: Val,
    error: String,
    thumb: Option<String>,
    class: String,
}

/// An absent side (an add, a delete) is an empty document, not an error.
fn read_side(path: &str) -> Side {
    let empty = |error: String| Side { doc: Val::Map(Vec::new()), error, thumb: None, class: String::new() };
    if path.is_empty() || std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true) {
        return empty(String::new());
    }
    match Package::open(std::path::Path::new(path)) {
        Ok(pkg) => Side { doc: document(&pkg), error: String::new(), thumb: thumbnail_url(&pkg), class: main_class(&pkg) },
        Err(e) => empty(e),
    }
}

/// Compare two package files on disk.
#[tauri::command]
pub async fn asset_diff(left: String, right: String) -> Result<AssetDiff, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (l, r) = std::thread::scope(|s| {
            let l = s.spawn(|| read_side(&left));
            let r = read_side(&right);
            (l.join().unwrap_or_else(|_| read_side("")), r)
        });
        let rows = diff(&l.doc, &r.doc);
        let (mut changes, mut raw_changes, mut in_raw) = (0, 0, false);
        for row in &rows {
            if row.depth == 0 {
                in_raw = row.key == ALL_DATA;
            }
            if !row.kids && row.status != Status::Same {
                if in_raw {
                    raw_changes += 1;
                } else {
                    changes += 1;
                }
            }
        }
        AssetDiff {
            rows,
            changes,
            raw_changes,
            left_error: l.error,
            right_error: r.error,
            left_thumb: l.thumb,
            right_thumb: r.thumb,
            class: if r.class.is_empty() { l.class } else { r.class },
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// What an asset diff window needs to hand the same pair to Unreal.
#[derive(Serialize, Clone)]
pub struct AssetJob {
    pub conn: P4Conn,
    pub name: String,
}

fn asset_jobs() -> &'static std::sync::Mutex<std::collections::HashMap<String, AssetJob>> {
    static R: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, AssetJob>>> =
        std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[tauri::command]
pub fn asset_diff_job(job: String) -> Result<AssetJob, String> {
    asset_jobs()
        .lock()
        .map_err(|_| "window registry poisoned")?
        .get(&job)
        .cloned()
        .ok_or_else(|| format!("no asset diff job named {job}"))
}

/// Open the asset diff window on the `/assetdiff` route, one window per call.
#[tauri::command]
pub async fn open_asset_diff_window(app: AppHandle, pair: DiffPair, conn: P4Conn, name: String) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let label = format!("assetdiff-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    asset_jobs()
        .lock()
        .map_err(|_| "window registry poisoned")?
        .insert(label.clone(), AssetJob { conn, name });
    let url = format!(
        "assetdiff?left={}&right={}&ll={}&rl={}&title={}&job={}",
        enc(&pair.left),
        enc(&pair.right),
        enc(&pair.left_label),
        enc(&pair.right_label),
        enc(&pair.title),
        enc(&label),
    );
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(format!("Asset diff — {}", pair.title))
        .inner_size(1280.0, 800.0)
        .min_inner_size(700.0, 400.0)
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open asset diff window: {e}"))?;
    crate::wingeom::apply(&win, "assetdiff");
    Ok(())
}
