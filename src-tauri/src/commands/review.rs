//! One review, in depth: who is reviewing it, what versions it has been through,
//! what changed between two of them, and the state changes this user is allowed
//! to make.
//!
//! The Reviews tab lists reviews; this is for reading ONE. It is a window for the
//! same reason the resolve and history windows are: reviewing happens while the
//! rest of the app stays where it was.
//!
//! Everything here is measured against Swarm 2023.1 (`/api/version` reports API
//! versions 9, 10 and 11):
//!
//!   GET  /api/v9/reviews/{id}               state, author, description,
//!                                           participants (with votes), versions
//!   GET  /api/v10/reviews/{id}/transitions  exactly what THIS user may do to it,
//!                                           with Swarm's own labels, plus what is
//!                                           blocked — so the window never offers
//!                                           an action the server will refuse
//!   POST /api/v9/reviews/{id}/state         the state change itself
//!
//! A version's files live in its own `change`: shelved when `pending` is true,
//! submitted otherwise. `archiveChange` is only Swarm's copy of the live shelf, so
//! it is not what a version should be read from.

use super::diffwin::{print_side, DiffPair};
use crate::p4::{self, P4Conn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Which snapshot of a review to read: one of its versions, or the depot the
/// version was written against.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRef {
    /// The changelist that holds this version's files.
    pub change: String,
    /// Shelved (true) or submitted (false) — it decides the revision syntax.
    pub pending: bool,
    /// What to call it in a diff window ("v2", "base of v2").
    #[serde(default)]
    pub label: String,
    /// Read the revisions this version's files were OPENED from, rather than the
    /// version itself: the comparison point that makes "what does v2 change?" a
    /// question with an answer. Per file, not per changelist — a review's files
    /// are each opened at their own revision.
    #[serde(default)]
    pub base: bool,
}

impl VersionRef {
    /// What to call the VERSION: the caller's own name for it ("v2"), or the
    /// changelist when it did not say.
    fn version_name(&self) -> String {
        if self.label.is_empty() {
            format!("@{}", self.change)
        } else {
            self.label.clone()
        }
    }

    /// What to call this SIDE. The "base of" is added here rather than trusted to
    /// the caller's label: a side that forgot it would read as the version
    /// itself, and "v2 vs v2" is exactly the confusion this avoids.
    fn name(&self) -> String {
        if self.base {
            format!("base of {}", self.version_name())
        } else {
            self.version_name()
        }
    }

    /// The short form for a title, where the version is already named by the
    /// other side: "base -> v2" beats "base of v2 -> v2".
    fn short(&self) -> String {
        if self.base {
            "base".to_string()
        } else {
            self.version_name()
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    pub user: String,
    /// 1 = up, -1 = down, 0 = has not voted.
    pub vote: i64,
    /// Which version they voted on — a vote can be older than the review.
    pub voted_version: u64,
    /// Swarm's own flag: their vote predates the current version.
    pub stale: bool,
    pub required: bool,
    pub is_author: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewVersion {
    /// 1-based, as Swarm numbers them.
    pub n: u64,
    pub change: String,
    pub pending: bool,
    pub user: String,
    /// Unix seconds.
    pub time: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDetail {
    pub id: u64,
    pub state: String,
    pub state_label: String,
    pub author: String,
    pub description: String,
    pub updated: u64,
    pub test_status: String,
    pub reviewers: Vec<Reviewer>,
    pub versions: Vec<ReviewVersion>,
    pub changes: Vec<String>,
    pub commits: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transitions {
    pub items: Vec<Transition>,
    /// Swarm's reasons a transition is unavailable (empty when there are none).
    pub blocked: Vec<String>,
}

/// One file in a version-to-version comparison.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionFile {
    pub depot_file: String,
    /// "changed" | "same" | "added" | "removed", between the two versions asked
    /// about — NOT the file's p4 action, which says nothing about that.
    pub status: String,
    pub size_a: u64,
    pub size_b: u64,
}

// --- Swarm plumbing --------------------------------------------------------

pub(crate) struct Swarm {
    pub base: String,
    pub user: String,
    pub ticket: String,
    pub client: reqwest::blocking::Client,
}

/// Base URL, credentials and a client, or the reason there are none. Unlike the
/// review BADGES (which stay quiet when Swarm is unreachable, because a missing
/// badge is not an error), everything here is something the user asked for, so a
/// failure has to say so.
pub(crate) fn swarm_conn(conn: &P4Conn) -> Result<Swarm, String> {
    let base = super::server::swarm_base(conn);
    if base.is_empty() {
        return Err("No Swarm URL configured on this server (P4.Swarm.URL).".into());
    }
    let ticket = p4::ticket(conn).ok_or("Not logged in to Perforce, so Swarm cannot be reached.")?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    Ok(Swarm { base, user: conn.user.clone(), ticket, client })
}

impl Swarm {
    pub fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        self.send(reqwest::Method::GET, path, None)
    }
    /// Create something (a comment, a state change). Swarm takes form encoding,
    /// including its bracketed nested fields (`context[content][]`).
    pub fn post(&self, path: &str, form: &[(String, String)]) -> Result<serde_json::Value, String> {
        self.send(reqwest::Method::POST, path, Some(form))
    }
    /// Change part of something, leaving the rest alone.
    pub fn patch(&self, path: &str, form: &[(String, String)]) -> Result<serde_json::Value, String> {
        self.send(reqwest::Method::PATCH, path, Some(form))
    }

    fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        form: Option<&[(String, String)]>,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}{path}", self.base);
        let mut req = self
            .client
            .request(method, &url)
            .basic_auth(&self.user, Some(&self.ticket));
        if let Some(f) = form {
            req = req.form(f);
        }
        let resp = req.send().map_err(|e| format!("Swarm unreachable: {e}"))?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(swarm_error(&body, status.as_u16()));
        }
        if body.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&body).map_err(|e| format!("Swarm sent something unreadable: {e}"))
    }
}

/// Swarm's own words for a failure, when it left any: it puts them in `error` or
/// in `messages`, and an HTTP code alone tells the user nothing.
pub(crate) fn swarm_error(body: &str, status: u16) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        // A rejected field is reported per field: {"error":"Bad Request",
        // "details":{"body":"Value is required and can't be empty"}} — measured.
        // The details say what is actually wrong; "Bad Request" does not.
        let details: Vec<String> = v
            .get("details")
            .and_then(|d| d.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, val)| match val.as_str() {
                        Some(s) => {
                            if k == "context" {
                                s.to_string()
                            } else {
                                format!("{k}: {s}")
                            }
                        }
                        None => format!("{k}: {val}"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !details.is_empty() {
            return format!("Swarm refused: {}", details.join("; "));
        }
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            return format!("Swarm refused: {e}");
        }
        let msgs: Vec<String> = v
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        m.as_str().map(String::from).or_else(|| {
                            m.get("text").and_then(|t| t.as_str()).map(String::from)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !msgs.is_empty() {
            return format!("Swarm refused: {}", msgs.join("; "));
        }
    }
    let tail: String = body.chars().take(200).collect();
    format!("Swarm returned HTTP {status}{}", if tail.trim().is_empty() { String::new() } else { format!(": {tail}") })
}

fn as_u64(v: Option<&serde_json::Value>) -> u64 {
    v.and_then(|x| x.as_u64())
        .or_else(|| v.and_then(|x| x.as_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
fn as_str(v: Option<&serde_json::Value>) -> String {
    v.and_then(|x| x.as_str()).unwrap_or("").to_string()
}
/// A changelist number from either a number or a string — Swarm is inconsistent.
fn as_change(v: &serde_json::Value) -> Option<String> {
    v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(String::from))
}

/// The participants map turned into a reviewer list.
///
/// Swarm keys it by user, with an empty object for "no vote" and
/// `{vote: {value, version, isStale}}` for a vote. The author is in there too, so
/// they are flagged rather than dropped: "who is on this review" includes them,
/// and the window can show them apart.
fn reviewers_of(review: &serde_json::Value, author: &str) -> Vec<Reviewer> {
    let mut out: Vec<Reviewer> = Vec::new();
    if let Some(map) = review.get("participants").and_then(|p| p.as_object()) {
        for (user, info) in map {
            let vote = info.get("vote");
            out.push(Reviewer {
                user: user.clone(),
                vote: vote.and_then(|v| v.get("value")).and_then(|v| v.as_i64()).unwrap_or(0),
                voted_version: as_u64(vote.and_then(|v| v.get("version"))),
                stale: vote
                    .and_then(|v| v.get("isStale"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                required: info
                    .get("required")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                is_author: user == author,
            });
        }
    }
    // Author last, then required first, then by name: the people whose opinion is
    // being waited on belong at the top.
    out.sort_by(|a, b| {
        a.is_author
            .cmp(&b.is_author)
            .then(b.required.cmp(&a.required))
            .then(a.user.cmp(&b.user))
    });
    out
}

fn versions_of(review: &serde_json::Value) -> Vec<ReviewVersion> {
    review
        .get("versions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, v)| ReviewVersion {
                    n: i as u64 + 1,
                    change: as_u64(v.get("change")).to_string(),
                    pending: v.get("pending").and_then(|p| p.as_bool()).unwrap_or(true),
                    user: as_str(v.get("user")),
                    time: as_u64(v.get("time")),
                })
                .collect()
        })
        .unwrap_or_default()
}

// --- commands --------------------------------------------------------------

/// Everything about one review.
#[tauri::command]
pub async fn swarm_review_detail(conn: P4Conn, id: u64) -> Result<ReviewDetail, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sw = swarm_conn(&conn)?;
        let body = sw.get(&format!("/api/v9/reviews/{id}"))?;
        let r = body.get("review").unwrap_or(&body);
        let author = as_str(r.get("author"));
        let list = |key: &str| -> Vec<String> {
            r.get(key)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(as_change).collect())
                .unwrap_or_default()
        };
        Ok(ReviewDetail {
            id: as_u64(r.get("id")),
            state: as_str(r.get("state")),
            state_label: as_str(r.get("stateLabel")),
            reviewers: reviewers_of(r, &author),
            versions: versions_of(r),
            author,
            description: as_str(r.get("description")),
            updated: as_u64(r.get("updated")),
            test_status: as_str(r.get("testStatus")),
            changes: list("changes"),
            commits: list("commits"),
        })
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// What this user may do to this review, in Swarm's words.
///
/// Asking beats guessing: whether someone may approve depends on the project's
/// rules, on who they are, and on the review's own state. The window offers what
/// comes back here and nothing else.
#[tauri::command]
pub async fn swarm_transitions(conn: P4Conn, id: u64) -> Result<Transitions, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sw = swarm_conn(&conn)?;
        let body = sw.get(&format!("/api/v10/reviews/{id}/transitions"))?;
        let data = body.get("data").unwrap_or(&body);
        let mut items = data
            .get("transitions")
            .and_then(|t| t.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| Transition {
                        key: k.clone(),
                        label: v.as_str().unwrap_or(k).to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        items.sort_by_key(|t| (rank(&t.key), t.key.clone()));
        let blocked = data
            .get("blocked")
            .and_then(|b| b.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        m.as_str().map(String::from).or_else(|| {
                            m.get("text").and_then(|t| t.as_str()).map(String::from)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Transitions { items, blocked })
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// Button order: the thing a reviewer came to do first, the destructive ones
/// last. Swarm hands the transitions back as a JSON object, so without this the
/// buttons would sit in map order and shuffle as the offered set changes from one
/// review to the next.
fn rank(key: &str) -> u8 {
    match key {
        "approved" => 0,
        "approved:commit" => 1,
        "needsRevision" => 2,
        "needsReview" => 3,
        "rejected" => 4,
        "archived" => 5,
        _ => 6,
    }
}

/// Change a review's state — approve it, ask for revision, reject, archive.
///
/// `state` is a key from `swarm_transitions`, so the app never invents one. On
/// refusal Swarm's own text comes back: it knows things the app does not, such as
/// a project requiring more votes than the review has.
#[tauri::command]
pub async fn swarm_set_state(conn: P4Conn, id: u64, state: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sw = swarm_conn(&conn)?;
        let url = format!("{}/api/v9/reviews/{id}/state", sw.base);
        let resp = sw
            .client
            .post(&url)
            .basic_auth(&sw.user, Some(&sw.ticket))
            .form(&[("state", state.as_str())])
            .send()
            .map_err(|e| format!("Swarm unreachable: {e}"))?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(swarm_error(&body, status.as_u16()));
        }
        // The new label, when Swarm bothers to send the review back.
        let label = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                let r = v.get("review").cloned().unwrap_or(v);
                r.get("stateLabel").and_then(|s| s.as_str()).map(String::from)
            })
            .unwrap_or_else(|| state.clone());
        Ok(label)
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// One version's files, with everything `describe` says about each: the revision
/// it was opened at (or landed as), the action, and — usefully — the digest and
/// size of the content in THAT version, so no second call is needed for it.
fn files_of(conn: &P4Conn, v: &VersionRef) -> Result<Vec<VerFile>, String> {
    let args: Vec<&str> = if v.pending {
        vec!["describe", "-S", "-s", &v.change]
    } else {
        vec!["describe", "-s", &v.change]
    };
    let recs = p4::run(conn, &args)?;
    let mut out: Vec<VerFile> = Vec::new();
    for r in &recs {
        // `describe` packs the files into depotFile0, depotFile1, … on one record.
        for row in p4::explode_indexed(r, "depotFile") {
            let depot_file = as_str(row.get("depotFile"));
            if depot_file.is_empty() {
                continue;
            }
            out.push(VerFile {
                depot_file,
                rev: as_u64(row.get("rev")),
                action: as_str(row.get("action")),
                digest: as_str(row.get("digest")),
                size: as_u64(row.get("fileSize")),
            });
        }
    }
    out.sort_by(|a, b| a.depot_file.cmp(&b.depot_file));
    out.dedup_by(|a, b| a.depot_file == b.depot_file);
    Ok(out)
}

/// One file as `describe` reports it in one version.
#[derive(Clone, Debug)]
struct VerFile {
    depot_file: String,
    /// A shelf's `rev` is the revision the file was opened AT; a submitted
    /// change's is the revision it landed AS.
    rev: u64,
    action: String,
    digest: String,
    size: u64,
}

/// Nothing precedes these actions, so a file carrying one has no base revision.
fn is_new_file(action: &str) -> bool {
    matches!(action, "add" | "move/add" | "branch" | "import")
}

/// The revision a file's change was written against, per the rules measured
/// above: a shelf's own `rev`, a submitted change's `rev - 1`, and nothing at all
/// for a file the change created.
fn base_rev(f: &VerFile, pending: bool) -> Option<u64> {
    if is_new_file(&f.action) {
        return None;
    }
    if pending {
        (f.rev > 0).then_some(f.rev)
    } else {
        (f.rev > 1).then(|| f.rev - 1)
    }
}

/// One side of a comparison: which files it has, and the spec + stats of each.
struct Side {
    stats: HashMap<String, FileStat>,
    /// Spec to print for a diff, per file (absent = the file is not on this side).
    specs: HashMap<String, String>,
}

/// Resolve a version reference into files, specs and stats.
///
/// For a version itself this costs ONE `describe`: it already carries the digest
/// and size of the content in that version. A base side needs an `fstat` on top,
/// batched, because `describe` says nothing about `#rev` — so `want_stats` is
/// false when only the specs are wanted (opening ONE file's diff), which on a
/// large review saves an fstat over every file in it.
fn resolve(conn: &P4Conn, v: &VersionRef, want_stats: bool) -> Result<Side, String> {
    let files = files_of(conn, v)?;
    let mut stats: HashMap<String, FileStat> = HashMap::new();
    let mut specs: HashMap<String, String> = HashMap::new();

    if !v.base {
        for f in files {
            specs.insert(
                f.depot_file.clone(),
                if v.pending {
                    format!("{}@={}", f.depot_file, v.change)
                } else {
                    format!("{}@{}", f.depot_file, v.change)
                },
            );
            stats.insert(
                f.depot_file,
                FileStat { digest: f.digest, size: f.size, action: f.action },
            );
        }
        return Ok(Side { stats, specs });
    }

    // The base side: a file the version CREATED has no base, so it is simply
    // absent here — which is what makes it read as "added" against the version.
    for f in &files {
        if let Some(rev) = base_rev(f, v.pending) {
            specs.insert(f.depot_file.clone(), format!("{}#{rev}", f.depot_file));
        }
    }
    if !want_stats {
        return Ok(Side { stats, specs });
    }
    let list: Vec<String> = specs.values().cloned().collect();
    for chunk in list.chunks(50) {
        let mut args: Vec<&str> = vec!["fstat", "-Ol"];
        args.extend(chunk.iter().map(String::as_str));
        // A chunk whose specs are all unreadable comes back as an error (p4 warns
        // per spec, and `run` only fails when nothing else parsed): that is
        // "none of these exist at that revision", which reads as added.
        let Ok(recs) = p4::run(conn, &args) else { continue };
        for r in recs {
            let file = as_str(r.get("depotFile"));
            if file.is_empty() {
                continue;
            }
            stats.insert(
                file,
                FileStat {
                    digest: as_str(r.get("digest")),
                    size: as_u64(r.get("fileSize")),
                    action: as_str(r.get("headAction")),
                },
            );
        }
    }
    // A base revision that exists but could not be read is still a side the file
    // is ON: dropping it would report an edit as an addition.
    for (file, _) in specs.iter() {
        stats.entry(file.clone()).or_default();
    }
    Ok(Side { stats, specs })
}

/// What a file looked like at one version.
#[derive(Clone, Debug, Default, PartialEq)]
struct FileStat {
    /// Empty for a file with no content at that version — a shelved delete has
    /// none, which is why the action below is carried alongside.
    digest: String,
    size: u64,
    action: String,
}

/// How one file compares between two versions.
///
/// Content first, action second: a shelved delete (or any file with no content
/// at that version) has no digest, so two of them would look "changed" forever
/// on a digest test alone. A differing ACTION with identical bytes still counts
/// as changed — the two versions do different things to the depot.
fn compare(a: Option<&FileStat>, b: Option<&FileStat>) -> &'static str {
    match (a, b) {
        (None, Some(_)) => "added",
        (Some(_), None) => "removed",
        (None, None) => "same", // in neither version; nothing to say about it
        (Some(x), Some(y)) => {
            if x.action != y.action {
                "changed"
            } else if x.digest.is_empty() && y.digest.is_empty() {
                // Neither has content (a delete on both sides): the action is
                // all there is, and it matched.
                "same"
            } else if x.digest == y.digest {
                "same"
            } else {
                "changed"
            }
        }
    }
}

/// Which files differ between two versions of a review.
///
/// A digest comparison, so "changed" means the bytes changed — not merely that
/// the file is in both shelves, which is true of every file in every version and
/// is what makes a bare file list useless for this question.
#[tauri::command]
pub async fn review_version_files(
    conn: P4Conn,
    a: VersionRef,
    b: VersionRef,
) -> Result<Vec<VersionFile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sa = resolve(&conn, &a, true)?;
        let sb = resolve(&conn, &b, true)?;
        let mut all: Vec<String> = sa.stats.keys().chain(sb.stats.keys()).cloned().collect();
        all.sort();
        all.dedup();
        Ok(all
            .into_iter()
            .map(|f| {
                let x = sa.stats.get(&f);
                let y = sb.stats.get(&f);
                VersionFile {
                    status: compare(x, y).into(),
                    depot_file: f,
                    size_a: x.map(|s| s.size).unwrap_or(0),
                    size_b: y.map(|s| s.size).unwrap_or(0),
                }
            })
            .collect())
    })
    .await
    .map_err(|e| format!("version diff task failed: {e}"))?
}

/// One file, as it stood at two versions of a review.
#[tauri::command]
pub async fn diff_pair_versions(
    conn: P4Conn,
    depot_file: String,
    a: VersionRef,
    b: VersionRef,
) -> Result<DiffPair, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = depot_file.rsplit('/').next().unwrap_or("file").to_string();
        // An empty spec prints an EMPTY side, which is exactly right for a file
        // that only one of the two sides has.
        let sa = resolve(&conn, &a, false)?;
        let sb = resolve(&conn, &b, false)?;
        let spec_a = sa.specs.get(&depot_file).cloned().unwrap_or_default();
        let spec_b = sb.specs.get(&depot_file).cloned().unwrap_or_default();
        let left = print_side(&conn, &spec_a, &format!("{name}-a{}", a.change))?;
        let right = print_side(&conn, &spec_b, &format!("{name}-b{}", b.change))?;
        Ok(DiffPair {
            left,
            right,
            left_label: format!("{name} ({})", a.name()),
            right_label: format!("{name} ({})", b.name()),
            title: format!("{name}  {} → {}", a.short(), b.short()),
            right_editable: false,
            unresolved_note: String::new(),
        })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

// --- the window ------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewJob {
    pub conn: P4Conn,
    pub id: u64,
}

fn registry() -> &'static Mutex<HashMap<String, ReviewJob>> {
    static R: OnceLock<Mutex<HashMap<String, ReviewJob>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Open (or re-focus) the window for one review. One window per review: asking
/// twice means "show me that again".
#[tauri::command]
pub async fn open_review_window(app: AppHandle, conn: P4Conn, id: u64) -> Result<(), String> {
    let name = format!("review-{id}");
    registry()
        .lock()
        .map_err(|_| "window registry poisoned")?
        .insert(name.clone(), ReviewJob { conn, id });
    if let Some(w) = app.get_webview_window(&name) {
        let _ = w.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(
        &app,
        &name,
        WebviewUrl::App(format!("review?job={name}").into()),
    )
    .title(format!("Review {id}"))
    .inner_size(1150.0, 800.0)
    .min_inner_size(700.0, 400.0)
    .visible(false) // shown by wingeom::apply, already at its remembered spot
    .build()
    .map_err(|e| format!("failed to open the review window: {e}"))?;
    // One geometry for every review window: the labels are per-review, so
    // per-window state could never be restored.
    crate::wingeom::apply(&win, "review");
    Ok(())
}

/// The connection and review id for a window, fetched by the window itself: a
/// `P4Conn` carries a ticket, which has no business in a URL.
#[tauri::command]
pub fn review_job(job: String) -> Result<ReviewJob, String> {
    registry()
        .lock()
        .map_err(|_| "window registry poisoned")?
        .get(&job)
        .cloned()
        .ok_or_else(|| format!("no review job named {job}"))
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use serde_json::json;

    fn vf(rev: u64, action: &str) -> VerFile {
        VerFile {
            depot_file: "//d/f.cpp".into(),
            rev,
            action: action.into(),
            digest: "ABC".into(),
            size: 10,
        }
    }

    #[test]
    fn a_base_side_never_reads_as_the_version_itself() {
        // The exact payload the review window sends for base -> v2.
        let a: VersionRef = serde_json::from_value(json!({
            "change": "202152", "pending": true, "label": "v2", "base": true
        }))
        .unwrap();
        let b: VersionRef = serde_json::from_value(json!({
            "change": "202152", "pending": true, "label": "v2", "base": false
        }))
        .unwrap();
        assert_eq!(a.name(), "base of v2");
        assert_eq!(b.name(), "v2");
        // The title names each side once, and says which way round it is.
        assert_eq!(format!("{} \u{2192} {}", a.short(), b.short()), "base → v2");
        // A caller that sends no label at all still cannot produce "v2 vs v2".
        let bare: VersionRef =
            serde_json::from_value(json!({ "change": "202152", "pending": true, "base": true }))
                .unwrap();
        assert_eq!(bare.name(), "base of @202152");
        assert_ne!(bare.name(), bare.version_name());
    }

    #[test]
    fn a_shelfs_rev_is_its_base_and_a_submitted_changes_is_not() {
        // Measured: shelf 202151 lists its file at rev 2, and //f#2 is the
        // content the shelf was written against. A submitted change lists the
        // revision it LANDED as, so its base is one before.
        assert_eq!(base_rev(&vf(2, "edit"), true), Some(2));
        assert_eq!(base_rev(&vf(2, "edit"), false), Some(1));
        // The first revision of a submitted file has nothing behind it.
        assert_eq!(base_rev(&vf(1, "edit"), false), None);
    }

    #[test]
    fn a_file_the_version_created_has_no_base() {
        // Measured: a shelved `add` sits at rev 1 with nothing behind it.
        assert_eq!(base_rev(&vf(1, "add"), true), None);
        assert_eq!(base_rev(&vf(3, "move/add"), true), None);
        assert_eq!(base_rev(&vf(1, "branch"), false), None);
        // ... which is what makes it read as an addition against the version.
        assert_eq!(compare(None, Some(&stat("ABC", 10, "add"))), "added");
    }

    #[test]
    fn participants_become_reviewers_with_their_votes() {
        // The shape Swarm actually returns (measured on 2023.1).
        let r = json!({
            "author": "gaetan.deturche",
            "participants": {
                "gaetan.deturche": [],
                "jerome.charles": { "vote": { "value": 1, "version": 1, "isStale": false } },
                "simon.odwyer": { "required": true }
            }
        });
        let list = reviewers_of(&r, "gaetan.deturche");
        assert_eq!(list.len(), 3);
        // Required first, author last.
        assert_eq!(list[0].user, "simon.odwyer");
        assert!(list[0].required && list[0].vote == 0);
        assert_eq!(list[1].user, "jerome.charles");
        assert_eq!(list[1].vote, 1);
        assert_eq!(list[1].voted_version, 1);
        assert!(list.last().unwrap().is_author);
    }

    #[test]
    fn a_stale_vote_says_so() {
        let r = json!({
            "author": "a",
            "participants": { "b": { "vote": { "value": -1, "version": 2, "isStale": true } } }
        });
        let list = reviewers_of(&r, "a");
        assert_eq!(list[0].vote, -1);
        assert!(list[0].stale);
    }

    #[test]
    fn versions_are_numbered_and_typed() {
        let r = json!({
            "versions": [
                { "change": 202151, "pending": true, "user": "a", "time": 1 },
                { "change": 202189, "pending": false, "user": "b", "time": 2 }
            ]
        });
        let v = versions_of(&r);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].n, v[0].change.as_str(), v[0].pending), (1, "202151", true));
        assert_eq!((v[1].n, v[1].change.as_str(), v[1].pending), (2, "202189", false));
    }

    fn stat(digest: &str, size: u64, action: &str) -> FileStat {
        FileStat { digest: digest.into(), size, action: action.into() }
    }

    #[test]
    fn digests_decide_what_changed() {
        let a = stat("F98AD26066619A9F99D65AB851CDDFBA", 1849463, "edit");
        let b = stat("44F6674622DDB1F73EE593F5BB6D63E7", 1845155, "edit");
        // The real digests of v1 and v2 of review 202150.
        assert_eq!(compare(Some(&a), Some(&b)), "changed");
        assert_eq!(compare(Some(&a), Some(&a)), "same");
        assert_eq!(compare(None, Some(&b)), "added");
        assert_eq!(compare(Some(&a), None), "removed");
    }

    #[test]
    fn a_shelved_delete_has_no_digest_and_is_not_a_change() {
        // Measured: `fstat -Ol //f@=202116` on a move/delete returns a record
        // with headAction and NO digest and NO fileSize. Two of those are the
        // same file, deleted in both versions.
        let d = stat("", 0, "move/delete");
        assert_eq!(compare(Some(&d), Some(&d)), "same");
        // But deleted in one and edited in the other is a change, even though
        // the digest test could say nothing either way.
        let e = stat("ABC", 10, "edit");
        assert_eq!(compare(Some(&d), Some(&e)), "changed");
    }

    #[test]
    fn identical_bytes_under_a_different_action_still_count() {
        // The author re-opened the file as an add rather than an edit: the same
        // bytes, a different thing done to the depot.
        let edit = stat("ABC", 10, "edit");
        let add = stat("ABC", 10, "add");
        assert_eq!(compare(Some(&edit), Some(&add)), "changed");
    }

    #[test]
    fn the_action_buttons_have_a_deliberate_order() {
        // The set measured on review 202178.
        let mut keys = vec!["archived", "approved:commit", "rejected", "needsRevision", "approved"];
        keys.sort_by_key(|k| (rank(k), k.to_string()));
        assert_eq!(
            keys,
            vec!["approved", "approved:commit", "needsRevision", "rejected", "archived"]
        );
    }

    #[test]
    fn a_rejected_field_reports_the_field() {
        // Measured: POST /api/v9/comments with no body.
        let body = r#"{"error":"Bad Request","isValid":false,"details":{"body":"Value is required and can't be empty"}}"#;
        let msg = swarm_error(body, 400);
        assert!(msg.contains("body: Value is required"), "{msg}");
        // A context failure is already a sentence; the field name adds nothing.
        let body = r#"{"error":"Provided context could not be filtered.","details":{"context":"Command failed: You don't have permission for this operation."}}"#;
        let msg = swarm_error(body, 400);
        assert!(msg.contains("Command failed"), "{msg}");
        assert!(!msg.contains("context:"), "{msg}");
    }

    #[test]
    fn swarms_own_words_survive_a_failure() {
        assert!(swarm_error(r#"{"error":"no such review"}"#, 404).contains("no such review"));
        assert!(swarm_error(r#"{"messages":["not permitted"]}"#, 403).contains("not permitted"));
        // Nothing usable in the body: the code is all there is to report.
        assert!(swarm_error("", 500).contains("500"));
    }
}
