//! Applying a unified-diff `.patch` onto the workspace — the inverse of
//! `export_patch`. Hunks are located by context search (so a patch still lands
//! on a file that has drifted), and whatever cannot be placed is reported as a
//! conflict and written to a `.rej` file instead of being guessed at.

use crate::p4::{self, P4Conn};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// How far from its recorded position a hunk may be found before giving up.
const SEARCH_WINDOW: usize = 2000;

#[derive(Clone)]
pub(crate) struct Hunk {
    /// 1-based line in the original file.
    pub(crate) old_start: usize,
    /// Context + removed lines: what the hunk expects to find.
    pub(crate) old: Vec<String>,
    /// Context + added lines: what it leaves behind.
    pub(crate) new: Vec<String>,
    /// Raw text, replayed verbatim into a `.rej` when the hunk is rejected.
    pub(crate) raw: String,
}

pub(crate) struct PatchFile {
    pub(crate) depot: String,
    /// The `+++` path, i.e. wherever the patch was generated. Only a fallback:
    /// another machine's workspace maps the same depot path elsewhere.
    pub(crate) local_hint: String,
    pub(crate) hunks: Vec<Hunk>,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HunkReport {
    /// 1-based hunk number within its file, as the user sees it in the patch.
    pub index: usize,
    /// "clean" | "fuzz" | "already" | "conflict"
    pub status: String,
    /// Where it landed (1-based), 0 when it did not.
    pub line: usize,
    /// Lines away from its recorded position.
    pub offset: i64,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileReport {
    pub depot: String,
    pub local: String,
    /// "clean" | "fuzz" | "already" | "partial" | "conflict" | "missing" | "notext"
    pub status: String,
    pub hunks: Vec<HunkReport>,
    pub applied: usize,
    pub conflicts: usize,
    /// Empty unless something needs saying (unresolvable path, .rej written…).
    pub message: String,
    /// Set on apply when rejected hunks were saved beside the file.
    pub rej_path: String,
}

/// Prompt for a `.patch` to open. None if the user cancelled.
#[tauri::command]
pub async fn pick_patch_file(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let picked = app
            .dialog()
            .file()
            .add_filter("Patch", &["patch", "diff"])
            .blocking_pick_file();
        match picked {
            Some(fp) => Ok(Some(fp.into_path().map_err(|e| e.to_string())?.display().to_string())),
            None => Ok(None),
        }
    })
    .await
    .map_err(|e| format!("pick-patch task failed: {e}"))?
}

/// Dry-run: report what applying `patch_path` would do, touching nothing.
#[tauri::command]
pub async fn preview_patch(conn: P4Conn, patch_path: String) -> Result<Vec<FileReport>, String> {
    tauri::async_runtime::spawn_blocking(move || run_patch(&conn, &patch_path, None))
        .await
        .map_err(|e| format!("preview-patch task failed: {e}"))?
}

/// Apply `patch_path`. `mode` is "edit" (open each target in `change` first, so
/// the result lands in a pending changelist) or "offline" (write to disk only,
/// leaving the files for the Offline section to pick up). `partial` allows a
/// file to take the hunks that fit; without it a file with any conflict is left
/// untouched.
#[tauri::command]
pub async fn apply_patch(
    conn: P4Conn,
    patch_path: String,
    mode: String,
    change: String,
    partial: bool,
) -> Result<Vec<FileReport>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_patch(&conn, &patch_path, Some(ApplyOpts { mode, change, partial }))
    })
    .await
    .map_err(|e| format!("apply-patch task failed: {e}"))?
}

struct ApplyOpts {
    mode: String,
    change: String,
    partial: bool,
}

/// Open a rejected hunk as a three-way merge: the patch's expected text is the
/// base, what it wants is "theirs", and the file's closest-matching region is
/// "yours". Returns the merge id for the resolve window.
#[tauri::command]
pub async fn merge_start_patch(
    conn: P4Conn,
    patch_path: String,
    depot_file: String,
    hunk_index: usize,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_patch_merge(&conn, &patch_path, &depot_file, hunk_index)
    })
    .await
    .map_err(|e| format!("merge-start-patch task failed: {e}"))?
}

pub(crate) fn prepare_patch_merge(
    conn: &P4Conn,
    patch_path: &str,
    depot_file: &str,
    hunk_index: usize,
) -> Result<String, String> {
    let text = std::fs::read_to_string(patch_path)
        .map_err(|e| format!("cannot read the patch: {e}"))?;
    let files = parse_patch(&text);
    let pf = files
        .iter()
        .find(|f| f.depot == *depot_file)
        .ok_or("this patch no longer contains that file")?;
    let h = pf
        .hunks
        .get(hunk_index.saturating_sub(1))
        .ok_or("this patch no longer contains that hunk")?;
    let local = resolve_target(conn, &pf.depot, &pf.local_hint)
        .ok_or("the target file is not in this workspace")?;
    let raw = std::fs::read(&local).map_err(|e| format!("cannot read the target: {e}"))?;
    let (_, body) = split_bom(&raw);
    let body = String::from_utf8(body.to_vec())
        .map_err(|_| "the target is not a UTF-8 text file".to_string())?;
    let lines = split_lines(&body);

    // The region the hunk was meant for: whichever window of the file looks
    // most like its expected text.
    let (from, to) = closest_region(&lines, &h.old, h.old_start.saturating_sub(1));
    let name = pf.depot.rsplit('/').next().unwrap_or("file").to_string();
    let rej = rej_for(&local, &h.raw);
    Ok(super::merge::register(super::merge::MergeJob {
        kind: "patch".into(),
        conn: conn.clone(),
        depot: pf.depot.clone(),
        target: local,
        name,
        base_label: format!("patch expects (hunk #{hunk_index})"),
        theirs_label: "patch".into(),
        yours_label: format!("workspace (lines {}–{})", from + 1, to.max(from + 1)),
        base: h.old.clone(),
        ours: lines[from..to].to_vec(),
        theirs: h.new.clone(),
        splice: Some((from, to)),
        rej,
    }))
}

/// The `.rej` beside `local` and this hunk's text, so resolving it can prune
/// the entry. None when no `.rej` mentions the hunk.
fn rej_for(local: &str, raw: &str) -> Option<(String, String)> {
    let path = format!("{local}.rej");
    let body = std::fs::read_to_string(&path).ok()?;
    body.contains(raw).then_some((path, raw.to_string()))
}

/// The window of `lines` that best matches `want`, searching outward from
/// `expected`. Ties go to the position nearest the expected one.
fn closest_region(lines: &[String], want: &[String], expected: usize) -> (usize, usize) {
    let len = want.len().min(lines.len());
    if len == 0 || lines.is_empty() {
        let at = expected.min(lines.len());
        return (at, at);
    }
    let mut best = (expected.min(lines.len() - len), 0usize);
    for at in candidates(lines.len(), len, expected) {
        let score = want
            .iter()
            .zip(&lines[at..at + len])
            .filter(|(a, b)| a.trim() == b.trim())
            .count();
        if score > best.1 {
            best = (at, score);
        }
    }
    (best.0, best.0 + len)
}

/// The shared pipeline: parse, resolve each target, place every hunk, and —
/// when `opts` is set — write the results. Preview and apply run the same code
/// so the dialog cannot promise something the apply then does differently.
fn run_patch(
    conn: &P4Conn,
    patch_path: &str,
    opts: Option<ApplyOpts>,
) -> Result<Vec<FileReport>, String> {
    let bytes = std::fs::read(patch_path).map_err(|e| format!("cannot read the patch: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "the patch is not valid UTF-8".to_string())?;
    let files = parse_patch(&text);
    if files.is_empty() {
        return Err("No unified-diff hunks found in this file.".into());
    }

    let mut out: Vec<FileReport> = Vec::new();
    for pf in &files {
        out.push(apply_one(conn, pf, opts.as_ref()));
    }
    Ok(out)
}

fn apply_one(conn: &P4Conn, pf: &PatchFile, opts: Option<&ApplyOpts>) -> FileReport {
    let mut rep = FileReport {
        depot: pf.depot.clone(),
        local: String::new(),
        status: "conflict".into(),
        hunks: Vec::new(),
        applied: 0,
        conflicts: 0,
        message: String::new(),
        rej_path: String::new(),
    };

    // Map the depot path through THIS workspace; the patch's own local path is
    // only a fallback for a patch made outside any client view.
    let local = resolve_target(conn, &pf.depot, &pf.local_hint);
    let Some(local) = local else {
        rep.status = "missing".into();
        rep.message = "not mapped in this workspace and no local file to patch".into();
        return rep;
    };
    rep.local = local.clone();

    let raw = match std::fs::read(&local) {
        Ok(b) => b,
        Err(e) => {
            rep.status = "missing".into();
            rep.message = format!("cannot read the target: {e}");
            return rep;
        }
    };
    let (bom, body) = split_bom(&raw);
    let Ok(body) = String::from_utf8(body.to_vec()) else {
        rep.status = "notext".into();
        rep.message = "not a UTF-8 text file".into();
        return rep;
    };
    let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_with_eol = body.ends_with('\n');
    let mut lines: Vec<String> = split_lines(&body);

    // Place each hunk against the file as it stands after the earlier ones.
    let mut delta: i64 = 0;
    let mut rejected: Vec<&Hunk> = Vec::new();
    for (i, h) in pf.hunks.iter().enumerate() {
        let expected = (h.old_start as i64 - 1 + delta).max(0) as usize;
        match place(&lines, h, expected) {
            Placed::At { at, loose } => {
                let offset = at as i64 - expected as i64;
                rep.hunks.push(HunkReport {
                    index: i + 1,
                    status: if loose || offset != 0 { "fuzz".into() } else { "clean".into() },
                    line: at + 1,
                    offset,
                });
                lines.splice(at..at + h.old.len(), h.new.iter().cloned());
                delta += h.new.len() as i64 - h.old.len() as i64;
                rep.applied += 1;
            }
            Placed::Already { at } => {
                rep.hunks.push(HunkReport {
                    index: i + 1,
                    status: "already".into(),
                    line: at + 1,
                    offset: 0,
                });
            }
            Placed::No => {
                rep.hunks.push(HunkReport {
                    index: i + 1,
                    status: "conflict".into(),
                    line: 0,
                    offset: 0,
                });
                rejected.push(h);
                rep.conflicts += 1;
            }
        }
    }

    let already = rep.hunks.iter().filter(|h| h.status == "already").count();
    let fuzzed = rep.hunks.iter().any(|h| h.status == "fuzz");
    rep.status = if rep.conflicts == pf.hunks.len() {
        "conflict".into()
    } else if rep.conflicts > 0 {
        "partial".into()
    } else if already == pf.hunks.len() {
        "already".into()
    } else if fuzzed {
        "fuzz".into()
    } else {
        "clean".into()
    };

    let Some(opts) = opts else { return rep }; // preview stops here

    // Nothing to write: every hunk was already in the file, or the whole file
    // conflicts, or it partly conflicts and the user did not allow partials.
    if rep.applied == 0 || (rep.conflicts > 0 && !opts.partial) {
        if rep.conflicts > 0 {
            rep.message = "left untouched".into();
        }
        return rep;
    }

    if opts.mode == "edit" {
        let mut args: Vec<&str> = vec!["edit"];
        if !opts.change.is_empty() {
            args.push("-c");
            args.push(&opts.change);
        }
        args.push(&pf.depot);
        if let Err(e) = p4::run(conn, &args) {
            rep.status = "conflict".into();
            rep.message = format!("p4 edit failed: {e}");
            return rep;
        }
    } else if let Err(e) = make_writable(&local) {
        rep.status = "conflict".into();
        rep.message = format!("cannot make the file writable: {e}");
        return rep;
    }

    let mut text = lines.join(eol);
    if ends_with_eol {
        text.push_str(eol);
    }
    let mut bytes = bom.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    if let Err(e) = std::fs::write(&local, &bytes) {
        rep.status = "conflict".into();
        rep.message = format!("cannot write the file: {e}");
        return rep;
    }

    if !rejected.is_empty() {
        let rej = format!("{local}.rej");
        let mut body = format!("--- {}\n+++ {}\n", pf.depot, local);
        for h in &rejected {
            body.push_str(&h.raw);
        }
        match std::fs::write(&rej, body) {
            Ok(()) => {
                rep.rej_path = rej.clone();
                rep.message = format!("{} hunk(s) rejected — saved to {rej}", rep.conflicts);
            }
            Err(e) => rep.message = format!("{} hunk(s) rejected; .rej not written: {e}", rep.conflicts),
        }
    }
    rep
}

enum Placed {
    At { at: usize, loose: bool },
    Already { at: usize },
    No,
}

/// Find where `h` fits in `lines`, preferring `expected`. Searches outward for
/// an exact context match first, then retries ignoring trailing whitespace.
fn place(lines: &[String], h: &Hunk, expected: usize) -> Placed {
    if h.old.is_empty() {
        // Pure insertion: nothing to match against, take the recorded spot.
        return Placed::At { at: expected.min(lines.len()), loose: false };
    }
    for loose in [false, true] {
        for at in candidates(lines.len(), h.old.len(), expected) {
            if matches_at(lines, &h.old, at, loose) {
                return Placed::At { at, loose };
            }
        }
    }
    // Not found — but the hunk's result may already be in the file (patch
    // applied twice), which is a no-op rather than a conflict.
    for at in candidates(lines.len(), h.new.len(), expected) {
        if matches_at(lines, &h.new, at, false) {
            return Placed::Already { at };
        }
    }
    Placed::No
}

/// Positions to try, nearest to `expected` first.
fn candidates(len: usize, need: usize, expected: usize) -> Vec<usize> {
    if need > len {
        return Vec::new();
    }
    let last = len - need;
    let mut v: Vec<usize> = Vec::new();
    if expected <= last {
        v.push(expected);
    }
    for d in 1..=SEARCH_WINDOW {
        let mut any = false;
        if let Some(lo) = expected.checked_sub(d) {
            if lo <= last {
                v.push(lo);
                any = true;
            }
        }
        let hi = expected + d;
        if hi <= last {
            v.push(hi);
            any = true;
        }
        if !any && expected.checked_sub(d).is_none() && expected + d > last {
            break;
        }
    }
    v
}

fn matches_at(lines: &[String], want: &[String], at: usize, loose: bool) -> bool {
    if at + want.len() > lines.len() {
        return false;
    }
    want.iter().zip(&lines[at..at + want.len()]).all(|(a, b)| {
        if loose {
            a.trim_end() == b.trim_end()
        } else {
            a == b
        }
    })
}

/// The local path to patch: `p4 where` for this client, else the patch's own
/// `+++` path when it happens to exist on this machine.
pub(crate) fn resolve_target(conn: &P4Conn, depot: &str, hint: &str) -> Option<String> {
    if let Ok(recs) = p4::run(conn, &["where", depot]) {
        let mapped = recs.first().and_then(|r| {
            r.get("path")
                .or_else(|| r.get("clientFile"))
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        if let Some(p) = mapped {
            if std::path::Path::new(&p).is_file() {
                return Some(p);
            }
        }
    }
    if !hint.is_empty() && std::path::Path::new(hint).is_file() {
        return Some(hint.to_string());
    }
    None
}

pub(crate) fn make_writable(path: &str) -> std::io::Result<()> {
    let md = std::fs::metadata(path)?;
    let mut perms = md.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

pub(crate) fn split_bom(raw: &[u8]) -> (&[u8], &[u8]) {
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        raw.split_at(3)
    } else {
        (&[], raw)
    }
}

/// Split into lines without their terminator, EOL style irrelevant. A trailing
/// newline does not produce a final empty line (that is tracked separately).
pub(crate) fn split_lines(body: &str) -> Vec<String> {
    let mut v: Vec<String> = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
    if body.ends_with('\n') {
        v.pop();
    }
    v
}

/// Parse a unified diff. Hunk bodies are consumed by their `@@` line counts, so
/// content that itself looks like a header cannot derail the walk.
pub(crate) fn parse_patch(text: &str) -> Vec<PatchFile> {
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut files: Vec<PatchFile> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(rest) = l.strip_prefix("--- ") {
            let next = lines.get(i + 1).copied().unwrap_or("");
            if let Some(plus) = next.strip_prefix("+++ ") {
                files.push(PatchFile {
                    depot: strip_tab(rest).to_string(),
                    local_hint: strip_tab(plus).to_string(),
                    hunks: Vec::new(),
                });
                i += 2;
                continue;
            }
        }
        if l.starts_with("@@") {
            if let Some((old_start, old_count, new_count)) = parse_range(l) {
                let mut h = Hunk {
                    old_start,
                    old: Vec::new(),
                    new: Vec::new(),
                    raw: format!("{l}\n"),
                };
                let (mut got_old, mut got_new) = (0usize, 0usize);
                i += 1;
                while i < lines.len() && (got_old < old_count || got_new < new_count) {
                    let c = lines[i];
                    let (tag, content) = match c.chars().next() {
                        Some(t @ (' ' | '-' | '+')) => (t, &c[1..]),
                        // "\ No newline at end of file" — a note, not content.
                        Some('\\') => {
                            h.raw.push_str(c);
                            h.raw.push('\n');
                            i += 1;
                            continue;
                        }
                        // An empty line in the body is a context line whose
                        // single leading space some tools strip.
                        None => (' ', ""),
                        _ => break,
                    };
                    match tag {
                        ' ' => {
                            h.old.push(content.to_string());
                            h.new.push(content.to_string());
                            got_old += 1;
                            got_new += 1;
                        }
                        '-' => {
                            h.old.push(content.to_string());
                            got_old += 1;
                        }
                        _ => {
                            h.new.push(content.to_string());
                            got_new += 1;
                        }
                    }
                    h.raw.push_str(c);
                    h.raw.push('\n');
                    i += 1;
                }
                if let Some(f) = files.last_mut() {
                    f.hunks.push(h);
                }
                continue;
            }
        }
        i += 1;
    }
    files.retain(|f| !f.hunks.is_empty());
    files
}

fn strip_tab(s: &str) -> &str {
    match s.find('\t') {
        Some(k) => &s[..k],
        None => s.trim_end(),
    }
}

/// `@@ -12,7 +12,9 @@` → (12, 7, 9). A missing count means 1.
fn parse_range(l: &str) -> Option<(usize, usize, usize)> {
    let body = l.strip_prefix("@@")?;
    let end = body.find("@@")?;
    let mut parts = body[..end].split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let split = |s: &str| -> Option<(usize, usize)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = split(old)?;
    let (_, new_count) = split(new)?;
    Some((old_start.max(1), old_count, new_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "--- //depot/a.txt\t2026-01-01\r\n+++ C:\\ws\\a.txt\t2026-01-01\r\n@@ -2,3 +2,4 @@\r\n b\r\n-c\r\n+C\r\n+extra\r\n d\r\n";

    #[test]
    fn parses_headers_and_counts() {
        let f = parse_patch(P);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].depot, "//depot/a.txt");
        assert_eq!(f[0].local_hint, "C:\\ws\\a.txt");
        assert_eq!(f[0].hunks.len(), 1);
        assert_eq!(f[0].hunks[0].old, vec!["b", "c", "d"]);
        assert_eq!(f[0].hunks[0].new, vec!["b", "C", "extra", "d"]);
    }

    #[test]
    fn places_clean_and_shifted_and_conflicting() {
        let h = &parse_patch(P)[0].hunks[0];
        let clean: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert!(matches!(place(&clean, h, 1), Placed::At { at: 1, loose: false }));

        // Same content pushed down by 2 lines: found by the context search.
        let shifted: Vec<String> =
            ["x", "y", "a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert!(matches!(place(&shifted, h, 1), Placed::At { at: 3, loose: false }));

        // The context is gone: a conflict, not a wrong-place apply.
        let drifted: Vec<String> = ["a", "zzz", "qqq", "d"].iter().map(|s| s.to_string()).collect();
        assert!(matches!(place(&drifted, h, 1), Placed::No));
    }

    #[test]
    fn detects_already_applied() {
        let h = &parse_patch(P)[0].hunks[0];
        let done: Vec<String> =
            ["a", "b", "C", "extra", "d"].iter().map(|s| s.to_string()).collect();
        assert!(matches!(place(&done, h, 1), Placed::Already { .. }));
    }

    #[test]
    fn loose_match_ignores_trailing_whitespace() {
        let h = &parse_patch(P)[0].hunks[0];
        let ws: Vec<String> = ["a", "b  ", "c\t", "d"].iter().map(|s| s.to_string()).collect();
        assert!(matches!(place(&ws, h, 1), Placed::At { loose: true, .. }));
    }

    #[test]
    fn round_trips_crlf_and_no_trailing_newline() {
        assert_eq!(split_lines("a\r\nb\r\n"), vec!["a", "b"]);
        assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
    }

    /// No server: `p4 where` fails and the `+++` path is used, which is what
    /// lets these exercise the real write path end to end.
    fn offline_conn() -> P4Conn {
        P4Conn {
            port: "0.0.0.0:1".into(),
            user: String::new(),
            client: String::new(),
            cwd: String::new(),
            charset: String::new(),
            ticket: String::new(),
        }
    }

    fn scratch(name: &str, body: &[u8]) -> String {
        let p = std::env::temp_dir().join(format!("p4gui_test_{name}"));
        std::fs::write(&p, body).unwrap();
        p.display().to_string()
    }

    #[test]
    fn applies_to_disk_preserving_crlf_and_bom() {
        let target = scratch("crlf.txt", "\u{feff}a\r\nb\r\nc\r\nd\r\n".as_bytes());
        let patch = scratch(
            "crlf.patch",
            format!("--- //depot/a.txt\t0\n+++ {target}\t0\n@@ -2,3 +2,4 @@\n b\n-c\n+C\n+extra\n d\n")
                .as_bytes(),
        );
        let rep = run_patch(
            &offline_conn(),
            &patch,
            Some(ApplyOpts { mode: "offline".into(), change: String::new(), partial: false }),
        )
        .unwrap();
        assert_eq!(rep[0].status, "clean");
        assert_eq!(rep[0].applied, 1);
        let out = std::fs::read(&target).unwrap();
        assert_eq!(out, "\u{feff}a\r\nb\r\nC\r\nextra\r\nd\r\n".as_bytes());
    }

    #[test]
    fn conflicting_file_is_left_untouched_without_partial() {
        let before = "a\nzzz\nqqq\nd\n";
        let target = scratch("conflict.txt", before.as_bytes());
        let patch = scratch(
            "conflict.patch",
            format!("--- //depot/a.txt\t0\n+++ {target}\t0\n@@ -2,3 +2,4 @@\n b\n-c\n+C\n+extra\n d\n")
                .as_bytes(),
        );
        let rep = run_patch(
            &offline_conn(),
            &patch,
            Some(ApplyOpts { mode: "offline".into(), change: String::new(), partial: false }),
        )
        .unwrap();
        assert_eq!(rep[0].status, "conflict");
        assert_eq!(rep[0].applied, 0);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), before, "must not write on conflict");
        assert!(rep[0].rej_path.is_empty(), "nothing applied, so no .rej");
    }

    #[test]
    fn partial_applies_what_fits_and_rejects_the_rest() {
        // Hunk 1 matches; hunk 2's context is gone.
        let target = scratch("partial.txt", "a\nb\nc\nd\nGONE\nyyy\nzzz\n".as_bytes());
        let patch = scratch(
            "partial.patch",
            format!(
                "--- //depot/a.txt\t0\n+++ {target}\t0\n@@ -2,3 +2,4 @@\n b\n-c\n+C\n+extra\n d\n@@ -5,3 +6,3 @@\n xxx\n-yyy\n+YYY\n zzz\n"
            )
            .as_bytes(),
        );
        let rep = run_patch(
            &offline_conn(),
            &patch,
            Some(ApplyOpts { mode: "offline".into(), change: String::new(), partial: true }),
        )
        .unwrap();
        assert_eq!(rep[0].status, "partial");
        assert_eq!((rep[0].applied, rep[0].conflicts), (1, 1));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "a\nb\nC\nextra\nd\nGONE\nyyy\nzzz\n");
        let rej = std::fs::read_to_string(&rep[0].rej_path).unwrap();
        assert!(rej.contains("-yyy"), "the rejected hunk is replayed verbatim: {rej}");
    }

    #[test]
    fn preview_writes_nothing() {
        let before = "a\nb\nc\nd\n";
        let target = scratch("preview.txt", before.as_bytes());
        let patch = scratch(
            "preview.patch",
            format!("--- //depot/a.txt\t0\n+++ {target}\t0\n@@ -2,3 +2,4 @@\n b\n-c\n+C\n+extra\n d\n")
                .as_bytes(),
        );
        let rep = run_patch(&offline_conn(), &patch, None).unwrap();
        assert_eq!(rep[0].status, "clean");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), before);
    }
}
