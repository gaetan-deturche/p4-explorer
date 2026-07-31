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
    /// PID of the running offline-changes scan (killed on an interactive write).
    pub offline_pid: Arc<Mutex<Option<u32>>>,
    /// Set when an offline scan is cancelled, so its result is discarded.
    pub offline_abort: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        AppState {
            db: Mutex::new(db),
            mem: Mutex::new(None),
            sync_pids: Arc::new(Mutex::new(Vec::new())),
            sync_abort: Arc::new(AtomicBool::new(false)),
            offline_pid: Arc::new(Mutex::new(None)),
            offline_abort: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub fn init_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS file_index(client TEXT NOT NULL, path TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS idx_file_client ON file_index(client);
         CREATE TABLE IF NOT EXISTS cache(
             scope TEXT NOT NULL,
             key   TEXT NOT NULL,
             json  TEXT NOT NULL,
             PRIMARY KEY(scope, key)
         );",
    )
}

// --- Generic blob cache (source of truth for view data; the front-end mirrors
// hot entries in localStorage/memory for instant reads). Small, indexed by the
// (scope, key) primary key. -----------------------------------------------------

/// Read a cached blob, or None if absent.
#[tauri::command]
pub async fn cache_get(
    state: tauri::State<'_, AppState>,
    scope: String,
    key: String,
) -> Result<Option<String>, String> {
    let db = state.db.lock().unwrap();
    match db.query_row(
        "SELECT json FROM cache WHERE scope=?1 AND key=?2",
        rusqlite::params![scope, key],
        |r| r.get::<_, String>(0),
    ) {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Insert or replace a cached blob.
#[tauri::command]
pub async fn cache_set(
    state: tauri::State<'_, AppState>,
    scope: String,
    key: String,
    json: String,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO cache(scope, key, json) VALUES(?1, ?2, ?3)
         ON CONFLICT(scope, key) DO UPDATE SET json=excluded.json",
        rusqlite::params![scope, key, json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete every entry in a scope (e.g. all of a client's tree cache).
#[tauri::command]
pub async fn cache_clear(state: tauri::State<'_, AppState>, scope: String) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM cache WHERE scope=?1", rusqlite::params![scope])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a single cache entry.
#[tauri::command]
pub async fn cache_del(
    state: tauri::State<'_, AppState>,
    scope: String,
    key: String,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "DELETE FROM cache WHERE scope=?1 AND key=?2",
        rusqlite::params![scope, key],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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

/// Number of indexed files for a client (0 = needs building). A COUNT query, so
/// it does NOT load the (potentially millions of) rows into memory — that only
/// happens on an actual search (`ensure_loaded`).
#[tauri::command]
pub async fn index_status(state: State<'_, AppState>, client: String) -> Result<usize, String> {
    let db = state.db.lock().unwrap();
    let n: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM file_index WHERE client=?1",
            [&client],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n as usize)
}

/// Depot paths from `p4 files <pattern>`, dropping deleted/purged/archived revs.
fn server_files(conn: &P4Conn, pattern: &str) -> Result<Vec<String>, String> {
    let recs = p4::run(conn, &["files", pattern])?;
    Ok(recs
        .into_iter()
        .filter(|r| {
            let a = r.get("action").and_then(|v| v.as_str()).unwrap_or("");
            !a.contains("delete") && a != "purge" && a != "archive"
        })
        .filter_map(|r| r.get("depotFile").and_then(|v| v.as_str()).map(str::to_string))
        .collect())
}

/// Store `paths` under `key` (replacing any prior rows) and load into memory.
fn store_paths(state: &AppState, key: &str, paths: Vec<String>) -> Result<usize, String> {
    let n = paths.len();
    {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM file_index WHERE client=?1", [key])
            .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare("INSERT INTO file_index(client, path) VALUES(?1, ?2)")
                .map_err(|e| e.to_string())?;
            for p in &paths {
                stmt.execute(rusqlite::params![key, p]).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    *state.mem.lock().unwrap() = Some((key.to_string(), to_entries(paths)));
    Ok(n)
}

/// Recursively collect every file under `dir`, as depot-form paths rooted at
/// `base` (i.e. `base/<relative path with '/' separators>`).
fn walk_local(dir: &std::path::Path, base: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let child = format!("{base}/{name}");
        if ft.is_dir() {
            walk_local(&e.path(), &child, out);
        } else if ft.is_file() {
            out.push(child);
        }
    }
}

/// (Re)build the index for a client key: `p4 files //root/...`, keep the
/// existing (non-deleted) files, store them, and load into memory. Returns
/// the count. Used for the workspace source (`key` = client name).
#[tauri::command]
pub async fn index_build(
    state: State<'_, AppState>,
    conn: P4Conn,
    client: String,
    root: String,
) -> Result<usize, String> {
    let pattern = format!("{}/...", root.trim_end_matches('/'));
    let paths = tauri::async_runtime::spawn_blocking(move || server_files(&conn, &pattern))
        .await
        .map_err(|e| format!("index task failed: {e}"))??;
    store_paths(&state, &client, paths)
}

/// Build the whole-depot index (`p4 files //...`, every depot), stored under
/// `key` (shared per server). Can be large/slow on big servers.
#[tauri::command]
pub async fn index_build_depot(
    state: State<'_, AppState>,
    conn: P4Conn,
    key: String,
) -> Result<usize, String> {
    let paths = tauri::async_runtime::spawn_blocking(move || server_files(&conn, "//..."))
        .await
        .map_err(|e| format!("index task failed: {e}"))??;
    store_paths(&state, &key, paths)
}

/// Build the on-disk index for the Local source: every file under `root` (the
/// workspace root on disk), as depot-form paths rooted at `root_path` (the
/// stream root), stored under `key`.
#[tauri::command]
pub async fn index_build_local(
    state: State<'_, AppState>,
    key: String,
    root: String,
    root_path: String,
) -> Result<usize, String> {
    let paths = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        if !root.is_empty() {
            walk_local(std::path::Path::new(&root), root_path.trim_end_matches('/'), &mut out);
        }
        out
    })
    .await
    .map_err(|e| format!("index task failed: {e}"))?;
    store_paths(&state, &key, paths)
}

/// Both halves of a file search over the index, from ONE pass (every substring
/// match is also a subsequence match): `contains` are literal case-insensitive
/// substring matches — what the file view filters on, so results are predictable
/// — and `fuzzy` are ranked subsequence matches, offered as suggestions.
#[derive(serde::Serialize, Default)]
pub struct SearchHits {
    pub contains: Vec<String>,
    pub fuzzy: Vec<String>,
}

/// Search the client's index, case-insensitively. `max` caps each list.
#[tauri::command]
pub async fn index_search(
    state: State<'_, AppState>,
    client: String,
    query: String,
    max: usize,
) -> Result<SearchHits, String> {
    let entries = ensure_loaded(&state, &client);
    let q = query.trim().to_lowercase();
    if q.is_empty() || entries.is_empty() {
        return Ok(SearchHits::default());
    }
    let qb = q.as_bytes();

    let mut scored: Vec<(i32, usize)> = Vec::new();
    let mut contains: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.lower.contains(&q) {
            contains.push(i);
        }
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
    // Substring hits stay in index order (alphabetical by path) — a filtered
    // view of the tree, not a ranking.
    contains.sort_by(|&a, &b| entries[a].path.cmp(&entries[b].path));
    contains.truncate(max);
    Ok(SearchHits {
        contains: contains.into_iter().map(|i| entries[i].path.clone()).collect(),
        fuzzy: scored.into_iter().map(|(_, i)| entries[i].path.clone()).collect(),
    })
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
