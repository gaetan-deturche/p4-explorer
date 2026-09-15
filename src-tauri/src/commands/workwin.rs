//! A second app window, on another workspace.
//!
//! Nothing the feature stores hold is shared between webviews — their module
//! state is per-window — so a window on the main route already IS an
//! independent workspace as soon as it knows which one to open. That is all
//! this does: name the window after the workspace and put the answer in its URL.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::diffwin::enc;

/// One workspace window, as the session remembers it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkWindow {
    pub port: String,
    pub client: String,
}

/// What each workspace window is showing, and when it closed.
///
/// The close TIME is the whole trick. Tauri reports `ExitRequested` only once the
/// windows are already destroyed, so at that moment a quit and a window the user
/// closed an hour ago look the same. A shutdown closes every window within
/// milliseconds of the exit, so an entry closed that recently was closed BY the
/// shutdown and belongs to the session; anything older was closed on purpose and
/// does not.
struct Tracked {
    what: WorkWindow,
    closed: Option<Instant>,
}

fn tracked() -> &'static Mutex<HashMap<String, Tracked>> {
    static T: OnceLock<Mutex<HashMap<String, Tracked>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How recently a window must have closed to count as closed BY the shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// This window is showing `client` on `port` — called when a workspace window
/// opens, and again whenever the workspace it shows changes (an empty window
/// being pointed somewhere, or a switch).
#[tauri::command]
pub async fn note_workspace_window(window: tauri::Window, port: String, client: String) {
    if client.is_empty() {
        return;
    }
    tracked().lock().unwrap().insert(
        window.label().to_string(),
        Tracked { what: WorkWindow { port, client }, closed: None },
    );
}

/// A workspace window has gone. Remembered, not forgotten: whether it counts is
/// decided at exit, by how long ago this was.
pub fn note_closed(label: &str) {
    if let Some(e) = tracked().lock().unwrap().get_mut(label) {
        e.closed = Some(Instant::now());
    }
}

/// The workspace windows to restore: those still open, plus those the shutdown
/// has just closed.
pub fn session() -> Vec<WorkWindow> {
    let now = Instant::now();
    tracked()
        .lock()
        .unwrap()
        .values()
        .filter(|e| match e.closed {
            None => true,
            Some(at) => now.duration_since(at) < SHUTDOWN_GRACE,
        })
        .map(|e| e.what.clone())
        .collect()
}

/// What the last session had open. Empty when there is nothing to restore.
#[tauri::command]
pub async fn workspace_session(app: AppHandle) -> Vec<WorkWindow> {
    let Some(state) = app.try_state::<crate::index::AppState>() else { return Vec::new() };
    let Ok(db) = state.cache_db.lock() else { return Vec::new() };
    let json: String = db
        .query_row(
            "SELECT json FROM cache WHERE scope='nav' AND key='workwindows'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    serde_json::from_str(&json).unwrap_or_default()
}

/// Write the session down. Called as the app exits.
pub fn save_session(app: &AppHandle) {
    let open = session();
    let Some(state) = app.try_state::<crate::index::AppState>() else { return };
    let Ok(db) = state.cache_db.lock() else { return };
    let Ok(json) = serde_json::to_string(&open) else { return };
    let _ = db.execute(
        "INSERT INTO cache(scope, key, json) VALUES('nav', 'workwindows', ?1)
         ON CONFLICT(scope, key) DO UPDATE SET json=excluded.json",
        [json],
    );
}

/// Tauri labels take only alphanumerics, `-`, `/`, `:` and `_`, and workspace
/// names here carry dots (`gaetan.deturche_sloclap-41_Curiosity2`).
fn sanitize(client: &str) -> String {
    client
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// One window per workspace, labelled by it, so asking twice raises the window
/// already showing that workspace instead of opening a rival copy — two windows
/// on one workspace would each run their own offline scan over the same files.
///
/// An empty `client` is the "new window" case: it is asked for precisely so a
/// DIFFERENT workspace can be picked in it, so there is nothing yet to name it
/// after and it takes a counted label.
fn label_for(app: &AppHandle, client: &str) -> String {
    if !client.is_empty() {
        return format!("work-{}", sanitize(client));
    }
    for n in 2.. {
        let label = format!("work-new{n}");
        if app.get_webview_window(&label).is_none() {
            return label;
        }
    }
    unreachable!()
}

/// Open (or re-focus) a workspace window. `client` empty = an empty window for
/// the user to pick in.
#[tauri::command]
pub async fn open_workspace_window(
    app: AppHandle,
    port: String,
    client: String,
) -> Result<(), String> {
    let label = label_for(&app, &client);
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    // `new=1` says "don't restore the remembered workspace" — an empty window
    // exists to be pointed somewhere else.
    let url = if client.is_empty() {
        format!("?new=1&port={}", enc(&port))
    } else {
        format!("?port={}&client={}", enc(&port), enc(&client))
    };
    let title = if client.is_empty() {
        "Auger".to_string()
    } else {
        format!("Auger — {client}")
    };
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(1280.0, 780.0)
        .min_inner_size(900.0, 500.0)
        // Same as the main window: Tauri's OS drag-drop handler otherwise
        // swallows the in-webview HTML5 drag that moves a file between
        // changelists.
        .disable_drag_drop_handler()
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open the workspace window: {e}"))?;
    if !client.is_empty() {
        tracked().lock().unwrap().insert(
            label.clone(),
            Tracked { what: WorkWindow { port: port.clone(), client: client.clone() }, closed: None },
        );
    }
    // Geometry PER WORKSPACE, unlike the diff and resolve windows. Theirs are
    // interchangeable and their labels are unique per window, so per-label state
    // would never match again; a workspace window's label is the workspace, so it
    // comes back where that workspace was left.
    crate::wingeom::apply(&win, &label);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn a_workspace_name_becomes_a_usable_label() {
        // Dots are the reason this exists: Tauri rejects a label containing one,
        // and every workspace name on this server starts with `user.name_`.
        assert_eq!(
            sanitize("gaetan.deturche_sloclap-41_Curiosity2"),
            "gaetan_deturche_sloclap-41_Curiosity2"
        );
        assert_eq!(sanitize("ws with spaces"), "ws_with_spaces");
    }
}
