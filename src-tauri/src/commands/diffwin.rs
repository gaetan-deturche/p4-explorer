//! In-app diff window: materialize the two sides of a diff to files on disk
//! (`p4 print` for server revisions; the live workspace file for "local") and
//! open a separate webview window on the `/diff` route that renders them
//! side-by-side. The window reads the files back via `read_text_file`.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use crate::p4::{self, P4Conn};

/// The two sides of a diff, ready to open (paths on disk + display labels).
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiffPair {
    pub left: String,
    pub right: String,
    pub left_label: String,
    pub right_label: String,
    pub title: String,
    /// True when `right` is the workspace file itself rather than a printed
    /// revision — the only case where editing it in the window means anything.
    #[serde(default)]
    pub right_editable: bool,
    /// Set when the file still needs a `p4 resolve`: says which depot revision is
    /// NOT in the file yet. Empty otherwise. The diff window turns it into a
    /// warning, because in that state a plain diff is actively misleading.
    #[serde(default)]
    pub unresolved_note: String,
}

fn base_name(depot_file: &str) -> &str {
    depot_file.rsplit('/').next().unwrap_or("file")
}

/// Print `spec` to a temp file named for the diff window; empty `spec` creates
/// an EMPTY temp file (the left side of an added file's diff). Names are
/// uniquified per call — a running editor holds previously-diffed files open
/// (locked and cached), so reusing a name would collide or show stale content —
/// and the read-only attribute `p4 print` sets is cleared (UE's -diff copies
/// the file, and the copy inherits the attribute).
fn print_side(conn: &P4Conn, spec: &str, file_tag: &str) -> Result<String, String> {
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("p4gui_diff_{uniq}_{file_tag}"));
    let tmp_s = tmp.to_str().ok_or("bad temp path")?.to_string();
    if spec.is_empty() {
        std::fs::write(&tmp, "").map_err(|e| e.to_string())?;
        return Ok(tmp_s);
    }
    p4::run_raw(conn, &["print", "-q", "-o", &tmp_s, spec])?;
    if !tmp.is_file() {
        return Err(format!("p4 print produced no file for {spec}"));
    }
    if let Ok(md) = std::fs::metadata(&tmp) {
        let mut perm = md.permissions();
        if perm.readonly() {
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&tmp, perm);
        }
    }
    Ok(tmp_s)
}

fn sane(s: &str) -> String {
    s.replace(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '_' && c != '#', "_")
}

/// Temp-file tag carrying a revision label: `M_Creature_01.uasset` + `33` →
/// `M_Creature_01#33.uasset`. UE's `-diff` window derives each side's revision
/// caption by splitting the file name at the first `#`
/// (FMergeAsset::GetRevisionInfo); without one, both panels read the misleading
/// "Local Revision". `p4 print -o` writes such paths fine (verified), and the
/// in-place path re-sanitizes for /Temp package names, so the `#` never leaks
/// into a package path.
fn tag_rev(name: &str, label: &str) -> String {
    if label.is_empty() {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) => format!("{}#{}{}", &name[..i], label, &name[i..]),
        None => format!("{name}#{label}"),
    }
}

/// A revision vs its predecessor (history/changelist details). Rev 1 (added)
/// diffs against an empty left side.
#[tauri::command]
pub async fn diff_pair_rev(conn: P4Conn, depot_file: String, rev: i64) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = base_name(&depot_file).to_string();
        let prev = if rev > 1 { format!("{depot_file}#{}", rev - 1) } else { String::new() };
        let left = print_side(&conn, &prev, &sane(&tag_rev(&name, &if rev > 1 { (rev - 1).to_string() } else { String::new() })))?;
        let right = print_side(&conn, &format!("{depot_file}#{rev}"), &sane(&tag_rev(&name, &rev.to_string())))?;
        Ok(DiffPair {
            left,
            right,
            left_label: if rev > 1 { format!("{name}#{}", rev - 1) } else { format!("{name} (added)") },
            right_label: format!("{name}#{rev}"),
            title: format!("{name}  #{}{rev}", if rev > 1 { format!("{} → #", rev - 1) } else { String::new() }),
            right_editable: false,
            unresolved_note: String::new(),
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// A shelved file vs its base revision.
#[tauri::command]
pub async fn diff_pair_shelved(
    conn: P4Conn,
    depot_file: String,
    rev: i64,
    change: String,
) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = base_name(&depot_file).to_string();
        let base = if rev >= 1 { format!("{depot_file}#{rev}") } else { String::new() };
        let left = print_side(&conn, &base, &sane(&tag_rev(&name, &if rev >= 1 { rev.to_string() } else { String::new() })))?;
        let right = print_side(&conn, &format!("{depot_file}@={change}"), &sane(&tag_rev(&name, &format!("shelf-{change}"))))?;
        Ok(DiffPair {
            left,
            right,
            left_label: if rev >= 1 { format!("{name}#{rev}") } else { format!("{name} (added)") },
            right_label: format!("{name} (shelved @{change})"),
            title: format!("{name}  shelf @{change}"),
            right_editable: false,
            unresolved_note: String::new(),
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// The live workspace file vs the revision it is based on.
///
/// Normally that is `#have`. But a file waiting on a `p4 resolve` is based on the
/// resolve's BASE, not on `#have`: the sync already moved `have` to the incoming
/// revision while leaving the workspace file alone. Diffing against `#have` there
/// shows the incoming change INVERTED — the depot's additions appear as your
/// deletions — which reads exactly like someone reverting that change, and
/// "cleaning up" the diff silently drops it. So an unresolved file is compared to
/// its base, and the window says what is missing.
#[tauri::command]
pub async fn diff_pair_local(conn: P4Conn, depot_file: String) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let recs = p4::run(&conn, &["fstat", "-T", "clientFile,haveRev", &depot_file])?;
        let first = recs.first().ok_or("file is not in this workspace")?;
        let local = first
            .get("clientFile")
            .and_then(|v| v.as_str())
            .ok_or("file is not in this workspace")?
            .to_string();
        let have = first.get("haveRev").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name = base_name(&depot_file).to_string();

        // Is a resolve pending? Then the file is based on the resolve base.
        let (mut base_rev, mut note) = (have.clone(), String::new());
        if let Ok(rr) = p4::run(&conn, &["fstat", "-Ru", "-Or", &depot_file]) {
            if let Some(rec) = rr.first() {
                if let Some(sub) = p4::explode_indexed(rec, "resolveBaseFile").first() {
                    let g = |k: &str| {
                        sub.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
                    };
                    let (br, fr) = (g("resolveBaseRev"), g("resolveEndFromRev"));
                    if !br.is_empty() && !fr.is_empty() {
                        base_rev = br.clone();
                        note = format!(
                            "Unresolved merge: depot #{fr} is not in this file yet, so it is compared to \
                             its base #{br} — you are seeing only your own edits. Resolve the file to take \
                             the depot change."
                        );
                    }
                }
            }
        }

        // No revision to compare against (add in progress) → empty left side.
        let spec = if base_rev.is_empty() {
            String::new()
        } else {
            format!("{depot_file}#{base_rev}")
        };
        let left = print_side(&conn, &spec, &sane(&tag_rev(&name, &base_rev)))?;
        Ok(DiffPair {
            left,
            right: local,
            left_label: if base_rev.is_empty() {
                format!("{name} (new)")
            } else if note.is_empty() {
                format!("{name}#{base_rev}")
            } else {
                format!("{name}#{base_rev} (pre-merge base)")
            },
            right_label: format!("{name} (workspace)"),
            title: format!("{name}  local changes"),
            right_editable: true,
            unresolved_note: note,
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// Read a text file for the diff window (lossy UTF-8; size-capped so a huge or
/// binary file can't hang the renderer).
#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        const CAP: u64 = 16 * 1024 * 1024; // 16 MB
        let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if meta.len() > CAP {
            return Err(format!("File too large to diff in-app ({} MB).", meta.len() / (1024 * 1024)));
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
    .await
    .map_err(|e| format!("read task failed: {e}"))?
}

/// `UnrealEditor.exe` above `root` (the engine sits at or above the workspace —
/// e.g. a Games/<Project> client under the repo root).
fn find_unreal_editor(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(root);
    for _ in 0..5 {
        let d = dir?;
        let exe = d.join("Engine").join("Binaries").join("Win64").join("UnrealEditor.exe");
        if exe.is_file() {
            return Some(exe);
        }
        dir = d.parent();
    }
    None
}

/// The first `.uproject` in `root`, its direct children, or `root\Games\*`.
fn find_uproject(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let has_uproject = |d: &std::path::Path| -> Option<std::path::PathBuf> {
        std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).find(|p| {
            p.extension().is_some_and(|x| x.eq_ignore_ascii_case("uproject"))
        })
    };
    if let Some(p) = has_uproject(root) {
        return Some(p);
    }
    for base in [root.to_path_buf(), root.join("Games")] {
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if let Some(u) = has_uproject(&p) {
                        return Some(u);
                    }
                }
            }
        }
    }
    None
}

/// Strip characters that would break out of a Python string literal.
fn py_safe(s: &str) -> String {
    s.replace(['\'', '"', '\\', '\n', '\r'], "_")
}

/// The in-editor diff script: load both packages (LoadPackage accepts raw file
/// paths — temp files get a transient package, the workspace file resolves to
/// its mounted /Game package, i.e. the LIVE asset), find the asset by name (an
/// asset's object name == its file base name), and open the class-specific diff
/// via IAssetTools::DiffAssets (BlueprintCallable → exposed to Python).
/// Open ONE asset in the editor: what an added file (no previous revision) or a
/// deleted one (no current revision) can still usefully show. `unreal` exposes
/// the subsystem only through get_editor_subsystem — constructing
/// AssetEditorSubsystem() yields an uninitialised object whose
/// open_editor_for_assets crashes the editor outright.
fn single_script(spec: &str, name: &str) -> String {
    let s = spec.replace('\'', "_");
    let name = py_safe(name);
    format!(
        r#"import unreal
_p = unreal.load_package(r'{s}')
_a = unreal.find_object(_p, '{name}') if _p else None
if not _a:
    raise RuntimeError('Auger: asset {name} not found in {s}')
unreal.get_editor_subsystem(unreal.AssetEditorSubsystem).open_editor_for_assets([_a])
"#
    )
}

fn diff_script(left: &str, right: &str, name: &str, left_rev: &str, right_rev: &str) -> String {
    // Paths go through raw strings; quotes/newlines are stripped defensively.
    let (l, r) = (left.replace('\'', "_"), right.replace('\'', "_"));
    let (name, lrev, rrev) = (py_safe(name), py_safe(left_rev), py_safe(right_rev));
    format!(
        r#"import unreal
_lp = unreal.load_package(r'{l}')
_rp = unreal.load_package(r'{r}')
_la = unreal.find_object(_lp, '{name}') if _lp else None
_ra = unreal.find_object(_rp, '{name}') if _rp else None
if not _la or not _ra:
    raise RuntimeError('Auger: asset {name} not found (left ok=%s, right ok=%s)' % (_la is not None, _ra is not None))
_li = unreal.RevisionInfo()
_ri = unreal.RevisionInfo()
try:
    _li.revision = '{lrev}'
    _ri.revision = '{rrev}'
except Exception:
    pass
unreal.AssetToolsHelpers.get_asset_tools().diff_assets(_la, _ra, _li, _ri)
"#
    )
}

/// What to pass to `unreal.load_package` for a diff side. A file under the
/// running project's mounted Content loads by its raw path (converts to its
/// /Game package — the LIVE asset). Anything else (our temp files; Python's
/// load_package returns None for unmounted paths) is COPIED into the running
/// editor's `Saved/Diff` under a clean unique name and loaded as
/// `/Temp/Diff/<stem>` — the built-in `/Temp` package root maps to the
/// project's Saved dir (verified live).
fn loadable_spec(editor_project_dir: &std::path::Path, src: &str) -> Result<String, String> {
    let content = editor_project_dir.join("Content");
    if std::path::Path::new(src).starts_with(&content) {
        return Ok(src.to_string());
    }
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // The package-name stem must avoid '.'; keep the file extension on disk.
    let fname = src.rsplit(['\\', '/']).next().unwrap_or("file");
    let (stem, ext) = match fname.rfind('.') {
        Some(i) => (&fname[..i], &fname[i..]),
        None => (fname, ""),
    };
    let stem: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let dir = editor_project_dir.join("Saved").join("Diff");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Best effort: prune day-old copies from previous diffs.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
        for e in rd.flatten() {
            let name = e.file_name();
            let is_ours = name.to_str().is_some_and(|n| n.starts_with("auger_"));
            let old = e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
            if is_ours && old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let base = format!("auger_{uniq}_{stem}");
    let dst = dir.join(format!("{base}{ext}"));
    std::fs::copy(src, &dst).map_err(|e| format!("copy for in-place diff failed: {e}"))?;
    // Freshly copied — make sure it isn't read-only (source may be a p4 print).
    if let Ok(md) = std::fs::metadata(&dst) {
        let mut p = md.permissions();
        if p.readonly() {
            p.set_readonly(false);
            let _ = std::fs::set_permissions(&dst, p);
        }
    }
    Ok(format!("/Temp/Diff/{base}"))
}

/// Per-process config overrides for an editor we launch ourselves for a diff.
/// Nothing is written to the user's own ini files — these are command-line
/// overrides, and a throwaway diff instance doesn't need what they disable.
/// Measured on a real launch (editor log timestamps):
/// - the startup map is by far the slowest part of a boot the diff never shows;
/// - Python developer mode regenerated the unreal.py stub for ~30s on frame 1;
/// - Wwise kept trying to reach a WAAPI authoring app that isn't there.
/// Remote execution stays ON on purpose: the diff editor answers the next
/// asset diff's ping, so follow-up diffs open in it instantly.
const DIFF_LAUNCH_OVERRIDES: [&str; 4] = [
    "-ini:EditorPerProjectUserSettings:[/Script/UnrealEd.EditorLoadingSavingSettings]:LoadLevelAtStartup=None",
    "-ini:Engine:[/Script/PythonScriptPlugin.PythonScriptPluginSettings]:bDeveloperMode=False",
    "-ini:EditorPerProjectUserSettings:[/Script/PythonScriptPlugin.PythonScriptPluginUserSettings]:bDeveloperMode=False",
    "-ini:EditorPerProjectUserSettings:[/Script/AkAudio.AkSettingsPerUser]:bAutoConnectToWAAPI=False",
];

/// Diff two UE asset files in Unreal's asset-diff tool. Prefers an ALREADY
/// RUNNING editor (Python remote execution → in-place DiffAssets); falls back
/// to launching `UnrealEditor.exe <project> -diff <left> <right>`. Returns
/// "remote" or "launched" for the caller's notice.
#[tauri::command]
pub async fn open_unreal_diff(
    conn: P4Conn,
    left: String,
    right: String,
    name: String,
    left_rev: String,
    right_rev: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let t0 = std::time::Instant::now();
        // An added asset has no previous revision, a deleted one has no current
        // revision: that side was printed as an EMPTY temp file, which Unreal
        // cannot load as a package. There is nothing to COMPARE, but the asset
        // itself is still worth looking at — a text add opens in the diff window
        // as a single pane, so an asset add should not just refuse either. The
        // solo side is opened in the asset editor instead.
        let empty = |p: &str| std::fs::metadata(p).map(|m| m.len() == 0).unwrap_or(false);
        let (l_empty, r_empty) = (empty(&left), empty(&right));
        if l_empty && r_empty {
            return Ok("nocompare".to_string());
        }
        let solo: Option<String> = if l_empty {
            Some(right.clone())
        } else if r_empty {
            Some(left.clone())
        } else {
            None
        };
        // In-place first: a running editor with remote execution enabled. Its
        // pong tells us WHICH project it runs (its Saved dir backs the /Temp
        // package root the copies load through) — no p4 round trip needed; the
        // workspace lookup only happens on the fallback-launch path.
        let in_place = || -> Result<Option<()>, String> {
            let (left, right) = (left.clone(), right.clone());
            let (name, left_rev, right_rev) = (name.clone(), left_rev.clone(), right_rev.clone());
            let solo_in = solo.clone();
            super::unreal_remote::run_python_in_editor_with(move |node| {
                let editor_dir = node
                    .get("project_root")
                    .and_then(|v| v.as_str())
                    .map(|p| {
                        let pb = std::path::PathBuf::from(p);
                        if pb.extension().is_some_and(|x| x.eq_ignore_ascii_case("uproject")) {
                            pb.parent().map(|d| d.to_path_buf()).unwrap_or(pb)
                        } else {
                            pb
                        }
                    })
                    .ok_or("editor pong carried no project_root")?;
                if let Some(one) = &solo_in {
                    let s = loadable_spec(&editor_dir, one)?;
                    return Ok(single_script(&s, &name));
                }
                let l = loadable_spec(&editor_dir, &left)?;
                let r = loadable_spec(&editor_dir, &right)?;
                Ok(diff_script(&l, &r, &name, &left_rev, &right_rev))
            })
        };
        match in_place() {
            Ok(Some(())) => {
                eprintln!("[auger-timing] in-place unreal diff: {:?}", t0.elapsed());
                return Ok(if solo.is_some() { "single-remote".to_string() } else { "remote".to_string() });
            }
            Ok(None) => {} // no running editor — launch one
            Err(e) => {
                // An editor answered but the in-place diff failed — fall back to
                // a fresh instance so the user still gets a diff.
                eprintln!("in-place Unreal diff failed, launching an instance: {e}");
            }
        }
        eprintln!("[auger-timing] in-place attempt (pre-fallback): {:?}", t0.elapsed());
        let recs = p4::run(&conn, &["info"])?;
        let root = recs
            .first()
            .and_then(|r| r.get("clientRoot"))
            .and_then(|v| v.as_str())
            .ok_or("no workspace root (p4 info clientRoot)")?
            .to_string();
        let root = std::path::PathBuf::from(root);
        let project = find_uproject(&root)
            .ok_or("No .uproject found under the workspace root")?;
        let editor = find_unreal_editor(&root)
            .ok_or("UnrealEditor.exe not found at or above the workspace root")?;
        let mut c = std::process::Command::new(editor);
        // The overrides go BEFORE -diff on purpose: the engine splits the command
        // line at `-diff` and treats everything after it as the diff's own
        // arguments. The -ini: mechanism is compiled into every non-shipping
        // build (ALLOW_INI_OVERRIDE_FROM_COMMANDLINE), and skipping the startup
        // map works because UnrealEdMisc::OnInit only loads one when the setting
        // is not ELoadLevelAtStartup::None.
        c.arg(&project);
        for flag in DIFF_LAUNCH_OVERRIDES {
            c.arg(flag);
        }
        if let Some(one) = &solo {
            // No counterpart to diff against, so ask the fresh editor to open the
            // asset: `py "<file>"` is a real console command and -ExecCmds runs
            // deferred commands once the engine is up. The script lives beside
            // the copied package under the project's Saved dir (a UE project path
            // has no spaces, which -ExecCmds could not carry).
            let proj_dir = project.parent().map(|d| d.to_path_buf()).unwrap_or(root.clone());
            let spec = loadable_spec(&proj_dir, one)?;
            let dir = proj_dir.join("Saved").join("Diff");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let py = dir.join("auger_open_asset.py");
            std::fs::write(&py, single_script(&spec, &name))
                .map_err(|e| format!("cannot write the open-asset script: {e}"))?;
            c.arg(format!("-ExecCmds=py {}", py.display()));
            c.spawn().map_err(|e| format!("failed to launch Unreal: {e}"))?;
            return Ok("single-launched".to_string());
        }
        c.arg("-diff").arg(&left).arg(&right);
        c.spawn().map_err(|e| format!("failed to launch Unreal diff: {e}"))?;
        Ok("launched".to_string())
    })
    .await
    .map_err(|e| format!("unreal diff task failed: {e}"))?
}

/// Minimal percent-encoding for query-string values (RFC 3986 unreserved kept).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Write `text` to a workspace file, keeping its BOM, line endings and trailing
/// newline, and clearing the read-only flag p4 leaves on unopened files.
#[tauri::command]
pub async fn write_local_file(path: String, text: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let raw = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let (bom, body) = super::patch::split_bom(&raw);
        let existing = String::from_utf8(body.to_vec()).map_err(|_| "not a UTF-8 text file")?;
        let eol = if existing.contains("\r\n") { "\r\n" } else { "\n" };
        let ends = existing.ends_with('\n');
        let mut out = bom.to_vec();
        let mut joined = super::patch::split_lines(&text).join(eol);
        if ends {
            joined.push_str(eol);
        }
        out.extend_from_slice(joined.as_bytes());
        super::patch::make_writable(&path).map_err(|e| format!("cannot make {path} writable: {e}"))?;
        std::fs::write(&path, &out).map_err(|e| format!("cannot write {path}: {e}"))
    })
    .await
    .map_err(|e| format!("write task failed: {e}"))?
}

/// Open the in-app diff window on the `/diff` route. Each call gets its own
/// window (unique label), like an external diff tool would.
#[tauri::command]
pub async fn open_diff_window(app: AppHandle, pair: DiffPair) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("diff-{n}");
    let url = format!(
        "diff?left={}&right={}&ll={}&rl={}&title={}&edit={}&note={}",
        enc(&pair.left),
        enc(&pair.right),
        enc(&pair.left_label),
        enc(&pair.right_label),
        enc(&pair.title),
        if pair.right_editable { "1" } else { "0" },
        enc(&pair.unresolved_note),
    );
    let title = format!("Diff — {}", pair.title);
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(&title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(700.0, 400.0)
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open diff window: {e}"))?;
    // One geometry shared by every diff window: their labels are unique, so
    // per-window state could never be restored.
    crate::wingeom::apply(&win, "diff");
    Ok(())
}
