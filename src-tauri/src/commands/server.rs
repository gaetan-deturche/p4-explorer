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


/// The review status of MANY changelists in one request.
///
/// Swarm's `change[]` filter takes a list, and each returned review names the
/// changelists it covers — so N changelists cost one HTTP round trip instead of
/// N. That is what makes it cheap enough to poll: the badges used to need a
/// manual refresh because nothing re-asked, and re-asking per changelist on a
/// timer would have meant a request per changelist every tick.
///
/// One record per changelist that HAS a review: `change`, `id`, `state`,
/// `stateLabel`. Changelists with no review are simply absent.
#[tauri::command]
pub async fn swarm_reviews_for(conn: P4Conn, changes: Vec<String>) -> Res {
    tauri::async_runtime::spawn_blocking(move || {
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        let base = swarm_base(&conn);
        if base.is_empty() {
            return Ok(Vec::new());
        }
        let Some(ticket) = p4::ticket(&conn) else {
            return Ok(Vec::new()); // not logged in → no badges, not an error
        };
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        let wanted: std::collections::HashSet<&str> = changes.iter().map(String::as_str).collect();
        let mut out: Vec<p4::Record> = Vec::new();
        // Chunked: a very long query string would be refused, and 50 keeps it
        // comfortable.
        for batch in changes.chunks(50) {
            let mut url = format!("{base}/api/v9/reviews?max=200&fields=id,state,changes,commits");
            for c in batch {
                url.push_str(&format!("&change[]={c}"));
            }
            let Ok(resp) = client.get(&url).basic_auth(&conn.user, Some(&ticket)).send() else {
                continue; // unreachable Swarm → no badges this round
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(body) = resp.json::<serde_json::Value>() else { continue };
            for rv in body.get("reviews").and_then(|r| r.as_array()).into_iter().flatten() {
                let id = rv.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                let state = rv.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if state.is_empty() {
                    continue;
                }
                // A review covers several changelists; report it for each one the
                // caller asked about, including the review's own id (the shelf
                // changelist) which is what a pending row may be.
                let mut covered: Vec<String> = vec![id.to_string()];
                for key in ["changes", "commits"] {
                    for c in rv.get(key).and_then(|v| v.as_array()).into_iter().flatten() {
                        if let Some(n) = c.as_u64() {
                            covered.push(n.to_string());
                        }
                    }
                }
                for c in covered {
                    if !wanted.contains(c.as_str()) {
                        continue;
                    }
                    let mut rec = p4::Record::new();
                    rec.insert("change".into(), c.into());
                    rec.insert("id".into(), id.into());
                    rec.insert("state".into(), state.clone().into());
                    rec.insert("stateLabel".into(), state_label(&state).into());
                    out.push(rec);
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("swarm-reviews task failed: {e}"))?
}

/// Whether the connection is currently authenticated (`p4 login -s` exits 0).
/// True also when the server needs no login (security level 0).
/// Whether the session is authenticated, plus the p4 error text when it isn't —
/// the caller must tell "no/expired ticket" (→ prompt for a password) apart
/// from e.g. a charset mismatch (→ fix the charset and re-check, no prompt).
#[derive(serde::Serialize)]
pub struct LoginStatus {
    pub ok: bool,
    pub error: String,
}

#[tauri::command]
pub async fn p4_login_status(conn: P4Conn) -> Result<LoginStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // This check gates the password prompt, so a transient auth failure must
        // not reach it: run_status retries auth failures (with an explicit
        // ticket), so when it still reports one, it really is one.
        let (ok, error) = p4::run_status(&conn, &["login", "-s"])?;
        Ok(LoginStatus { ok, error })
    })
    .await
    .map_err(|e| format!("login-status task failed: {e}"))?
}

/// Log in with a password (`p4 login`, password fed on stdin). Errors on bad
/// credentials. The password is used only to obtain a p4 ticket; it is never
/// stored — only p4's own ticket persists.
#[tauri::command]
pub async fn p4_login(conn: P4Conn, password: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        p4::run_raw_stdin(&conn, &["login"], &format!("{password}\n"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("login task failed: {e}"))?
}

/// Which of `paths` exist as directories on this machine — used to flag which
/// workspaces are actually checked out here vs bound to another machine.
#[tauri::command]
pub async fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| !p.is_empty() && std::path::Path::new(p).is_dir())
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// The cached ticket VALUE for this connection (address-preferred, else the
/// first ticket of conn.user). Used to pass `-P` explicitly when p4's own
/// lookup fails despite a valid ticket (multi-edge auth.id keying mismatch).
#[tauri::command]
pub async fn p4_ticket_value(conn: P4Conn) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(p4::ticket(&conn).unwrap_or_default()))
        .await
        .map_err(|e| format!("ticket task failed: {e}"))?
}

/// The user of a cached ticket whose address matches conn.port (`p4 tickets`),
/// or "". Lets adding a server adopt the account you already logged in as (e.g.
/// via P4V) instead of the ambient P4USER, which may not exist on that server.
#[tauri::command]
pub async fn p4_ticket_user(conn: P4Conn) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let want = conn.port.trim().trim_start_matches("ssl:").to_string();
        if want.is_empty() {
            return Ok(String::new());
        }
        let out = p4::run_raw(&conn, &["tickets"]).unwrap_or_default();
        for line in out.lines() {
            // "<address> (<user>) <ticket>"
            let Some((addr, after)) = line.trim().split_once(" (") else {
                continue;
            };
            let Some((user, _)) = after.split_once(") ") else { continue };
            if addr.trim() == want {
                return Ok(user.trim().to_string());
            }
        }
        Ok(String::new())
    })
    .await
    .map_err(|e| format!("ticket-user task failed: {e}"))?
}

/// Trust an SSL server's fingerprint (`p4 trust -y -f`) so subsequent commands
/// don't fail on an unknown/changed fingerprint — needed on first connect to an
/// `ssl:` server.
#[tauri::command]
pub async fn p4_trust(conn: P4Conn) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = p4::base_command(&conn);
        cmd.arg("trust").arg("-y").arg("-f");
        let start = std::time::Instant::now();
        let out = cmd
            .output()
            .map_err(|e| format!("failed to launch p4: {e} (is p4 on PATH?)"))?;
        p4::log_command_err(&["trust", "-y", "-f"], start.elapsed().as_millis(), out.status.success(), &if out.status.success() { String::new() } else { p4::extract_error(&out.stdout, &out.stderr) });
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("trust task failed: {e}"))?
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
