//! Folder history kept in SQLite. Every changelist a History view fetches is
//! stored per server and path, and a search runs over the stored history instead
//! of paging the whole of it through the view. Opening a folder's History fills
//! in its full history in the background with one `p4 changes` (a few thousand
//! changelists come back in well under a second); after that only what is newer
//! is asked for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::{run, v};
use crate::index::AppState;
use crate::p4::{P4Conn, Record};

pub fn histdb_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS hist_change(
             server TEXT NOT NULL,
             change INTEGER NOT NULL,
             time   INTEGER NOT NULL,
             user   TEXT NOT NULL,
             client TEXT NOT NULL,
             desc   TEXT NOT NULL,
             PRIMARY KEY(server, change)
         );
         CREATE TABLE IF NOT EXISTS hist_path(
             server TEXT NOT NULL,
             path   TEXT NOT NULL,
             change INTEGER NOT NULL,
             PRIMARY KEY(server, path, change)
         );
         -- The path's whole submitted history is stored, up to `newest`.
         CREATE TABLE IF NOT EXISTS hist_full(
             server TEXT NOT NULL,
             path   TEXT NOT NULL,
             newest INTEGER NOT NULL,
             PRIMARY KEY(server, path)
         );",
    )
}

/// The `p4 changes` spec a folder's history is read with (and stored under).
pub(crate) fn history_base(path: &str) -> String {
    if path.ends_with("...") || path.contains('*') {
        path.to_string()
    } else {
        format!("{}/...", path.trim_end_matches('/'))
    }
}

fn text(r: &Record, key: &str) -> String {
    r.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn number(r: &Record, key: &str) -> i64 {
    text(r, key).parse().unwrap_or(0)
}

/// Store changelists `p4 changes` returned for `base` on `server`.
pub(crate) fn remember(db: &mut Connection, server: &str, base: &str, recs: &[Record]) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    {
        let mut change = tx.prepare(
            "INSERT INTO hist_change(server, change, time, user, client, desc) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(server, change) DO UPDATE SET time=excluded.time, user=excluded.user,
                 client=excluded.client, desc=excluded.desc",
        )?;
        let mut path = tx.prepare("INSERT OR IGNORE INTO hist_path(server, path, change) VALUES(?1, ?2, ?3)")?;
        for r in recs {
            let n = number(r, "change");
            if n <= 0 {
                continue;
            }
            change.execute(params![server, n, number(r, "time"), text(r, "user"), text(r, "client"), text(r, "desc")])?;
            path.execute(params![server, base, n])?;
        }
    }
    tx.commit()
}

/// Store a page of history off the async workers (called after `p4_changes`).
pub(crate) async fn remember_later(app: AppHandle, server: String, base: String, recs: Vec<Record>) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let lock = state.db.lock();
        if let Ok(mut db) = lock {
            let _ = remember(&mut db, &server, &base, &recs);
        }
    })
    .await;
}

/// Newer changelists are asked for at most this often per path while typing.
const REFRESH: Duration = Duration::from_secs(60);

fn refreshed() -> &'static Mutex<HashMap<String, Instant>> {
    static R: std::sync::OnceLock<Mutex<HashMap<String, Instant>>> = std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One fill at a time per path: a search arriving while the background fill
/// runs waits for it, then finds the history fresh, instead of fetching again.
fn gate(key: &str) -> Arc<tauri::async_runtime::Mutex<()>> {
    static G: std::sync::OnceLock<Mutex<HashMap<String, Arc<tauri::async_runtime::Mutex<()>>>>> =
        std::sync::OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap().entry(key.to_string()).or_default().clone()
}

/// Fill in a folder's stored history in the background as its History opens,
/// so the first search finds it ready.
#[tauri::command]
pub async fn history_warm(app: AppHandle, conn: P4Conn, path: String) -> Result<(), String> {
    let base = history_base(&path);
    complete(&app, &conn, &conn.port, &base).await
}

/// Make the stored history of `base` complete up to now: all of it the first
/// time, then only what is newer than what is stored.
async fn complete(app: &AppHandle, conn: &P4Conn, server: &str, base: &str) -> Result<(), String> {
    let key = format!("{server}|{base}");
    let gate = gate(&key);
    let _filling = gate.lock().await;
    if refreshed().lock().unwrap().get(&key).is_some_and(|t| t.elapsed() < REFRESH) {
        return Ok(());
    }
    let newest: Option<i64> = {
        let (a, s, b) = (app.clone(), server.to_string(), base.to_string());
        tauri::async_runtime::spawn_blocking(move || {
            let state = a.state::<AppState>();
            let db = state.db.lock().unwrap();
            db.query_row("SELECT newest FROM hist_full WHERE server=?1 AND path=?2", params![s, b], |r| r.get(0)).ok()
        })
        .await
        .map_err(|e| e.to_string())?
    };
    let spec = match newest {
        Some(n) => format!("{base}@{},@now", n + 1),
        None => base.to_string(),
    };
    let recs = run(conn.clone(), v(&["changes", "-l", "-s", "submitted", &spec])).await?;
    let top = recs.iter().map(|r| number(r, "change")).max().unwrap_or(0).max(newest.unwrap_or(0));
    let (a, s, b) = (app.clone(), server.to_string(), base.to_string());
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let state = a.state::<AppState>();
        let mut db = state.db.lock().unwrap();
        remember(&mut db, &s, &b, &recs).map_err(|e| e.to_string())?;
        db.execute(
            "INSERT INTO hist_full(server, path, newest) VALUES(?1, ?2, ?3)
             ON CONFLICT(server, path) DO UPDATE SET newest=excluded.newest",
            params![s, b, top],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    refreshed().lock().unwrap().insert(key, Instant::now());
    Ok(())
}

/// The History search's rule: every term matches one field, as a substring or,
/// for a short plain term, as a subsequence of one word ("jlclp" finds
/// "Jellyclip"). Mirrors HistoryTable.svelte, which applies it to file history.
fn term_matches(term: &str, field: &str) -> bool {
    if field.contains(term) {
        return true;
    }
    if term.len() <= 8 && term.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
        return field.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| {
            if word.len() < term.len() {
                return false;
            }
            let mut want = term.bytes().peekable();
            for b in word.bytes() {
                if want.peek() == Some(&b) {
                    want.next();
                }
            }
            want.peek().is_none()
        });
    }
    false
}

fn matches(terms: &[String], change: i64, user: &str, desc: &str) -> bool {
    let fields = [format!("@{change}"), user.to_lowercase(), desc.to_lowercase()];
    terms.iter().all(|t| fields.iter().any(|f| term_matches(t, f)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySearch {
    /// Matching changelists, newest first, at most `max`.
    pub rows: Vec<Record>,
    /// How many matched in all.
    pub matched: usize,
    /// How many changelists the path's history holds.
    pub total: usize,
    /// Said when nothing matched and the query names a changelist outside this
    /// history: where it was submitted instead.
    pub note: String,
}

/// Search a folder's whole history (see the module comment).
#[tauri::command]
pub async fn history_search(app: AppHandle, conn: P4Conn, path: String, query: String, max: usize) -> Result<HistorySearch, String> {
    let base = history_base(&path);
    let server = conn.port.clone();
    complete(&app, &conn, &server, &base).await?;
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let (a, s, b, t) = (app.clone(), server.clone(), base.clone(), terms.clone());
    let mut found = tauri::async_runtime::spawn_blocking(move || {
        let state = a.state::<AppState>();
        let db = state.db.lock().unwrap();
        search_stored(&db, &s, &b, &t, max).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if found.matched == 0 {
        found.note = elsewhere(&conn, &terms, &base).await;
    }
    Ok(found)
}

/// The stored history of `base` that matches every term, newest first.
fn search_stored(db: &Connection, server: &str, base: &str, terms: &[String], max: usize) -> rusqlite::Result<HistorySearch> {
    let mut stmt = db.prepare(
        "SELECT c.change, c.time, c.user, c.client, c.desc FROM hist_path p
         JOIN hist_change c ON c.server = p.server AND c.change = p.change
         WHERE p.server = ?1 AND p.path = ?2 ORDER BY p.change DESC",
    )?;
    let mut out = HistorySearch { rows: Vec::new(), matched: 0, total: 0, note: String::new() };
    let mut q = stmt.query(params![server, base])?;
    while let Some(r) = q.next()? {
        out.total += 1;
        let change: i64 = r.get(0)?;
        let user: String = r.get(2)?;
        let desc: String = r.get(4)?;
        if !matches(terms, change, &user, &desc) {
            continue;
        }
        out.matched += 1;
        if out.rows.len() < max {
            let mut rec = Record::new();
            rec.insert("change".into(), change.to_string().into());
            rec.insert("time".into(), r.get::<_, i64>(1)?.to_string().into());
            rec.insert("user".into(), user.into());
            rec.insert("client".into(), r.get::<_, String>(3)?.into());
            rec.insert("desc".into(), desc.into());
            rec.insert("status".into(), "submitted".into());
            out.rows.push(rec);
        }
    }
    Ok(out)
}

/// For a query that is a changelist number found nowhere in this history: where
/// that changelist was submitted, so the search can say so instead of nothing.
async fn elsewhere(conn: &P4Conn, terms: &[String], base: &str) -> String {
    let [term] = terms else { return String::new() };
    let digits = term.trim_start_matches('@');
    let Ok(n) = digits.parse::<u64>() else { return String::new() };
    let n = n.to_string();
    match run(conn.clone(), v(&["describe", "-s", "-m", "1", &n])).await {
        Ok(recs) => match recs.first() {
            Some(r) if text(r, "status") == "pending" => format!("@{n} is a pending changelist, not part of any history."),
            Some(r) if !text(r, "path").is_empty() => {
                format!("@{n} is not in {base}: it was submitted to {}.", text(r, "path"))
            }
            Some(_) => format!("@{n} is not in {base}."),
            None => format!("There is no changelist @{n} on this server."),
        },
        Err(_) => format!("There is no changelist @{n} on this server."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changelist_number_matches_its_own_row() {
        let t = vec!["@210516".to_string()];
        assert!(matches(&t, 210516, "jenkins", "merge"));
        assert!(!matches(&t, 210517, "jenkins", "merge"));
    }

    fn rec(change: &str, user: &str, desc: &str) -> Record {
        let mut r = Record::new();
        for (k, v) in [("change", change), ("time", "1"), ("user", user), ("client", "c"), ("desc", desc)] {
            r.insert(k.into(), v.into());
        }
        r
    }

    #[test]
    fn stored_history_is_searched_per_path() {
        let mut db = Connection::open_in_memory().unwrap();
        histdb_schema(&db).unwrap();
        remember(&mut db, "srv", "//a/...", &[rec("10", "ann", "Fix lumen"), rec("12", "bob", "Add sensor")]).unwrap();
        remember(&mut db, "srv", "//b/...", &[rec("11", "ann", "Fix lumen elsewhere")]).unwrap();
        let s = search_stored(&db, "srv", "//a/...", &["fix".into()], 10).unwrap();
        assert_eq!((s.matched, s.total), (1, 2));
        assert_eq!(s.rows[0].get("change").unwrap(), "10");
        let s = search_stored(&db, "srv", "//a/...", &["@11".into()], 10).unwrap();
        assert_eq!(s.matched, 0);
    }

    /// Against the real server: the full history of a big stream, stored and searched.
    /// `cargo test --lib histdb -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_history_round_trip() {
        let conn: P4Conn = serde_json::from_value(serde_json::json!({ "port": "ssl:perforce.sloclap.net:1666" })).unwrap();
        let mut db = Connection::open_in_memory().unwrap();
        histdb_schema(&db).unwrap();
        let base = "//Rematch/Dev/Main/...";
        let t = Instant::now();
        let recs = crate::p4::run(&conn, &["changes", "-l", "-s", "submitted", base]).unwrap();
        let fetched = t.elapsed();
        let t = Instant::now();
        remember(&mut db, "s", base, &recs).unwrap();
        let stored = t.elapsed();
        for q in ["@210516", "@223393", "lumen", "jenkins robomerge"] {
            let t = Instant::now();
            let terms: Vec<String> = q.split_whitespace().map(|t| t.to_lowercase()).collect();
            let s = search_stored(&db, "s", base, &terms, 500).unwrap();
            println!("{q:>20}: {} of {} in {:?}", s.matched, s.total, t.elapsed());
        }
        println!("fetch {} changelists {fetched:?}, store {stored:?}", recs.len());
    }

    #[test]
    fn short_terms_match_as_a_subsequence_of_one_word() {
        assert!(matches(&["jlclp".into()], 1, "u", "Fix the Jellyclip spawn"));
        assert!(!matches(&["jlclp".into()], 1, "u", "jelly clip"));
        assert!(matches(&["leo".into(), "lumen".into()], 1, "leo-paul.couturier", "Lumen Scalability"));
    }
}
