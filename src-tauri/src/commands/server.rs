//! Server / connection scope: streams, switching, Swarm URL, ambient P4PORT,
//! and the release-build flag.

use super::{run, v, Res};
use crate::p4::{self, P4Conn};

/// The configured Swarm base URL (`p4 property -l -n P4.Swarm.URL`), or empty.
#[tauri::command]
pub async fn swarm_url(conn: P4Conn) -> Result<String, String> {
    Ok(swarm_base(&conn))
}

/// Swarm base URL with any trailing slash removed (empty if unconfigured).
fn swarm_base(conn: &P4Conn) -> String {
    let out = p4::run_raw(conn, &["property", "-l", "-n", "P4.Swarm.URL"]).unwrap_or_default();
    out.lines()
        .next()
        .and_then(|l| l.splitn(2, '=').nth(1))
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// A changelist's Swarm review status.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewInfo {
    pub id: u64, // 0 = requested but not yet created/linked by Swarm
    pub state: String,
    pub state_label: String,
}

/// Human label for a Swarm review state.
fn state_label(state: &str) -> String {
    match state {
        "needsReview" => "Needs Review",
        "needsRevision" => "Needs Revision",
        "approved" | "approved:commit" => "Approved",
        "rejected" => "Rejected",
        "archived" => "Archived",
        "requested" => "Review Requested",
        other => {
            let mut c = other.chars();
            return match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            };
        }
    }
    .to_string()
}

/// The Swarm review associated with a changelist, or None when there is none
/// (or Swarm/ticket is unavailable). Queries `GET /api/v9/reviews?change[]=<cl>`
/// authenticated with the user's P4 ticket — the review is linked by change, not
/// by any `#review` marker in the description (Swarm doesn't rewrite the pending
/// CL). Network/HTTP errors return None so a missing badge never breaks the list.
#[tauri::command]
pub async fn swarm_review(conn: P4Conn, change: String) -> Result<Option<ReviewInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base = swarm_base(&conn);
        if base.is_empty() {
            return Ok(None);
        }
        let Some(ticket) = p4::ticket(&conn) else {
            return Ok(None); // not logged in → can't auth to Swarm
        };
        let url = format!("{base}/api/v9/reviews?change[]={change}&fields=id,state&max=1");
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
        {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };
        let resp = match client.get(&url).basic_auth(&conn.user, Some(&ticket)).send() {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(None), // unreachable / 401 → just no badge
        };
        let body: serde_json::Value = match resp.json() {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let Some(review) = body
            .get("reviews")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
        else {
            return Ok(None); // no review for this change
        };
        let id = review.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let state = review
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if state.is_empty() {
            return Ok(None);
        }
        Ok(Some(ReviewInfo {
            id,
            state_label: state_label(&state),
            state,
        }))
    })
    .await
    .map_err(|e| format!("swarm-review task failed: {e}"))?
}

/// All streams on the server (`p4 streams`).
#[tauri::command]
pub async fn p4_streams(conn: P4Conn) -> Res {
    run(conn, v(&["streams"])).await
}

/// Switch the connection's client to a different stream (`p4 switch <stream>`).
/// A workspace write; p4 refuses if files are open (surfaced as an error).
#[tauri::command]
pub async fn p4_switch(conn: P4Conn, stream: String) -> Res {
    run(conn, v(&["switch", &stream])).await
}

/// The configured P4PORT (`p4 set P4PORT`), stripped of its ` (origin)` tag —
/// seeds the server dropdown with the ambient default.
#[tauri::command]
pub async fn p4_env_port(conn: P4Conn) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = p4::run_raw(&conn, &["set", "P4PORT"]).unwrap_or_default();
        let line = out.lines().next().unwrap_or("");
        let val = line.strip_prefix("P4PORT=").unwrap_or("").trim();
        let val = match val.rfind(" (") {
            Some(i) => val[..i].trim(),
            None => val,
        };
        Ok(val.to_string())
    })
    .await
    .map_err(|e| format!("env-port task failed: {e}"))?
}

/// True only for tagged release builds: the release workflow compiles with the
/// AUGER_RELEASE env set. Dev/local (`--no-bundle`) builds return false so the
/// front-end skips the auto-update check (they carry a placeholder version).
#[tauri::command]
pub fn is_release_build() -> bool {
    option_env!("AUGER_RELEASE").is_some()
}
