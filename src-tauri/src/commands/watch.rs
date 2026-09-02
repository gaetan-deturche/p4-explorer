//! Noticing that a workspace file changed underneath a window.
//!
//! The diff and resolve windows both show a file that something else can write
//! while they are open — the editor it came from, a sync, a revert. Until now
//! they would go on showing what the file said when the window opened, and the
//! resolve window would eventually save its merge back over the newer content.
//!
//! The watch is on the file's PARENT DIRECTORY, not on the file. Editors do not
//! usually write a file in place: they write a temp file and rename it over the
//! original, and a watch bound to the original stops seeing anything the moment
//! that happens. Directory watches survive it, which is why every editor that
//! does this reliably uses one. Events are then filtered back down to the one
//! path, case-insensitively — on Windows the notification may not match the
//! spelling the caller gave.
//!
//! What is emitted is only "something touched it": the window re-reads the file
//! and compares CONTENT against what it last read or wrote, which is the only
//! comparison that cannot mistake a save of its own — or a touch that changed
//! nothing — for someone else's edit.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Watchers by window label. Dropping a watcher is what stops it, so the
/// registry is not a lookup table — it is the lifetime.
fn registry() -> &'static Mutex<HashMap<String, RecommendedWatcher>> {
    static R: OnceLock<Mutex<HashMap<String, RecommendedWatcher>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A single write arrives as several events (write, truncate, rename into
/// place, attribute change). Coalescing them keeps the window from re-reading
/// the file three times for one save.
const QUIET: Duration = Duration::from_millis(250);

/// Watch `path` for one window. Replaces whatever that window was watching, so
/// a window that reloads onto another file does not leak its old watch.
#[tauri::command]
pub async fn watch_file(app: AppHandle, label: String, path: String) -> Result<(), String> {
    let file = Path::new(&path).to_path_buf();
    let dir = file
        .parent()
        .filter(|d| d.is_dir())
        .ok_or_else(|| format!("{path} has no directory to watch"))?
        .to_path_buf();
    let want = file.to_string_lossy().to_lowercase();
    let win = label.clone();
    let last = Arc::new(Mutex::new(Instant::now() - QUIET));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !event.paths.iter().any(|p| p.to_string_lossy().to_lowercase() == want) {
            return;
        }
        {
            let mut last = last.lock().unwrap();
            if last.elapsed() < QUIET {
                return;
            }
            *last = Instant::now();
        }
        // The window may be gone; its watch is dropped on the next call rather
        // than from inside the watcher's own thread.
        if let Some(w) = app.get_webview_window(&win) {
            let _ = w.emit("file-on-disk-changed", ());
        }
    })
    .map_err(|e| format!("cannot watch {path}: {e}"))?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("cannot watch {}: {e}", dir.display()))?;
    registry().lock().unwrap().insert(label, watcher);
    Ok(())
}

/// Stop watching for one window. Idempotent: a window that never watched
/// anything (a diff of two depot revisions) closes through here too.
#[tauri::command]
pub async fn unwatch_file(label: String) -> Result<(), String> {
    registry().lock().unwrap().remove(&label);
    Ok(())
}
