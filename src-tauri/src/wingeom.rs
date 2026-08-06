//! Remembering where windows were, across restarts.
//!
//! Keyed by window KIND, not by label: diff/resolve labels are unique per window
//! (`diff-1`, `diff-2`, …), so per-label state would never match on the next run
//! and would accumulate an entry per diff ever opened. All diff windows share
//! one remembered geometry, all resolve windows another, and the main window has
//! its own — which is the behaviour anyone actually wants from interchangeable
//! transient windows.
//!
//! Restores are validated against the CURRENT monitors: a window remembered on a
//! second screen that is now unplugged would otherwise come back invisible.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{LogicalPosition, LogicalSize, Manager, WebviewWindow, WindowEvent};

use crate::index::AppState;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
struct Geom {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    maximized: bool,
}

/// Last non-maximized geometry per kind: a maximized window reports the
/// maximized rect, so the size to restore has to be remembered from before.
static LAST: Mutex<Option<HashMap<String, Geom>>> = Mutex::new(None);
/// Throttle: move/resize fire continuously while dragging.
static WROTE_AT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn with_map<T>(m: &Mutex<Option<HashMap<String, T>>>, f: impl FnOnce(&mut HashMap<String, T>)) {
    let mut guard = m.lock().unwrap();
    f(guard.get_or_insert_with(HashMap::new));
}

fn load(win: &WebviewWindow, kind: &str) -> Option<Geom> {
    let state = win.app_handle().try_state::<AppState>()?;
    let db = state.cache_db.lock().ok()?;
    let json: String = db
        .query_row(
            "SELECT json FROM cache WHERE scope='wingeom' AND key=?1",
            [kind],
            |r| r.get(0),
        )
        .ok()?;
    serde_json::from_str(&json).ok()
}

fn store(win: &WebviewWindow, kind: &str, g: Geom) {
    let Some(state) = win.app_handle().try_state::<AppState>() else { return };
    let Ok(db) = state.cache_db.lock() else { return };
    let Ok(json) = serde_json::to_string(&g) else { return };
    let _ = db.execute(
        "INSERT INTO cache(scope, key, json) VALUES('wingeom', ?1, ?2)
         ON CONFLICT(scope, key) DO UPDATE SET json=excluded.json",
        rusqlite::params![kind, json],
    );
}

/// Is `g` at least partly on a monitor that exists right now? Compares in
/// LOGICAL coordinates, and requires a real overlap (not just a shared edge) so
/// a window can't come back as a one-pixel sliver off the side of a screen.
fn on_screen(win: &WebviewWindow, g: &Geom) -> bool {
    let Ok(monitors) = win.available_monitors() else { return false };
    monitors.iter().any(|m| {
        let scale = m.scale_factor();
        let p = m.position().to_logical::<f64>(scale);
        let s = m.size().to_logical::<f64>(scale);
        let (mx, my, mw, mh) = (p.x, p.y, s.width, s.height);
        let (gx, gy, gw, gh) = (g.x as f64, g.y as f64, g.w as f64, g.h as f64);
        // Require ~80 logical px of the title bar area to be visible, so the
        // window can always be grabbed and moved.
        let ox = (gx + gw).min(mx + mw) - gx.max(mx);
        let oy = (gy + gh).min(my + mh) - gy.max(my);
        ox > 80.0 && oy > 40.0
    })
}

/// Apply the remembered geometry for `kind`, if there is one and it still fits
/// a present monitor. Call BEFORE showing the window so there is no jump.
pub fn restore(win: &WebviewWindow, kind: &str) {
    let Some(g) = load(win, kind) else { return };
    if g.w < 200 || g.h < 150 || !on_screen(win, &g) {
        return; // nonsense or off-screen: keep the built-in default
    }
    let _ = win.set_size(LogicalSize::new(g.w as f64, g.h as f64));
    let _ = win.set_position(LogicalPosition::new(g.x as f64, g.y as f64));
    if g.maximized {
        let _ = win.maximize();
    }
    with_map(&LAST, |m| {
        m.insert(kind.to_string(), g);
    });
}

/// Track this window's geometry and persist it. Writes are throttled while
/// dragging and forced on close, so a normal quit always records the final spot.
pub fn watch(win: &WebviewWindow, kind: &str) {
    let kind = kind.to_string();
    let w = win.clone();
    win.on_window_event(move |ev| {
        let force = matches!(ev, WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed);
        match ev {
            WindowEvent::Moved(_)
            | WindowEvent::Resized(_)
            | WindowEvent::CloseRequested { .. }
            | WindowEvent::Destroyed => {}
            _ => return,
        }
        let maximized = w.is_maximized().unwrap_or(false);
        let scale = w.scale_factor().unwrap_or(1.0);
        // While maximized the reported rect IS the maximized one — keep the last
        // normal size so restoring un-maximized lands somewhere sensible.
        let mut g = if maximized {
            let mut prev = LAST
                .lock()
                .ok()
                .and_then(|m| m.as_ref().and_then(|m| m.get(&kind).copied()))
                .unwrap_or(Geom { x: 100, y: 100, w: 1280, h: 800, maximized: false });
            prev.maximized = true;
            prev
        } else {
            let Ok(pos) = w.outer_position() else { return };
            let Ok(size) = w.inner_size() else { return };
            let p = pos.to_logical::<f64>(scale);
            let s = size.to_logical::<f64>(scale);
            Geom {
                x: p.x.round() as i32,
                y: p.y.round() as i32,
                w: s.width.round() as u32,
                h: s.height.round() as u32,
                maximized: false,
            }
        };
        if w.is_minimized().unwrap_or(false) {
            return; // a minimized window's rect is not worth remembering
        }
        if !maximized {
            with_map(&LAST, |m| {
                m.insert(kind.clone(), g);
            });
        } else {
            g.maximized = true;
        }

        if !force {
            let mut skip = false;
            with_map(&WROTE_AT, |m| {
                let now = Instant::now();
                match m.get(&kind) {
                    Some(t) if now.duration_since(*t) < Duration::from_millis(700) => skip = true,
                    _ => {
                        m.insert(kind.clone(), now);
                    }
                }
            });
            if skip {
                return;
            }
        }
        store(&w, &kind, g);
    });
}

/// Restore + watch in one call, then reveal the window.
pub fn apply(win: &WebviewWindow, kind: &str) {
    restore(win, kind);
    watch(win, kind);
    let _ = win.show();
}
