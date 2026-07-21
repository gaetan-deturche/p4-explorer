//! Local file index (SQLite) powering per-keystroke fuzzy search.
//!
//! On workspace open we snapshot every depot path under the stream root into a
//! SQLite table, then load it into memory as `Entry` rows (path + lowercased
//! copy). Search is a fuzzy subsequence scorer run over that in-memory list —
//! no `p4` call per keystroke, and case-insensitive by construction.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::State;

use crate::p4::{self, P4Conn};

pub struct Entry {
    pub path: String,
    pub lower: String,
}

pub struct AppState {
    pub db: Mutex<Connection>,
    /// (client, entries) currently loaded in memory for searching.
    pub mem: Mutex<Option<(String, Arc<Vec<Entry>>)>>,
    /// PIDs of the running `p4 sync` children (preview + real; for cancellation).
    pub sync_pids: Arc<Mutex<Vec<u32>>>,
    /// Set when the user cancels a sync, so the backend stops between phases.
    pub sync_abort: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        AppState {
            db: Mutex::new(db),
            mem: Mutex::new(None),
            sync_pids: Arc::new(Mutex::new(Vec::new())),
            sync_abort: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub fn init_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS file_index(client TEXT NOT NULL, path TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS idx_file_client ON file_index(client);",
    )
}

fn to_entries(paths: Vec<String>) -> Arc<Vec<Entry>> {
    Arc::new(
        paths
            .into_iter()
            .map(|p| {
                let lower = p.to_lowercase();
                Entry { path: p, lower }
            })
            .collect(),
    )
}

/// Ensure the in-memory index for `client` is loaded (from SQLite if needed).
/// Returns the entry list (empty if the client has never been indexed).
fn ensure_loaded(state: &AppState, client: &str) -> Arc<Vec<Entry>> {
    if let Some((c, e)) = state.mem.lock().unwrap().as_ref() {
        if c == client {
            return e.clone();
        }
    }
    let paths: Vec<String> = {
        let db = state.db.lock().unwrap();
        let mut stmt = match db.prepare("SELECT path FROM file_index WHERE client=?1") {
            Ok(s) => s,
            Err(_) => return Arc::new(Vec::new()),
        };
        let rows = stmt.query_map([client], |r| r.get::<_, String>(0));
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    };
    let entries = to_entries(paths);
    *state.mem.lock().unwrap() = Some((client.to_string(), entries.clone()));
    entries
}

/// Number of indexed files for a client (0 = needs building).
#[tauri::command]
pub async fn index_status(state: State<'_, AppState>, client: String) -> Result<usize, String> {
    Ok(ensure_loaded(&state, &client).len())
}

/// (Re)build the index for a client: `p4 files //root/...`, keep the existing
/// (non-deleted) files, store them, and load into memory. Returns the count.
#[tauri::command]
pub async fn index_build(
    state: State<'_, AppState>,
    conn: P4Conn,
    client: String,
    root: String,
) -> Result<usize, String> {
    let pattern = format!("{}/...", root.trim_end_matches('/'));
    let paths: Vec<String> = tauri::async_runtime::spawn_blocking(move || {
        let recs = p4::run(&conn, &["files", &pattern])?;
        let out: Vec<String> = recs
            .into_iter()
            .filter(|r| {
                let a = r.get("action").and_then(|v| v.as_str()).unwrap_or("");
                !a.contains("delete") && a != "purge" && a != "archive"
            })
            .filter_map(|r| {
                r.get("depotFile")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .collect();
        Ok::<Vec<String>, String>(out)
    })
    .await
    .map_err(|e| format!("index task failed: {e}"))??;

    let n = paths.len();
    {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM file_index WHERE client=?1", [&client])
            .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare("INSERT INTO file_index(client, path) VALUES(?1, ?2)")
                .map_err(|e| e.to_string())?;
            for p in &paths {
                stmt.execute(rusqlite::params![client, p])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    *state.mem.lock().unwrap() = Some((client.clone(), to_entries(paths)));
    Ok(n)
}

/// Fuzzy subsequence search over the client's index. Returns the best `max`
/// depot paths, ranked. Case-insensitive.
#[tauri::command]
pub async fn index_search(
    state: State<'_, AppState>,
    client: String,
    query: String,
    max: usize,
) -> Result<Vec<String>, String> {
    let entries = ensure_loaded(&state, &client);
    let q = query.trim().to_lowercase();
    if q.is_empty() || entries.is_empty() {
        return Ok(Vec::new());
    }
    let qb = q.as_bytes();

    let mut scored: Vec<(i32, usize)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if let Some(s) = fuzzy_score(qb, &e.lower) {
            scored.push((s, i));
        }
    }
    // Highest score first; tie-break shorter path, then alphabetical.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| entries[a.1].path.len().cmp(&entries[b.1].path.len()))
            .then_with(|| entries[a.1].path.cmp(&entries[b.1].path))
    });
    scored.truncate(max);
    Ok(scored.into_iter().map(|(_, i)| entries[i].path.clone()).collect())
}

/// fzf-style subsequence scorer. `None` if `q` is not a subsequence of `path`.
/// Rewards contiguous runs and matches at word boundaries (after / _ - . space
/// or a digit→alpha change), and gently prefers shorter paths.
fn fuzzy_score(q: &[u8], path: &str) -> Option<i32> {
    let p = path.as_bytes();
    let mut qi = 0usize;
    let mut score = 0i32;
    let mut last: i32 = -2;
    let mut streak = 0i32;
    for (i, &c) in p.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if c == q[qi] {
            let mut s = 1;
            if i as i32 == last + 1 {
                streak += 1;
                s += 4 + streak;
            } else {
                streak = 0;
            }
            let boundary = i == 0 || matches!(p[i - 1], b'/' | b'_' | b'-' | b'.' | b' ');
            if boundary {
                s += 10;
            }
            score += s;
            last = i as i32;
            qi += 1;
        }
    }
    if qi == q.len() {
        Some(score - (p.len() as i32) / 8)
    } else {
        None
    }
}
