//! The file-history window: one file's revisions, opened from anywhere a file
//! is shown (pending, shelved, offline, changelist details, reviews, the tree).
//!
//! It is a window rather than a center-pane mode because a file's history is
//! consulted WHILE working on something else — the pending list you are about to
//! submit, the review you are reading — and taking the center pane away to show
//! it loses exactly the context that prompted the question.
//!
//! Like the resolve window, the job is registered here and fetched by id: the
//! child webview needs the connection to run its own `p4` commands, and a
//! `P4Conn` carries a ticket, which has no business in a window URL.

use crate::p4::P4Conn;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// What a file-history window needs to do its own work.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistJob {
    pub conn: P4Conn,
    pub depot_file: String,
    /// Revision spec for a blame window ("#8", "@=1234", "" = head). The history
    /// window ignores it — it lists every revision anyway.
    pub rev_spec: String,
}

fn registry() -> &'static Mutex<HashMap<String, HistJob>> {
    static R: OnceLock<Mutex<HashMap<String, HistJob>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Open (or re-focus) the history window for one depot file.
///
/// One window per file: asking twice for the same file means "show me that
/// again", not "give me a second copy of it" — and the label makes that free.
#[tauri::command]
pub async fn open_file_history_window(
    app: AppHandle,
    conn: P4Conn,
    depot_file: String,
) -> Result<(), String> {
    let name = depot_file.rsplit('/').next().unwrap_or(&depot_file).to_string();
    let id = format!("h{}", slug(&depot_file));
    let label = format!("filehist-{id}");
    registry().lock().unwrap().insert(
        id.clone(),
        HistJob { conn, depot_file: depot_file.clone(), rev_spec: String::new() },
    );

    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(format!("filehist?id={id}").into()))
        .title(format!("History — {name}"))
        .inner_size(1100.0, 700.0)
        .min_inner_size(600.0, 300.0)
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open the history window: {e}"))?;
    // One geometry for every history window, as for diffs: their labels are
    // per-file, so per-window state could never be restored.
    crate::wingeom::apply(&win, "filehist");
    Ok(())
}

/// Open (or re-focus) the blame window for a file, at `rev_spec` ("" = head).
///
/// A sibling of the diff window rather than a mode of it: blame has one text and
/// nothing editable, so it shares the chrome, the syntax highlighting and the
/// geometry, and none of the diff's block/caret/edit model.
#[tauri::command]
pub async fn open_blame_window(
    app: AppHandle,
    conn: P4Conn,
    depot_file: String,
    rev_spec: String,
) -> Result<(), String> {
    let name = depot_file.rsplit('/').next().unwrap_or(&depot_file).to_string();
    // Keyed by file AND revision, apart from the history window. Both windows can
    // be open on one file, and "blame before this change" must open the EARLIER
    // revision rather than re-focus the one it was launched from — which also
    // lets the two be compared side by side.
    let id = format!("b{}{}", slug(&depot_file), slug(&rev_spec));
    let label = format!("blame-{id}");
    registry().lock().unwrap().insert(
        id.clone(),
        HistJob { conn, depot_file: depot_file.clone(), rev_spec: rev_spec.clone() },
    );

    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(format!("blame?id={id}").into()))
        .title(format!("Blame — {name}{rev_spec}"))
        .inner_size(1100.0, 760.0)
        .min_inner_size(600.0, 300.0)
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open the blame window: {e}"))?;
    crate::wingeom::apply(&win, "blame");
    Ok(())
}

/// The job a history window was opened with.
#[tauri::command]
pub async fn file_history_job(id: String) -> Result<HistJob, String> {
    registry()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| "this history window is no longer available".to_string())
}

/// A window label safe for a depot path: Tauri labels allow only alphanumerics,
/// `-`, `/`, `:` and `_`, so everything else becomes `_`.
fn slug(depot_file: &str) -> String {
    depot_file
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn a_depot_path_becomes_a_legal_label() {
        let s = slug("//depot/main/Config/DefaultEngine.ini");
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        // Distinct files must not collide into one window.
        assert_ne!(slug("//d/a/f.ini"), slug("//d/b/f.ini"));
    }
}
