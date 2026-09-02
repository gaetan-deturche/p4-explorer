//! Stashes: a change set off to one side, kept in the app's own database.
//!
//! Perforce has no stash. The nearest thing is a shelf, which costs a changelist
//! and a round trip to the depot, and is bound to the workspace that made it.
//! What this stores instead is a PATCH — the same bytes `export_patch` writes to
//! disk — so the whole thing is a blob and a bit of metadata in SQLite.
//!
//! That choice is what makes a stash portable. A patch names DEPOT paths, and
//! `apply_patch` resolves each one with `p4 where` on the connection it is
//! applied with (see `patch::resolve_target`), so where a file lands is decided
//! by the workspace applying it, not the one that made it. Applying a stash in
//! another workspace is therefore the ordinary apply, with nothing special about
//! it. The database is per MACHINE, so stashes are shared across every workspace
//! on this one — and no further: nothing here reaches the depot.
//!
//! A stash is a COPY. Taking one leaves your files exactly as they are, so
//! nothing you have can be lost by making one; clearing the workspace afterwards
//! stays a separate, deliberate step.

use crate::index::AppState;
use crate::p4::{self, P4Conn};
use rusqlite::Connection;

/// A stash as the list shows it. `patch` is deliberately absent — the list is
/// read on every visit to the tab and the blob is only needed to apply one.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashRow {
    pub id: i64,
    pub name: String,
    /// Unix seconds.
    pub created: i64,
    /// Where it was taken: enough to tell two workspaces' stashes apart in a
    /// list that deliberately mixes them.
    pub port: String,
    pub client: String,
    pub user: String,
    pub stream: String,
    pub files: Vec<super::diff::PatchedFile>,
    /// Files the patch could not carry (deletes), recorded so the list can say
    /// so rather than quietly holding less than it appears to.
    pub skipped: Vec<String>,
    /// Size of the patch, for the list.
    pub bytes: i64,
}

pub(crate) fn stash_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS stash(
             id      INTEGER PRIMARY KEY AUTOINCREMENT,
             name    TEXT    NOT NULL,
             created INTEGER NOT NULL,
             port    TEXT    NOT NULL,
             client  TEXT    NOT NULL,
             user    TEXT    NOT NULL,
             stream  TEXT    NOT NULL,
             files   TEXT    NOT NULL,
             skipped TEXT    NOT NULL,
             patch   TEXT    NOT NULL
         );",
    )
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The stream this client is on ("" for a classic client) — recorded so a stash
/// taken on one branch is recognisable as such when it is applied on another.
fn stream_of(conn: &P4Conn) -> String {
    p4::run(conn, &["client", "-o"])
        .unwrap_or_default()
        .first()
        .and_then(|r| r.get("Stream").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default()
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<StashRow> {
    let files: String = r.get("files")?;
    let skipped: String = r.get("skipped")?;
    Ok(StashRow {
        id: r.get("id")?,
        name: r.get("name")?,
        created: r.get("created")?,
        port: r.get("port")?,
        client: r.get("client")?,
        user: r.get("user")?,
        stream: r.get("stream")?,
        files: serde_json::from_str(&files).unwrap_or_default(),
        skipped: serde_json::from_str(&skipped).unwrap_or_default(),
        bytes: r.get("bytes")?,
    })
}

/// Take a stash from `files`, or from every opened file of `change`.
///
/// Nothing is reverted: see the module note. The patch is generated exactly as
/// an export would generate it, and stored whole.
#[tauri::command]
pub async fn stash_save(
    state: tauri::State<'_, AppState>,
    conn: P4Conn,
    name: String,
    change: String,
    files: Vec<String>,
) -> Result<StashRow, String> {
    let built = tauri::async_runtime::spawn_blocking({
        let conn = conn.clone();
        move || {
            let (patch, carried, skipped) = super::diff::build_patch(&conn, &change, files)?;
            Ok::<_, String>((patch, carried, skipped, stream_of(&conn)))
        }
    })
    .await
    .map_err(|e| format!("stash task failed: {e}"))??;
    let (patch, carried, skipped, stream) = built;

    let created = now();
    let bytes = patch.len() as i64;
    let db = state.cache_db.lock().unwrap();
    db.execute(
        "INSERT INTO stash(name, created, port, client, user, stream, files, skipped, patch)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            name.trim(),
            created,
            conn.port,
            conn.client,
            conn.user,
            stream,
            serde_json::to_string(&carried).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".into()),
            patch,
        ],
    )
    .map_err(|e| e.to_string())?;
    // The row, not just its id: a caller that is about to clear the workspace
    // has to know exactly which files the patch CARRIED — reverting one it could
    // not carry would throw away work the stash cannot give back.
    Ok(StashRow {
        id: db.last_insert_rowid(),
        name: name.trim().to_string(),
        created,
        port: conn.port.clone(),
        client: conn.client.clone(),
        user: conn.user.clone(),
        stream,
        files: carried,
        skipped,
        bytes,
    })
}

/// Every stash on this machine, newest first — from every workspace, which is
/// the point of them.
#[tauri::command]
pub async fn stash_list(state: tauri::State<'_, AppState>) -> Result<Vec<StashRow>, String> {
    let db = state.cache_db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, name, created, port, client, user, stream, files, skipped,
                    LENGTH(patch) AS bytes
             FROM stash ORDER BY created DESC, id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_from).map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Write a stash's patch to a temp file and return the path.
///
/// From here on it is the ordinary apply-a-patch flow — same preview, same
/// end-state choice, same per-hunk resolve for whatever does not fit — which is
/// also how a review is applied. The file is a materialisation of the row, not a
/// second copy of the truth: it is rewritten from the database every time.
#[tauri::command]
pub async fn stash_patch_file(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<String, String> {
    let patch: String = {
        let db = state.cache_db.lock().unwrap();
        db.query_row("SELECT patch FROM stash WHERE id=?1", [id], |r| r.get(0))
            .map_err(|_| "That stash is gone.".to_string())?
    };
    let path = std::env::temp_dir().join(format!("auger-stash-{id}.patch"));
    std::fs::write(&path, patch).map_err(|e| format!("cannot write the patch: {e}"))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn stash_delete(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    let db = state.cache_db.lock().unwrap();
    db.execute("DELETE FROM stash WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn stash_rename(
    state: tauri::State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let db = state.cache_db.lock().unwrap();
    db.execute(
        "UPDATE stash SET name=?2 WHERE id=?1",
        rusqlite::params![id, name.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The patch itself, for reading it in a window or saving it out.
#[tauri::command]
pub async fn stash_patch(state: tauri::State<'_, AppState>, id: i64) -> Result<String, String> {
    let db = state.cache_db.lock().unwrap();
    db.query_row("SELECT patch FROM stash WHERE id=?1", [id], |r| r.get(0))
        .map_err(|_| "That stash is gone.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> Connection {
        let db = Connection::open_in_memory().expect("in-memory sqlite");
        stash_schema(&db).expect("schema");
        db
    }

    #[test]
    fn the_schema_round_trips_a_stash() {
        let db = scratch();
        db.execute(
            "INSERT INTO stash(name, created, port, client, user, stream, files, skipped, patch)
             VALUES('wip', 100, 'ssl:p:1666', 'clientA', 'me', '//d/Main', '[]', '[]', 'PATCH')",
            [],
        )
        .unwrap();
        let mut stmt = db
            .prepare(
                "SELECT id, name, created, port, client, user, stream, files, skipped,
                        LENGTH(patch) AS bytes FROM stash",
            )
            .unwrap();
        let rows: Vec<StashRow> = stmt
            .query_map([], row_from)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "wip");
        assert_eq!(rows[0].client, "clientA");
        assert_eq!(rows[0].bytes, 5);
        assert!(rows[0].files.is_empty());
    }

    /// A row whose metadata cannot be parsed must still list: the patch is the
    /// content, and losing a stash because its file list is malformed would be
    /// the worst possible trade.
    #[test]
    fn broken_metadata_still_lists() {
        let db = scratch();
        db.execute(
            "INSERT INTO stash(name, created, port, client, user, stream, files, skipped, patch)
             VALUES('wip', 1, '', '', '', '', 'not json', '{{', 'PATCH')",
            [],
        )
        .unwrap();
        let mut stmt = db
            .prepare(
                "SELECT id, name, created, port, client, user, stream, files, skipped,
                        LENGTH(patch) AS bytes FROM stash",
            )
            .unwrap();
        let rows: Vec<StashRow> = stmt
            .query_map([], row_from)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].files.is_empty());
        assert!(rows[0].skipped.is_empty());
    }
}
