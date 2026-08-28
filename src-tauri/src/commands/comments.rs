//! Swarm comments: reading a review's discussion, and taking part in it.
//!
//! Measured against Swarm 2023.1 (comments are a v9-only API here — `/api/v10`
//! and `/api/v11` have no comments route):
//!
//!   GET   /api/v9/comments?topic=reviews/{id}   every comment on the review
//!   POST  /api/v9/comments                      a new comment or a reply
//!   PATCH /api/v9/comments/{id}                 edit the body, move the task
//!                                               state, archive the thread
//!
//! `/api/v9/comments/notify` is 404 on this server, so there is no way to batch
//! the mail: every comment notifies immediately. The UI says so rather than
//! letting the user discover it by surprise.
//!
//! A line-anchored comment carries its own context, read back verbatim from a
//! real one (review 202116):
//!
//! ```json
//! {"file":"//CuriosityP4/.../3DFlowBase.cpp","leftLine":null,"rightLine":57,
//!  "content":[" \t\tmanager->UnregisterFlowPiece(this);"," \t}"," }"," ",
//!             "+void A3DFlowBase::OnLoadedActorRemovedFromLevel()"],
//!  "type":"text+C","review":202116,"version":2}
//! ```
//!
//! So the anchor is (file, version, left or right line) plus the five diff lines
//! ending at that line, prefixes included. A REPLY adds `"comment": <parent id>`
//! to the same context. Swarm re-validates the context against p4 when it is
//! posted — a bad path comes back as "Provided context could not be filtered",
//! so a wrong anchor fails loudly rather than landing somewhere odd.

use super::review::swarm_conn;
use crate::p4::P4Conn;
use serde::{Deserialize, Serialize};

/// Where a comment is anchored. Absent for a comment on the review as a whole.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentAnchor {
    pub file: String,
    /// The review version the line numbers belong to.
    pub version: u64,
    /// Exactly one of these: a line on the left (the version's base) or on the
    /// right (the version itself), as Swarm numbers its own diff.
    #[serde(default)]
    pub left_line: u64,
    #[serde(default)]
    pub right_line: u64,
    /// The five diff lines ending at the anchored one, each keeping its `+`, `-`
    /// or space prefix. Swarm stores this so a comment still reads correctly
    /// after the file moves under it.
    #[serde(default)]
    pub content: Vec<String>,
    /// The comment this one replies to (0 = a new thread).
    #[serde(default)]
    pub parent: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    /// The body as the user should read it: the UE plugin's trailing metadata
    /// blocks are taken out (see `split_body`).
    pub body: String,
    pub user: String,
    pub time: u64,
    pub updated: u64,
    pub edited: bool,
    /// "comment" | "open" | "addressed" | "verified".
    pub task_state: String,
    /// Swarm's `closed` flag: the thread is archived.
    pub closed: bool,
    /// Anchor, when the comment has one.
    pub file: String,
    pub version: u64,
    pub left_line: u64,
    pub right_line: u64,
    pub content: Vec<String>,
    pub parent: u64,
    /// A comment that carries nothing but the Unreal plugin's own bookkeeping
    /// (read/liked ids). Not something a person wrote: the UI hides these.
    pub bookkeeping: bool,
    /// The asset the Unreal plugin's comment was made on, and its category —
    /// those comments have no line anchor, only this.
    pub asset_file: String,
    pub asset_category: String,
}

/// Split a comment body into what a person wrote and what the Unreal review
/// plugin appended.
///
/// Measured on this server: the plugin stores its state in the comment body,
/// e.g. `"\n\n[UE_Userdata]\nLikedComments=\nReadComments=14313,14315"` (pure
/// bookkeeping, no prose at all — 4 of review 202012's 11 comments) and
/// `"<prose>\n\n[UE_Metadata]\nFile=//...uasset\nCategory=ActivationBlocked\n"`
/// (a real comment on an asset). Rendering those blocks verbatim would look like
/// the app is broken, and listing the pure-bookkeeping ones would show empty
/// comments from people who never wrote any.
fn split_body(raw: &str) -> (String, String, String, bool) {
    let mut prose = raw;
    let mut file = String::new();
    let mut category = String::new();
    let mut had_block = false;

    for tag in ["[UE_Metadata]", "[UE_Userdata]"] {
        if let Some(at) = prose.find(tag) {
            had_block = true;
            let block = &prose[at + tag.len()..];
            for line in block.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    break; // the next block begins
                }
                if let Some(v) = line.strip_prefix("File=") {
                    file = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Category=") {
                    category = v.trim().to_string();
                }
            }
            prose = &prose[..at];
        }
    }
    let prose = prose.trim().to_string();
    // Nothing but a metadata block: the plugin talking to itself.
    let bookkeeping = had_block && prose.is_empty() && file.is_empty();
    (prose, file, category, bookkeeping)
}

fn as_u64(v: Option<&serde_json::Value>) -> u64 {
    v.and_then(|x| x.as_u64())
        .or_else(|| v.and_then(|x| x.as_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
fn as_str(v: Option<&serde_json::Value>) -> String {
    v.and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn parse_comment(c: &serde_json::Value) -> Comment {
    // `context` is an object on an anchored comment and an empty ARRAY on one
    // without an anchor — measured, and the reason this reads it defensively
    // rather than deserializing into a struct.
    let empty = serde_json::Value::Null;
    let ctx = c.get("context").unwrap_or(&empty);
    let raw = as_str(c.get("body"));
    let (body, asset_file, asset_category, bookkeeping) = split_body(&raw);
    let edited = c.get("edited").map(|e| !e.is_null()).unwrap_or(false);
    Comment {
        id: as_u64(c.get("id")),
        body,
        user: as_str(c.get("user")),
        time: as_u64(c.get("time")),
        updated: as_u64(c.get("updated")),
        edited,
        task_state: as_str(c.get("taskState")),
        closed: c
            .get("flags")
            .and_then(|f| f.as_array())
            .map(|a| a.iter().any(|f| f.as_str() == Some("closed")))
            .unwrap_or(false),
        file: as_str(ctx.get("file")),
        version: as_u64(ctx.get("version")),
        left_line: as_u64(ctx.get("leftLine")),
        right_line: as_u64(ctx.get("rightLine")),
        content: ctx
            .get("content")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        parent: as_u64(ctx.get("comment")),
        bookkeeping,
        asset_file,
        asset_category,
    }
}

/// Every comment on a review, oldest first (which is the order a thread reads in).
#[tauri::command]
pub async fn swarm_comments(conn: P4Conn, review: u64) -> Result<Vec<Comment>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sw = swarm_conn(&conn)?;
        let body = sw.get(&format!("/api/v9/comments?topic=reviews/{review}&max=500"))?;
        let mut out: Vec<Comment> = body
            .get("comments")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().map(parse_comment).collect())
            .unwrap_or_default();
        out.sort_by_key(|c| (c.time, c.id));
        Ok(out)
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// Post a comment: on the review as a whole, on a line of one version's diff, or
/// as a reply in an existing thread.
#[tauri::command]
pub async fn swarm_add_comment(
    conn: P4Conn,
    review: u64,
    body: String,
    anchor: Option<CommentAnchor>,
    task_state: String,
) -> Result<Comment, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if body.trim().is_empty() {
            return Err("A comment needs something in it.".into());
        }
        let sw = swarm_conn(&conn)?;
        let mut form: Vec<(String, String)> = vec![
            ("topic".into(), format!("reviews/{review}")),
            ("body".into(), body),
        ];
        // "comment" is the default; only a real task state is worth sending, and
        // Swarm rejects a transition it does not allow (it names the legal ones).
        if !task_state.is_empty() && task_state != "comment" {
            form.push(("taskState".into(), task_state));
        }
        if let Some(a) = anchor {
            // A reply to an UNANCHORED comment has a parent and nothing else:
            // sending an empty file would fail Swarm's context check, which
            // reads the path from p4.
            if !a.file.is_empty() {
                form.push(("context[file]".into(), a.file));
                form.push(("context[version]".into(), a.version.to_string()));
            }
            if a.right_line > 0 {
                form.push(("context[rightLine]".into(), a.right_line.to_string()));
            }
            if a.left_line > 0 {
                form.push(("context[leftLine]".into(), a.left_line.to_string()));
            }
            for line in a.content {
                form.push(("context[content][]".into(), line));
            }
            if a.parent > 0 {
                form.push(("context[comment]".into(), a.parent.to_string()));
            }
        }
        let v = sw.post("/api/v9/comments", &form)?;
        let c = v.get("comment").cloned().unwrap_or(v);
        Ok(parse_comment(&c))
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// Edit a comment, move its task state, or archive/unarchive the thread.
///
/// Every field is optional: whatever is `None` is left alone. An empty `body`
/// would delete what someone wrote, so it is refused rather than sent.
#[tauri::command]
pub async fn swarm_edit_comment(
    conn: P4Conn,
    id: u64,
    body: Option<String>,
    task_state: Option<String>,
    closed: Option<bool>,
) -> Result<Comment, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sw = swarm_conn(&conn)?;
        let mut form: Vec<(String, String)> = Vec::new();
        if let Some(b) = body {
            if b.trim().is_empty() {
                return Err("A comment needs something in it.".into());
            }
            form.push(("body".into(), b));
        }
        if let Some(s) = task_state {
            form.push(("taskState".into(), s));
        }
        if let Some(c) = closed {
            // Swarm's flags are a list; an empty one clears `closed`, and the
            // empty-array form is how a list is emptied over form encoding.
            if c {
                form.push(("flags[]".into(), "closed".into()));
            } else {
                form.push(("flags".into(), String::new()));
            }
        }
        if form.is_empty() {
            return Err("Nothing to change on this comment.".into());
        }
        let v = sw.patch(&format!("/api/v9/comments/{id}"), &form)?;
        let c = v.get("comment").cloned().unwrap_or(v);
        Ok(parse_comment(&c))
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// The task states this comment may move to, according to Swarm.
///
/// There is no endpoint that lists them, but the PATCH route names them when it
/// refuses one: `taskState=?` on a plain comment answers "Invalid task state
/// transition specified. Valid transitions are: open" (measured on comment
/// 14343). A rejected PATCH changes nothing, so asking costs nothing — and the
/// UI then offers exactly what the server will accept, as it does for the
/// review's own state transitions.
#[tauri::command]
pub async fn swarm_task_transitions(conn: P4Conn, id: u64) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sw = swarm_conn(&conn)?;
        let form = vec![("taskState".to_string(), "?".to_string())];
        match sw.patch(&format!("/api/v9/comments/{id}"), &form) {
            // A server that ACCEPTS "?" is one this trick does not work on; say
            // nothing rather than pretend to know.
            Ok(_) => Ok(Vec::new()),
            Err(e) => Ok(parse_valid_transitions(&e)),
        }
    })
    .await
    .map_err(|e| format!("swarm task failed: {e}"))?
}

/// Pull the state list out of Swarm's refusal.
fn parse_valid_transitions(msg: &str) -> Vec<String> {
    let Some(at) = msg.find("Valid transitions are:") else {
        return Vec::new();
    };
    msg[at + "Valid transitions are:".len()..]
        .split(&[',', ';', '"'][..])
        .map(|s| s.trim().trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic()))
        .collect()
}

/// Swarm mails everyone on the review as soon as a comment is posted: this
/// server has no `/comments/notify` route, so there is nothing to batch with.
/// The UI shows this so the user knows a comment is not a private note.
#[tauri::command]
pub fn comments_notify_immediately() -> bool {
    true
}

#[cfg(test)]
mod comment_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_line_comment_keeps_its_anchor() {
        // The exact shape read back from review 202116.
        let c = parse_comment(&json!({
            "id": 14340,
            "body": "override inutile?",
            "user": "simon.odwyer",
            "time": 1787900000,
            "updated": 1787900000,
            "edited": null,
            "taskState": "comment",
            "flags": [],
            "context": {
                "file": "//d/3DFlowBase.cpp",
                "leftLine": null,
                "rightLine": 57,
                "content": [" \t}", " }", " ", "+void A3DFlowBase::OnLoaded()"],
                "type": "text+C",
                "review": 202116,
                "version": 2
            }
        }));
        assert_eq!((c.right_line, c.left_line, c.version), (57, 0, 2));
        assert_eq!(c.file, "//d/3DFlowBase.cpp");
        assert_eq!(c.content.len(), 4);
        assert!(!c.edited && !c.closed && !c.bookkeeping);
    }

    #[test]
    fn a_reply_carries_its_parent() {
        let c = parse_comment(&json!({
            "id": 14344,
            "body": "j'ai récup ça du proxy",
            "context": { "file": "//d/f.cpp", "rightLine": 36, "version": 2, "comment": 14343 }
        }));
        assert_eq!(c.parent, 14343);
    }

    #[test]
    fn an_unanchored_comment_has_an_empty_array_for_context() {
        // Measured: Swarm sends `"context": []` — not an object, not null.
        let c = parse_comment(&json!({ "id": 1, "body": "hello", "context": [] }));
        assert_eq!(c.file, "");
        assert_eq!(c.version, 0);
        assert_eq!(c.body, "hello");
    }

    #[test]
    fn the_unreal_plugins_bookkeeping_is_not_a_comment() {
        // Review 202012, comment 14300: no prose at all.
        let c = parse_comment(&json!({
            "id": 14300,
            "body": "\n\n[UE_Userdata]\nLikedComments=\nReadComments=14313,14315",
            "flags": ["closed"],
            "context": []
        }));
        assert!(c.bookkeeping);
        assert_eq!(c.body, "");
        assert!(c.closed);
    }

    #[test]
    fn an_unreal_asset_comment_keeps_its_prose_and_its_asset() {
        // Review 202012, comment 14313.
        let c = parse_comment(&json!({
            "id": 14313,
            "body": "le cas de 2 geysers n'est pas géré\n\n[UE_Metadata]\nFile=//d/BP_Windser.uasset\nCategory=ActivationBlocked\n",
            "context": []
        }));
        assert_eq!(c.body, "le cas de 2 geysers n'est pas géré");
        assert_eq!(c.asset_file, "//d/BP_Windser.uasset");
        assert_eq!(c.asset_category, "ActivationBlocked");
        assert!(!c.bookkeeping);
    }

    #[test]
    fn an_edited_comment_says_so() {
        let c = parse_comment(&json!({ "id": 2, "body": "x", "edited": 1787908240 }));
        assert!(c.edited);
    }

    #[test]
    fn swarm_names_the_task_states_it_will_accept() {
        // The real refusal, measured on comment 14343.
        let msg = "Swarm refused: Invalid task state transition specified. \
                   Valid transitions are: open";
        assert_eq!(parse_valid_transitions(msg), vec!["open"]);
        let msg = "Valid transitions are: addressed, verified.";
        assert_eq!(parse_valid_transitions(msg), vec!["addressed", "verified"]);
        // Some other failure: no list to be had, and none invented.
        assert!(parse_valid_transitions("Swarm unreachable").is_empty());
    }
}
