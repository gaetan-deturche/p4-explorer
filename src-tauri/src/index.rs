//! Local file index (SQLite) powering per-keystroke fuzzy search.
//!
//! On workspace open we snapshot every depot path under the stream root into a
//! SQLite table, then load it into memory as one `Paths` buffer. Search is a
//! case-insensitive substring + fuzzy subsequence pass over that buffer — no
//! `p4` call per keystroke.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rusqlite::Connection;
use tauri::{Manager, State};

use crate::p4::{self, P4Conn};

/// Every path of one index in one buffer. A String per path, plus a lowercase
/// copy, cost 5.4 GB for the 18.7M-path depot; this is the 2.2 GB of text plus
/// one offset each. Matching folds ASCII case on the fly instead.
pub struct Paths {
    text: String,
    ends: Vec<usize>,
}

impl Paths {
    fn new(paths: Vec<String>) -> Self {
        let mut text = String::with_capacity(paths.iter().map(String::len).sum());
        let mut ends = Vec::with_capacity(paths.len());
        for p in &paths {
            text.push_str(p);
            ends.push(text.len());
        }
        Paths { text, ends }
    }
    fn len(&self) -> usize {
        self.ends.len()
    }
    fn get(&self, i: usize) -> &str {
        let from = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.text[from..self.ends[i]]
    }
}

/// One index held in memory, with when it was last searched (for eviction).
struct Loaded {
    key: String,
    paths: Arc<Paths>,
    used: u64,
}

/// Path text kept in memory across indexes. Workspace and Local stay resident
/// together (~60 MB each); the depot alone is over it, so it evicts them and is
/// evicted in turn when they come back.
const MEM_BUDGET: usize = 1 << 30;

pub struct AppState {
    /// The file index's connection — long scans and bulk inserts live here.
    pub db: Mutex<Connection>,
    /// The cache table's own connection: cache reads back the ENTIRE UI at
    /// boot, so they must never queue behind an index scan on `db`.
    pub cache_db: Mutex<Connection>,
    /// The indexes loaded in memory for searching.
    mem: Mutex<Vec<Loaded>>,
    /// Held while an index loads from SQLite: searches typed meanwhile wait for
    /// that one load instead of each running their own.
    loading: Mutex<()>,
    /// Bumped by every search; a scan that sees it move is abandoned, since the
    /// panel only shows the newest keystroke's answer.
    search_gen: AtomicU64,
    /// PIDs of the running `p4 sync` children (preview + real; for cancellation).
    pub sync_pids: Arc<Mutex<Vec<u32>>>,
    /// Set when the user cancels a sync, so the backend stops between phases.
    pub sync_abort: Arc<AtomicBool>,
    /// PID of the running offline-changes scan (killed on an interactive write).
    pub offline_pid: Arc<Mutex<Option<u32>>>,
    /// Set when an offline scan is cancelled, so its result is discarded.
    pub offline_abort: Arc<AtomicBool>,
    /// Writes in progress that no offline scan may start under (see
    /// `commands::sync::hold_offline_scan`).
    pub offline_writes: Arc<AtomicUsize>,
}

impl AppState {
    pub fn new(db: Connection, cache_db: Connection) -> Self {
        AppState {
            db: Mutex::new(db),
            cache_db: Mutex::new(cache_db),
            mem: Mutex::new(Vec::new()),
            loading: Mutex::new(()),
            search_gen: AtomicU64::new(0),
            sync_pids: Arc::new(Mutex::new(Vec::new())),
            sync_abort: Arc::new(AtomicBool::new(false)),
            offline_pid: Arc::new(Mutex::new(None)),
            offline_abort: Arc::new(AtomicBool::new(false)),
            offline_writes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Per-connection pragmas: WAL so the two connections don't block each other
/// (one writer + readers run concurrently), NORMAL sync (WAL-safe), and a busy
/// timeout instead of hard SQLITE_BUSY errors when both do write.
pub fn init_conn(db: &Connection) -> rusqlite::Result<()> {
    db.pragma_update(None, "busy_timeout", 5000)?; // first: the others may need the lock
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    // After a successful checkpoint the WAL file shrinks back to this instead of
    // squatting at its high-water mark forever (observed: 269MB of dead
    // generations, because two busy connections kept starving the passive
    // auto-checkpoint).
    db.pragma_update(None, "journal_size_limit", 67108864)?; // 64 MiB
    Ok(())
}

/// Best-effort TRUNCATE checkpoint: backfill the WAL into the database and
/// shrink the file. Never fails the caller — a busy reader just means the next
/// quiesce point gets it.
pub fn checkpoint(db: &Connection) {
    let _ = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
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
    )?;
    crate::commands::stash_schema(db)
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
    let db = state.cache_db.lock().unwrap();
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
    let db = state.cache_db.lock().unwrap();
    db.execute(
        "INSERT INTO cache(scope, key, json) VALUES(?1, ?2, ?3)
         ON CONFLICT(scope, key) DO UPDATE SET json=excluded.json",
        rusqlite::params![scope, key, json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Every entry of a scope, for hydrating a whole namespace in one query (the
/// boot-time prefs live in one scope, and each is read synchronously).
#[tauri::command]
pub async fn cache_get_scope(
    state: tauri::State<'_, AppState>,
    scope: String,
) -> Result<Vec<(String, String)>, String> {
    let db = state.cache_db.lock().unwrap();
    let mut stmt = db
        .prepare("SELECT key, json FROM cache WHERE scope=?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&scope], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Delete every entry in a scope (e.g. all of a client's tree cache).
#[tauri::command]
pub async fn cache_clear(state: tauri::State<'_, AppState>, scope: String) -> Result<(), String> {
    let db = state.cache_db.lock().unwrap();
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
    let db = state.cache_db.lock().unwrap();
    db.execute(
        "DELETE FROM cache WHERE scope=?1 AND key=?2",
        rusqlite::params![scope, key],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The index for `key` if it is in memory, marked as just used.
fn cached(state: &AppState, key: &str) -> Option<Arc<Paths>> {
    static CLOCK: AtomicU64 = AtomicU64::new(0);
    let mut mem = state.mem.lock().unwrap();
    let l = mem.iter_mut().find(|l| l.key == key)?;
    l.used = CLOCK.fetch_add(1, Ordering::Relaxed) + 1;
    Some(l.paths.clone())
}

/// Keep `paths` in memory as `key`'s index, evicting the least recently used
/// others while the total is over `MEM_BUDGET`.
fn remember(state: &AppState, key: &str, paths: Arc<Paths>) {
    let mut mem = state.mem.lock().unwrap();
    mem.retain(|l| l.key != key);
    mem.push(Loaded { key: key.to_string(), paths, used: u64::MAX });
    mem.sort_by_key(|l| std::cmp::Reverse(l.used));
    let mut total = 0;
    mem.retain(|l| {
        total += l.paths.text.len();
        l.used == u64::MAX || total <= MEM_BUDGET
    });
    drop(mem);
    cached(state, key); // a real timestamp in place of the MAX placeholder
}

/// Ensure the in-memory index for `client` is loaded (from SQLite if needed).
/// Returns the paths (empty if the client has never been indexed).
fn ensure_loaded(state: &AppState, client: &str) -> Arc<Paths> {
    if let Some(p) = cached(state, client) {
        return p;
    }
    let _one = state.loading.lock().unwrap();
    if let Some(p) = cached(state, client) {
        return p; // loaded by the search this one queued behind
    }
    let t = Instant::now();
    let paths: Vec<String> = {
        let db = state.db.lock().unwrap();
        let mut stmt = match db.prepare("SELECT path FROM file_index WHERE client=?1") {
            Ok(s) => s,
            Err(_) => return Arc::new(Paths::new(Vec::new())),
        };
        let rows = stmt.query_map([client], |r| r.get::<_, String>(0));
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    };
    let paths = Arc::new(Paths::new(paths));
    p4::log_note(&format!(
        "index {client}: loaded {} paths ({} MB) in {} ms",
        paths.len(),
        paths.text.len() >> 20,
        t.elapsed().as_millis()
    ));
    remember(state, client, paths.clone());
    paths
}

/// Load `client`'s index into memory ahead of the first keystroke.
#[tauri::command]
pub async fn index_preload(app: tauri::AppHandle, client: String) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || ensure_loaded(&app.state::<AppState>(), &client).len())
        .await
        .map_err(|e| format!("index task failed: {e}"))
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
    let t = Instant::now();
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
        // The index rebuild is by far the biggest writer (hundreds of MB of
        // frames in one transaction). The moment it commits is the cheapest
        // time to backfill — leaving it to the passive auto-checkpoint is what
        // let the WAL grow unbounded.
        checkpoint(&db);
    }
    p4::log_note(&format!("index {key}: stored {n} paths in {} ms", t.elapsed().as_millis()));
    remember(state, key, Arc::new(Paths::new(paths)));
    Ok(n)
}

/// Build output: 3.0M of a UE workspace's 3.56M files on disk, and nothing a
/// file search is for. Skipping them took the walk from ~3 min to 10 s.
const BUILD_OUTPUT_DIRS: [&str; 6] = ["intermediate", "binaries", "saved", "deriveddatacache", ".vs", ".git"];

/// Recursively collect every file under `dir`, as depot-form paths rooted at
/// `base` (i.e. `base/<relative path with '/' separators>`), build output aside.
fn walk_local(dir: &std::path::Path, base: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let child = format!("{base}/{name}");
        if ft.is_dir() {
            if BUILD_OUTPUT_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d)) {
                continue;
            }
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
    let walked = key.clone();
    let paths = tauri::async_runtime::spawn_blocking(move || {
        let t = Instant::now();
        let mut out = Vec::new();
        if !root.is_empty() {
            walk_local(std::path::Path::new(&root), root_path.trim_end_matches('/'), &mut out);
        }
        p4::log_note(&format!("index {walked}: walked {} files in {} ms", out.len(), t.elapsed().as_millis()));
        out
    })
    .await
    .map_err(|e| format!("index task failed: {e}"))?;
    store_paths(&state, &key, paths)
}

/// Both halves of a file search over the index, from ONE pass (every substring
/// match is also a subsequence match): `contains` are literal case-insensitive
/// substring matches — what the file view filters on, so results are predictable
/// — and `fuzzy` is the ranked best-match list shown as suggestions, closest
/// file name first (a file literally called what you typed leads it).
#[derive(serde::Serialize, Default)]
pub struct SearchHits {
    pub contains: Vec<String>,
    pub fuzzy: Vec<String>,
}

/// Search the client's index, case-insensitively. `max` caps each list.
#[tauri::command]
pub async fn index_search(
    app: tauri::AppHandle,
    client: String,
    query: String,
    max: usize,
) -> Result<SearchHits, String> {
    let gen = app.state::<AppState>().search_gen.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || search(&app.state::<AppState>(), gen, &client, &query, max))
        .await
        .map_err(|e| format!("index task failed: {e}"))
}

/// `contains` against an already-lowercased `q`, folding ASCII case in `hay`.
fn contains_folded(hay: &[u8], q: &[u8]) -> bool {
    if q.is_empty() || q.len() > hay.len() {
        return q.is_empty();
    }
    let (lower, upper) = (q[0], q[0].to_ascii_uppercase());
    let last = hay.len() - q.len();
    let mut i = 0;
    while i <= last {
        match hay[i..=last].iter().position(|&b| b == lower || b == upper) {
            Some(k) => i += k,
            None => return false,
        }
        if hay[i + 1..i + q.len()].eq_ignore_ascii_case(&q[1..]) {
            return true;
        }
        i += 1;
    }
    false
}

/// The first `max` of `v` in `cmp` order, sorting only those.
fn keep_first<T>(v: &mut Vec<T>, max: usize, cmp: impl Fn(&T, &T) -> std::cmp::Ordering) {
    if max == 0 {
        v.clear();
        return;
    }
    if v.len() > max {
        v.select_nth_unstable_by(max - 1, &cmp);
        v.truncate(max);
    }
    v.sort_by(cmp);
}

/// The search behind `index_search`: empty when search `gen` has been
/// superseded, which the panel discards unseen anyway.
fn search(state: &AppState, gen: u64, client: &str, query: &str, max: usize) -> SearchHits {
    let t = Instant::now();
    let paths = ensure_loaded(state, client);
    let q = query.trim().to_lowercase();
    if q.is_empty() || paths.len() == 0 {
        return SearchHits::default();
    }
    let qb = q.as_bytes();
    let superseded = || state.search_gen.load(Ordering::SeqCst) != gen;

    let mut scored: Vec<(i32, usize)> = Vec::new();
    let mut contains: Vec<usize> = Vec::new();
    for i in 0..paths.len() {
        if i % 65536 == 0 && superseded() {
            p4::log_note(&format!("index {client}: search \"{q}\" superseded after {} ms", t.elapsed().as_millis()));
            return SearchHits::default();
        }
        let path = paths.get(i);
        // ASCII case folds byte by byte; anything else takes the full lowercase.
        let lowered;
        let p = if path.is_ascii() {
            path
        } else {
            lowered = path.to_lowercase();
            lowered.as_str()
        };
        if contains_folded(p.as_bytes(), qb) {
            contains.push(i);
        }
        // Fuzzy suggestions score the FILE NAME, not the whole path: a query
        // scattered across a long path is meaningless as a "did you mean"
        // ("linefilter.ush" would otherwise match Definitions.BuildPatchServices.h
        // by picking letters out of the directories).
        let name = match p.rfind('/') {
            Some(k) => &p[k + 1..],
            None => p,
        };
        if let Some(s) = fuzzy_score(qb, name) {
            scored.push((s, i));
        }
    }
    let found = (contains.len(), scored.len());
    // Highest score first; tie-break shorter path, then alphabetical.
    keep_first(&mut scored, max, |a, b| {
        b.0.cmp(&a.0)
            .then_with(|| paths.get(a.1).len().cmp(&paths.get(b.1).len()))
            .then_with(|| paths.get(a.1).cmp(paths.get(b.1)))
    });
    // Substring hits stay in index order (alphabetical by path) — a filtered
    // view of the tree, not a ranking.
    keep_first(&mut contains, max, |&a, &b| paths.get(a).cmp(paths.get(b)));
    p4::log_note(&format!(
        "index {client}: search \"{q}\" {} ms ({} contains, {} fuzzy of {})",
        t.elapsed().as_millis(),
        found.0,
        found.1,
        paths.len()
    ));
    SearchHits {
        contains: contains.into_iter().map(|i| paths.get(i).to_string()).collect(),
        fuzzy: scored.into_iter().map(|(_, i)| paths.get(i).to_string()).collect(),
    }
}

/// fzf-style subsequence scorer, folding ASCII case in `path` against a
/// lowercase `q`. `None` if `q` is not a subsequence of `path`.
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
        if c.to_ascii_lowercase() == q[qi] {
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

/// One piece of the offline scan: depot paths for `p4 -x` (folder patterns, or
/// exact files when a single folder holds too many itself), and how many
/// indexed files they cover.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct ScanPiece {
    pub patterns: Vec<String>,
    pub files: usize,
}

/// Cut the workspace's files into pieces of at most `target` files, balanced by
/// count rather than by folder: a quarter of the stream sits in one folder
/// (`__ExternalActors__`, 143k files). A folder that fits is one `dir/...`; a
/// bigger one is its own files (`dir/*`) plus each subfolder, cut the same way;
/// neighbouring small parts are then packed together, in path order.
///
/// Why: p4d keeps running a killed reconcile until its current stretch of work
/// is done — 153 s after a whole-workspace scan was killed 122 s in, against
/// 0.5 s for a 15k-file piece. Pieces of this size took 0.2–3.2 s each.
pub fn plan_scan(paths: &[&str], target: usize) -> Vec<ScanPiece> {
    let target = target.max(1);
    let mut sorted = paths.to_vec();
    // Sorted, every folder's files are one contiguous run (shared prefix).
    sorted.sort_unstable();
    let (Some(first), Some(last)) = (sorted.first(), sorted.last()) else {
        return Vec::new();
    };
    let common = first.bytes().zip(last.bytes()).take_while(|(a, b)| a == b).count();
    let root = &first[..first[..common].rfind('/').map_or(0, |k| k + 1)];
    let mut parts = Vec::new();
    split_folder(&sorted, root, target, &mut parts);

    let mut pieces: Vec<ScanPiece> = Vec::new();
    for (patterns, files) in parts {
        match pieces.last_mut() {
            Some(p) if p.files + files <= target => {
                p.patterns.extend(patterns);
                p.files += files;
            }
            _ => pieces.push(ScanPiece { patterns, files }),
        }
    }
    pieces
}

/// The parts covering `files`, all under folder `prefix` (which ends in '/').
fn split_folder(files: &[&str], prefix: &str, target: usize, out: &mut Vec<(Vec<String>, usize)>) {
    if files.len() <= target {
        out.push((vec![format!("{prefix}...")], files.len()));
        return;
    }
    let direct: Vec<&str> = files.iter().copied().filter(|f| !f[prefix.len()..].contains('/')).collect();
    if direct.len() > target {
        for chunk in direct.chunks(target) {
            out.push((chunk.iter().map(|f| f.to_string()).collect(), chunk.len()));
        }
    } else if !direct.is_empty() {
        out.push((vec![format!("{prefix}*")], direct.len()));
    }
    let mut i = 0;
    while i < files.len() {
        let Some(slash) = files[i][prefix.len()..].find('/') else {
            i += 1;
            continue;
        };
        let child = &files[i][..prefix.len() + slash + 1];
        let end = i + files[i..].iter().take_while(|f| f.starts_with(child)).count();
        split_folder(&files[i..end], child, target, out);
        i = end;
    }
}

/// The offline scan's pieces for `client`, from its workspace index. Empty when
/// there is no index (the caller then scans the workspace in one go).
#[tauri::command]
pub async fn index_scan_plan(app: tauri::AppHandle, client: String, target: usize) -> Result<Vec<ScanPiece>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = ensure_loaded(&app.state::<AppState>(), &client);
        let t = Instant::now();
        let all: Vec<&str> = (0..paths.len()).map(|i| paths.get(i)).collect();
        let plan = plan_scan(&all, target);
        p4::log_note(&format!(
            "offline scan {client}: {} pieces of <= {target} files planned in {} ms",
            plan.len(),
            t.elapsed().as_millis()
        ));
        plan
    })
    .await
    .map_err(|e| format!("plan task failed: {e}"))
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn plan(paths: &[&str], target: usize) -> Vec<(Vec<&'static str>, usize)> {
        plan_scan(paths, target)
            .into_iter()
            .map(|p| (p.patterns.into_iter().map(|s| &*s.leak()).collect(), p.files))
            .collect()
    }

    #[test]
    fn a_workspace_that_fits_is_one_piece() {
        assert_eq!(plan(&["//s/m/a.txt", "//s/m/b/c.txt"], 5), vec![(vec!["//s/m/..."], 2)]);
    }

    #[test]
    fn a_big_folder_is_cut_and_small_neighbours_are_packed() {
        let files = [
            "//s/m/Big/x/1", "//s/m/Big/x/2", "//s/m/Big/y/1", "//s/m/Big/y/2", "//s/m/Big/top",
            "//s/m/Small/1", "//s/m/Tiny/1", "//s/m/root.ini",
        ];
        assert_eq!(
            plan(&files, 3),
            vec![
                (vec!["//s/m/*", "//s/m/Big/*"], 2),
                (vec!["//s/m/Big/x/..."], 2),
                (vec!["//s/m/Big/y/...", "//s/m/Small/..."], 3),
                (vec!["//s/m/Tiny/..."], 1),
            ]
        );
    }

    #[test]
    fn a_folder_too_full_by_itself_is_listed_file_by_file() {
        let files = ["//s/m/d/1", "//s/m/d/2", "//s/m/d/3", "//s/m/e/1"];
        assert_eq!(
            plan(&files, 2),
            vec![
                (vec!["//s/m/d/1", "//s/m/d/2"], 2),
                (vec!["//s/m/d/3", "//s/m/e/..."], 2),
            ]
        );
    }

    #[test]
    fn every_file_is_covered_exactly_once() {
        let files: Vec<String> = (0..500).map(|i| format!("//s/m/{}/{}/f{i}", i % 7, i % 3)).collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let pieces = plan_scan(&refs, 40);
        let covers = |p: &str, f: &str| {
            if let Some(pre) = p.strip_suffix("...") {
                f.starts_with(pre)
            } else if let Some(pre) = p.strip_suffix('*') {
                f.starts_with(pre) && !f[pre.len()..].contains('/')
            } else {
                p == f
            }
        };
        for f in &refs {
            let n: usize = pieces.iter().flat_map(|p| &p.patterns).filter(|p| covers(p, f)).count();
            assert_eq!(n, 1, "{f}");
        }
        assert!(pieces.iter().all(|p| p.files <= 40));
        assert_eq!(pieces.iter().map(|p| p.files).sum::<usize>(), 500);
    }
}
