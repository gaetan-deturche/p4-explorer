//! Server / connection scope: streams, switching, Swarm URL, ambient P4PORT,
//! and the release-build flag.

use super::{run, v, Res};
use crate::p4::{self, P4Conn};

/// The configured Swarm base URL (`p4 property -l -n P4.Swarm.URL`), or empty.
#[tauri::command]
pub async fn swarm_url(conn: P4Conn) -> Result<String, String> {
    let out = p4::run_raw(&conn, &["property", "-l", "-n", "P4.Swarm.URL"])?;
    let url = out
        .lines()
        .next()
        .and_then(|l| l.splitn(2, '=').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Ok(url)
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
