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
///
/// Falls back to the client's own view when p4d refuses the listing: see
/// `dirs_from_view`.
#[tauri::command]
pub async fn p4_dirs(conn: P4Conn, path: String) -> Res {
    let pattern = format!("{}/*", path.trim_end_matches('/'));
    match run(conn.clone(), v(&["dirs", &pattern])).await {
        Err(e) if e.contains("too twisted") => dirs_from_view(conn, &path).await.map_err(|_| e),
        other => other,
    }
}

/// One line of a client view, split into its two sides.
#[derive(Debug, Clone, PartialEq)]
struct ViewLine {
    exclude: bool,
    /// An overlay (`+`) line adds to what came before instead of replacing it.
    overlay: bool,
    depot: String,
    /// The client side with the leading `//<client>/` removed.
    client: String,
}

/// The view of `client`, in order.
fn client_view(conn: &P4Conn, client: &str) -> Result<Vec<ViewLine>, String> {
    let form = p4::run_raw(conn, &["client", "-o", client])?;
    let mut out = Vec::new();
    let mut in_view = false;
    for line in form.lines() {
        if line.starts_with("View:") {
            in_view = true;
            continue;
        }
        if !in_view {
            continue;
        }
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        if !s.starts_with("//") && !s.starts_with("-//") && !s.starts_with("+//") {
            break; // a following field: the view is over
        }
        let Some((depot, client_side)) = s.split_once(char::is_whitespace) else { continue };
        let prefix = format!("//{client}/");
        let Some(rest) = client_side.trim().strip_prefix(&prefix) else { continue };
        out.push(ViewLine {
            exclude: depot.starts_with('-'),
            overlay: depot.starts_with('+'),
            depot: depot.trim_start_matches(['-', '+']).to_string(),
            client: rest.to_string(),
        });
    }
    Ok(out)
}

/// The components of a client-side pattern before its first wildcard.
fn fixed_head(pattern: &str) -> Vec<&str> {
    pattern
        .split('/')
        .take_while(|c| !c.contains('*') && !c.contains("..."))
        .collect()
}

/// Does this client pattern cover EVERYTHING under `dir`?
///
/// Only a pattern ending in a bare `...` can: `Binaries/...` covers the whole
/// directory, `.../Intermediate/...` covers one at any depth, and `....pyc`
/// covers only some files in it — which is why p4 keeps a directory whose
/// contents are partly excluded, and drops one whose contents are all excluded.
fn covers_all_of(pattern: &str, dir: &[&str]) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    fn walk(pat: &[&str], dir: &[&str]) -> bool {
        // The pattern is consumed exactly when a bare `...` is all that is left.
        if dir.is_empty() {
            return pat == ["..."];
        }
        match pat.first() {
            None => false,
            Some(&"...") => {
                // `...` spans any number of components, including none.
                walk(&pat[1..], dir) || walk(pat, &dir[1..])
            }
            Some(&c) => {
                if c.contains('*') || c.contains("...") {
                    // A wildcard within a component matches this one component.
                    walk(&pat[1..], &dir[1..])
                } else if c == dir[0] {
                    walk(&pat[1..], &dir[1..])
                } else {
                    false
                }
            }
        }
    }
    walk(&pat, dir)
}

/// Is this client directory excluded from the workspace?
///
/// p4's rule is that the last matching line wins, so a later include re-admits
/// what an earlier exclusion removed. Only lines that cover the WHOLE directory
/// count: a line excluding some files inside it leaves the directory itself.
fn dir_excluded(view: &[ViewLine], dir: &[&str]) -> bool {
    let mut excluded = false;
    for line in view {
        if covers_all_of(&line.client, dir) {
            excluded = line.exclude;
        }
    }
    excluded
}

/// A directory the view puts at the level being listed, and the depot path to
/// report it under.
#[derive(Debug, Clone, PartialEq)]
struct ViewDir {
    name: String,
    depot: String,
}

/// What the view says about one directory query, without asking p4d to invert
/// the whole map.
///
/// `prefix` is the client path being listed, relative to the client root ("" for
/// the root itself). Two things can put a directory at that level:
///
///   * a line whose CLIENT side is fixed past that level — `Rig/Shared/...`
///     names `Rig` at the root, whatever its depot side looks like. This is the
///     half p4d cannot express as a depot pattern, and the reason it refuses the
///     whole listing.
///   * a line whose wildcard reaches that level — `//depot/main/...` mapped to
///     `...` means the real depot directories under `//depot/main/<prefix>` are
///     the answer, which p4d lists happily when asked in DEPOT syntax.
///
/// Returns (directories named by the mapping, depot paths to list). Later lines
/// replace earlier ones for the same subtree, as p4 does, unless they are
/// overlays.
fn view_query(view: &[ViewLine], prefix: &str) -> (Vec<ViewDir>, Vec<String>) {
    let want: Vec<&str> = if prefix.is_empty() { Vec::new() } else { prefix.split('/').collect() };
    let mut named: Vec<ViewDir> = Vec::new();
    let mut roots: Vec<String> = Vec::new();

    for line in view.iter().filter(|l| !l.exclude) {
        let comps: Vec<&str> = line.client.split('/').collect();
        let fixed = fixed_head(&line.client);
        if !want.iter().zip(fixed.iter()).all(|(a, b)| a == b) {
            continue; // this line has nothing to do with the directory asked for
        }
        let depot_fixed: Vec<&str> = fixed_head(&line.depot);
        if fixed.len() > want.len() {
            let name = fixed[want.len()].to_string();
            // When the named directory IS this line's mapping point, p4 reports
            // the grafted depot's own path (measured: `Shared` under //client/Rig
            // comes back as //Rig/main). One level higher there is no depot path
            // to give — precisely the case p4d refuses — so the caller supplies
            // one from the mapping it is browsing through.
            let depot = if fixed.len() == want.len() + 1 { depot_fixed.join("/") } else { String::new() };
            named.push(ViewDir { name, depot });
        } else if comps.len() > fixed.len() {
            let mut base = depot_fixed.join("/");
            for extra in want.iter().skip(fixed.len()) {
                base.push('/');
                base.push_str(extra);
            }
            if line.overlay {
                roots.push(base);
            } else {
                roots = vec![base]; // a later line supersedes an earlier one
            }
        }
    }
    named.sort_by(|a, b| a.name.cmp(&b.name));
    named.dedup_by(|a, b| a.name == b.name);
    roots.dedup();
    (named, roots)
}

/// List a client directory that p4d refuses to list in one go.
///
/// Measured on this server: 89 of 400 client specs cannot answer
/// `p4 dirs //client/*` at all ("Client map too twisted for directory list",
/// MsgDm 439 / EV_TOOBIG) because a second depot is grafted two or more levels
/// into the client — `//Rig/main/... //client/Rig/Shared/...`. Every deeper
/// level answers fine, and `p4 dirs` in DEPOT syntax always does, so the listing
/// is assembled from the view instead: the directories a mapping names outright,
/// plus the real ones under each depot root the query reaches, minus the ones
/// the view excludes.
///
/// Answers in depot syntax, like `p4 dirs` itself, so callers need no special
/// case: what they get back is what they translate and descend into, and every
/// deeper level is one p4d can answer for itself.
async fn dirs_from_view(conn: P4Conn, path: &str) -> Res {
    let client = conn.client.clone();
    if client.is_empty() {
        return Err("no client".into());
    }
    let prefix = path
        .trim_end_matches('/')
        .strip_prefix(&format!("//{client}"))
        .ok_or("not a client path")?
        .trim_start_matches('/')
        .to_string();

    let view = {
        let conn = conn.clone();
        tauri::async_runtime::spawn_blocking(move || client_view(&conn, &conn.client))
            .await
            .map_err(|e| format!("view task failed: {e}"))??
    };
    let (named, roots) = view_query(&view, &prefix);

    let mut out: Vec<p4::Record> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut keep = |out: &mut Vec<p4::Record>, name: &str, depot: &str| {
        if name.is_empty() || !seen.insert(name.to_string()) {
            return;
        }
        let mut dir: Vec<&str> = if prefix.is_empty() { Vec::new() } else { prefix.split('/').collect() };
        dir.push(name);
        if dir_excluded(&view, &dir) {
            return; // p4 omits a directory whose every file the view excludes
        }
        let mut rec = p4::Record::new();
        rec.insert("dir".to_string(), serde_json::Value::String(depot.to_string()));
        out.push(rec);
    };

    for root in &roots {
        let Ok(recs) = run(conn.clone(), v(&["dirs", &format!("{root}/*")])).await else { continue };
        for r in recs {
            let dir = r.get("dir").and_then(|d| d.as_str()).unwrap_or("").to_string();
            let name = dir.rsplit('/').next().unwrap_or("").to_string();
            keep(&mut out, &name, &dir);
        }
    }
    // The mapping's own directories last: a name the depot side already supplied
    // keeps the depot's path, which is what p4 would have returned.
    let fallback_base = roots.first().cloned().unwrap_or_else(|| format!("//{client}"));
    for d in named {
        let depot = if d.depot.is_empty() {
            format!("{fallback_base}/{}", d.name)
        } else {
            d.depot.clone()
        };
        keep(&mut out, &d.name, &depot);
    }
    out.sort_by(|a, b| {
        let k = |r: &p4::Record| r.get("dir").and_then(|d| d.as_str()).unwrap_or("").to_lowercase();
        k(a).cmp(&k(b))
    });
    Ok(out)
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

#[cfg(test)]
mod twisted_tests {
    use super::{covers_all_of, dir_excluded, view_query, ViewLine};

    fn v(depot: &str, client: &str) -> ViewLine {
        ViewLine { exclude: false, overlay: false, depot: depot.into(), client: client.into() }
    }
    fn x(depot: &str, client: &str) -> ViewLine {
        ViewLine { exclude: true, overlay: false, depot: depot.into(), client: client.into() }
    }

    /// The real CurioData view, which p4d cannot list at the root.
    fn curio_data() -> Vec<ViewLine> {
        vec![
            v("//Curiosity-Data/main/...", "..."),
            v("//Rig/main/...", "Rig/Shared/..."),
            x("//Curiosity-Data/....pyc", "....pyc"),
            x("//Curiosity-Data/.../.mayaSwatches/...", ".../.mayaSwatches/..."),
        ]
    }

    #[test]
    fn the_root_is_the_depot_root_plus_what_the_graft_names() {
        let (named, roots) = view_query(&curio_data(), "");
        // The graft names `Rig` outright — the level p4d cannot express, since
        // there is no depot directory that stands for it.
        assert_eq!(named.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["Rig"]);
        assert_eq!(named[0].depot, "", "no depot path exists for this level");
        assert_eq!(roots, vec!["//Curiosity-Data/main"]);
    }

    #[test]
    fn at_the_graft_point_the_grafted_depot_is_named() {
        // Measured: `dirs //client/Rig/*` reports `Shared` as //Rig/main.
        let (named, roots) = view_query(&curio_data(), "Rig");
        assert_eq!(named.len(), 1);
        assert_eq!((named[0].name.as_str(), named[0].depot.as_str()), ("Shared", "//Rig/main"));
        assert_eq!(roots, vec!["//Curiosity-Data/main/Rig"]);
    }

    #[test]
    fn a_later_line_supersedes_an_earlier_one() {
        // Measured: `dirs //client/Rig/Shared/*` answers from //Rig/main alone,
        // though the root mapping covers that path too.
        let (named, roots) = view_query(&curio_data(), "Rig/Shared");
        assert!(named.is_empty());
        assert_eq!(roots, vec!["//Rig/main"]);
    }

    #[test]
    fn a_level_no_mapping_names_asks_only_the_depot_that_holds_it() {
        let (named, roots) = view_query(&curio_data(), "Characters/01_Creatures");
        assert!(named.is_empty());
        assert_eq!(roots, vec!["//Curiosity-Data/main/Characters/01_Creatures"]);
    }

    #[test]
    fn the_rematch_shape_names_its_graft_too() {
        let view = vec![
            v("//Rematch-Engine/Dev/Main/...", "..."),
            v("//Rematch/Dev/Main/...", "Games/Runtime/..."),
        ];
        let (named, roots) = view_query(&view, "");
        assert_eq!(named[0].name, "Games");
        assert_eq!(roots, vec!["//Rematch-Engine/Dev/Main"]);
        let (named, _) = view_query(&view, "Games");
        assert_eq!((named[0].name.as_str(), named[0].depot.as_str()), ("Runtime", "//Rematch/Dev/Main"));
    }

    #[test]
    fn a_wholly_excluded_directory_is_dropped() {
        // Measured on the _1973 client: Binaries, Saved, Intermediate,
        // DerivedDataCache and Restricted are all absent from `dirs //client/*`,
        // while Build — excluded nowhere — is listed.
        let view = vec![
            v("//Curiosity/main/...", "..."),
            x("//Curiosity/main/Binaries/...", "Binaries/..."),
            x("//Curiosity/.../Intermediate/...", ".../Intermediate/..."),
            x("//Curiosity/....p4config", "....p4config"),
        ];
        assert!(dir_excluded(&view, &["Binaries"]));
        assert!(dir_excluded(&view, &["Intermediate"]), "at the root");
        assert!(dir_excluded(&view, &["Engine", "Source", "Intermediate"]), "at any depth");
        assert!(!dir_excluded(&view, &["Build"]));
        // A rule that excludes SOME files leaves the directory itself.
        assert!(!dir_excluded(&view, &["Config"]));
    }

    #[test]
    fn a_later_include_puts_a_directory_back() {
        let view = vec![
            v("//d/main/...", "..."),
            x("//d/main/Engine/...", "Engine/..."),
            v("//d/main/Engine/...", "Engine/..."),
        ];
        assert!(!dir_excluded(&view, &["Engine"]));
    }

    #[test]
    fn only_a_trailing_ellipsis_covers_a_whole_directory() {
        assert!(covers_all_of("Binaries/...", &["Binaries"]));
        assert!(covers_all_of(".../obj/...", &["a", "b", "obj"]));
        assert!(!covers_all_of("....pyc", &["Scripts"]), "a file rule, not a directory one");
        assert!(!covers_all_of("Engine/Saved/...", &["Engine"]), "only part of Engine");
        assert!(!covers_all_of("Binaries/...", &["Build"]));
    }
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
