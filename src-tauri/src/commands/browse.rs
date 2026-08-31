//! Read-only browsing: info, workspaces, depot tree, history, search.

use super::{run, v, Res};
use crate::p4::{self, P4Conn};

/// `p4 info` — server / client / user context.
#[tauri::command]
pub async fn p4_info(conn: P4Conn) -> Res {
    run(conn, v(&["info"])).await
}

/// Client workspaces, optionally filtered to `conn.user`.
#[tauri::command]
pub async fn p4_clients(conn: P4Conn) -> Res {
    let args = if conn.user.is_empty() {
        v(&["clients"])
    } else {
        v(&["clients", "-u", &conn.user])
    };
    run(conn, args).await
}

/// One workspace's spec, as the manage dialog shows it.
#[derive(serde::Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClientSpec {
    pub client: String,
    pub owner: String,
    pub host: String,
    pub root: String,
    pub stream: String,
    pub description: String,
    pub options: String,
    pub submit_options: String,
    pub line_end: String,
    pub access: String,
    pub update: String,
}

/// Read a workspace spec (`client -o`, tagged).
#[tauri::command]
pub async fn p4_client_spec(conn: P4Conn, client: String) -> Result<ClientSpec, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let recs = p4::run(&conn, &["client", "-o", &client])?;
        let r = recs.first().ok_or("p4 returned no spec for that workspace")?;
        let get = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        Ok(ClientSpec {
            client: get("Client"),
            owner: get("Owner"),
            host: get("Host"),
            root: get("Root"),
            stream: get("Stream"),
            description: get("Description"),
            options: get("Options"),
            submit_options: get("SubmitOptions"),
            line_end: get("LineEnd"),
            access: get("Access"),
            update: get("Update"),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A stream client's View is derived from its Stream, so the template's View is
/// dropped before writing back — `client -i` regenerates it. Leaving it in makes
/// p4 reject a spec whose View no longer matches the stream.
fn drop_view(form: &str) -> String {
    match form.find("\nView:") {
        Some(i) => format!("{}\n", &form[..i]),
        None => form.to_string(),
    }
}

/// Change an existing workspace's root, stream, host or description.
///
/// The edit goes through `p4 --field`, which patches p4's own form: every field
/// the dialog does not touch (Options, SubmitOptions, LineEnd, the writable
/// entries, the View of a non-stream client) survives untouched. Rebuilding a
/// spec from parsed fields would quietly drop whatever this app does not know
/// about.
#[tauri::command]
pub async fn p4_client_save(
    conn: P4Conn,
    client: String,
    root: String,
    stream: String,
    host: String,
    description: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut args: Vec<String> = Vec::new();
        for (field, value) in [
            ("Root", &root),
            ("Host", &host),
            ("Description", &description),
        ] {
            args.push("--field".into());
            args.push(format!("{field}={}", value.trim()));
        }
        // An empty stream would mean "make this a classic client", which is not
        // what an empty box in the dialog means — it is left alone instead.
        if !stream.trim().is_empty() {
            args.push("--field".into());
            args.push(format!("Stream={}", stream.trim()));
        }
        args.extend(["client".into(), "-o".into(), client.clone()]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let form = p4::run_raw(&conn, &refs)?;
        let form = if stream.trim().is_empty() { form } else { drop_view(&form) };
        p4::run_raw_stdin(&conn, &["client", "-i"], &form)?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Delete a workspace (`client -d`).
///
/// p4 refuses at severity 3 with the reason ("has files opened. To delete the
/// client, revert any opened files and delete any pending changes first."), which
/// reaches the caller as the error text — no forcing, and no guessing why.
#[tauri::command]
pub async fn p4_client_delete(conn: P4Conn, client: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        p4::run_strict(&conn, &["client", "-d", &client]).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Rename a workspace (`renameclient`), which p4 allows the client's OWNER —
/// admin is not needed (checked against this server).
///
/// p4 moves the pending changes, shelves, opened files and have-list with it. It
/// refuses for a client with opened streams or promoted shelves, and for a target
/// name that already exists; in each case its own text is what comes back.
#[tauri::command]
pub async fn p4_client_rename(conn: P4Conn, from: String, to: String) -> Result<(), String> {
    let to = to.trim().to_string();
    if to.is_empty() {
        return Err("A new name is required.".into());
    }
    if to == from {
        return Err("That is already its name.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let from_arg = format!("--from={from}");
        let to_arg = format!("--to={to}");
        p4::run_strict(&conn, &["renameclient", &from_arg, &to_arg]).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Create a new stream workspace bound to this machine: sets Root + Stream +
/// Host (this host) and drops the View so p4 generates it from the stream.
#[tauri::command]
pub async fn p4_new_client(
    conn: P4Conn,
    name: String,
    root: String,
    stream: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Host = this machine, so the new client is bound here (gets the ● mark).
        let host = p4::run(&conn, &["info"])
            .ok()
            .and_then(|recs| {
                recs.into_iter()
                    .next()
                    .and_then(|r| r.get("clientHost").and_then(|v| v.as_str()).map(String::from))
            })
            .unwrap_or_default();
        let mut args: Vec<String> = vec![
            "--field".into(),
            format!("Root={root}"),
            "--field".into(),
            format!("Host={host}"),
        ];
        if !stream.is_empty() {
            args.push("--field".into());
            args.push(format!("Stream={stream}"));
        }
        args.extend(["client".into(), "-o".into(), name.clone()]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let form = p4::run_raw(&conn, &refs)?;
        // For a stream client the View is derived from the stream — drop the
        // template View so `client -i` regenerates it.
        let form = if stream.is_empty() {
            form
        } else {
            match form.find("\nView:") {
                Some(i) => format!("{}\n", &form[..i]),
                None => form,
            }
        };
        p4::run_raw_stdin(&conn, &["client", "-i"], &form)?;
        Ok(())
    })
    .await
    .map_err(|e| format!("new-client task failed: {e}"))?
}

/// Sub-directories of a depot path (`p4 dirs <path>/*`).
#[tauri::command]
pub async fn p4_dirs(conn: P4Conn, path: String) -> Res {
    let pattern = format!("{}/*", path.trim_end_matches('/'));
    run(conn, v(&["dirs", &pattern])).await
}

/// Files directly under a depot path, with have/head status for the synced
/// marker (`p4 fstat <path>/*`).
#[tauri::command]
pub async fn p4_files(conn: P4Conn, path: String) -> Res {
    let pattern = format!("{}/*", path.trim_end_matches('/'));
    run(
        conn,
        v(&[
            "fstat",
            "-T",
            "depotFile,headRev,haveRev,headAction,headChange,headType,headTime",
            &pattern,
        ]),
    )
    .await
}

/// Submitted changelists affecting a depot path (newest first). `before`, when
/// set, pages backward: only changelists <= that number are returned, so the
/// caller can fetch history in chunks (`before = smallest_seen - 1`).
#[tauri::command]
pub async fn p4_changes(conn: P4Conn, path: String, max: u32, before: Option<u32>) -> Res {
    let max = max.to_string();
    let base = if path.ends_with("...") || path.contains('*') {
        path
    } else {
        format!("{}/...", path.trim_end_matches('/'))
    };
    let spec = match before {
        Some(b) => format!("{base}@{b}"),
        None => base,
    };
    run(conn, v(&["changes", "-l", "-m", &max, "-s", "submitted", &spec])).await
}

/// The changelist a depot path is currently synced to (highest CL among the
/// have revisions). One record, or empty if nothing is synced.
#[tauri::command]
pub async fn p4_have_change(conn: P4Conn, path: String) -> Res {
    let spec = format!("{}/...#have", path.trim_end_matches('/'));
    run(conn, v(&["changes", "-m", "1", &spec])).await
}

/// Submitted changes for an EXACT spec, verbatim (no `/...` appended) — e.g.
/// `//depot/file.cpp#have` for a file's synced changelist, or the file itself
/// for every change that touched it. Appending `/...` to a file path matches
/// nothing, which is why this exists alongside `p4_changes`.
#[tauri::command]
pub async fn p4_changes_exact(conn: P4Conn, spec: String, max: u32) -> Res {
    let max = max.clamp(1, 1000).to_string();
    run(conn, v(&["changes", "-l", "-m", &max, "-s", "submitted", &spec])).await
}

/// Full description of a changelist, exploded to one row per affected file.
#[tauri::command]
pub async fn p4_describe(conn: P4Conn, change: String) -> Res {
    let recs = run(conn, v(&["describe", "-s", &change])).await?;
    let mut out = Vec::new();
    for rec in &recs {
        let rows = p4::explode_indexed(rec, "depotFile");
        if rows.is_empty() {
            out.push(rec.clone()); // empty changelist: keep the header row
        } else {
            out.extend(rows);
        }
    }
    Ok(out)
}

/// Revision history of a single file, one row per revision (newest first).
///
/// `follow` adds `-i`, which walks back through the branch the file was created
/// from. It is what makes a migrated depot readable: our own move to
/// //CuriosityP4/Dev/Main branched every file rather than re-adding it, so
/// without `-i` a file's history begins at "Initial copy of the game" and blame
/// credits the whole file to whoever ran the migration.
///
/// `filelog -i` answers with one record per file in the lineage, each carrying
/// its own `depotFile`, and `explode_indexed` copies that onto every row - so a
/// pre-migration row names the OLD path, and acting on it (diff, blame) reaches
/// the file those revision numbers actually belong to.
#[tauri::command]
pub async fn p4_filelog(conn: P4Conn, file: String, max: u32, follow: Option<bool>) -> Res {
    let max = max.to_string();
    let mut args: Vec<&str> = vec!["filelog", "-l"];
    if follow.unwrap_or(false) {
        args.push("-i");
    }
    args.extend(["-m", &max, &file]);
    let recs = run(conn, args.iter().map(|s| s.to_string()).collect()).await?;
    let mut out = Vec::new();
    for rec in &recs {
        out.extend(p4::explode_indexed(rec, "rev"));
    }
    Ok(out)
}

/// fstat for a single file (have/head revisions).
#[tauri::command]
pub async fn p4_fstat(conn: P4Conn, file: String) -> Res {
    run(conn, v(&["fstat", &file])).await
}

/// Depot-wide filename search under `root`: `p4 files //root/.../*term*`.
/// Case-sensitive (the server's case handling). Capped at `max` results.
#[tauri::command]
pub async fn p4_search(conn: P4Conn, root: String, term: String, max: u32) -> Res {
    let pattern = format!("{}/.../*{}*", root.trim_end_matches('/'), term);
    run(conn, v(&["files", "-m", &max.to_string(), &pattern])).await
}

/// All depots on the server (`p4 depots`).
#[tauri::command]
pub async fn p4_depots(conn: P4Conn) -> Res {
    run(conn, v(&["depots"])).await
}

/// A local-filesystem directory listing (names only).
#[derive(serde::Serialize)]
pub struct LocalDir {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
}

/// List a local directory via the OS filesystem. Used as an instant provisional
/// view of a workspace folder while the (cold, slow) `p4 dirs`/`fstat` scan runs
/// — the client root maps depot paths to local files, so this shows the synced
/// contents immediately, then gets replaced by the authoritative depot listing.
#[tauri::command]
pub async fn list_local_dir(path: String) -> Result<LocalDir, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => dirs.push(name),
                Ok(_) => files.push(name),
                Err(_) => {}
            }
        }
        dirs.sort_unstable();
        files.sort_unstable();
        Ok(LocalDir { dirs, files })
    })
    .await
    .map_err(|e| format!("list_local_dir task failed: {e}"))?
}

#[cfg(test)]
mod client_spec_tests {
    use super::drop_view;

    #[test]
    fn a_stream_clients_view_is_dropped() {
        let form = "Client:\tws\nStream:\t//d/main\nView:\n\t//d/main/... //ws/...\n";
        let out = drop_view(form);
        assert!(!out.contains("View:"));
        assert!(out.contains("Stream:"), "everything before the View survives");
        assert!(out.ends_with('\n'), "p4 wants a trailing newline");
    }

    #[test]
    fn a_form_without_a_view_is_untouched() {
        let form = "Client:\tws\nRoot:\tC:\\x\n";
        assert_eq!(drop_view(form), form);
    }
}
