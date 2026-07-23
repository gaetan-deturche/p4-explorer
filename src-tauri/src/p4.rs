//! Thin wrapper around the `p4` command line.
//!
//! Everything goes through `p4 -ztag -Mj <cmd>`, which emits one JSON object
//! per output record (field-per-key). We parse each stdout line with serde and
//! split data records from error records (which carry a numeric `severity`).

use serde::Deserialize;
use serde_json::{Map, Value};
use std::process::Command;

/// A single tagged output record: a JSON object of field -> value.
pub type Record = Map<String, Value>;

/// Connection context supplied by the front-end. Empty fields fall back to the
/// ambient p4 environment (P4PORT/P4USER/P4CLIENT, .p4config, tickets).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct P4Conn {
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub client: String,
    /// Working directory used for client resolution (`p4 -d`). Handy for the
    /// hybrid git+p4 layouts where the shell cwd is outside the client root.
    #[serde(default)]
    pub cwd: String,
}

impl P4Conn {
    fn global_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if !self.port.is_empty() {
            a.push("-p".into());
            a.push(self.port.clone());
        }
        if !self.user.is_empty() {
            a.push("-u".into());
            a.push(self.user.clone());
        }
        if !self.client.is_empty() {
            a.push("-c".into());
            a.push(self.client.clone());
        }
        if !self.cwd.is_empty() {
            a.push("-d".into());
            a.push(self.cwd.clone());
        }
        a
    }
}

/// p4 message severity levels (from the C++ API `Error::Severity`).
const E_WARN: i64 = 2;
const E_FAILED: i64 = 3;

pub fn base_command(conn: &P4Conn) -> Command {
    let mut cmd = Command::new("p4");
    for g in conn.global_args() {
        cmd.arg(g);
    }
    // Don't flash a console window when spawning p4 from the GUI process.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Run `p4 <global> <args>` WITHOUT `-ztag -Mj` and return raw stdout. For
/// commands whose output is plain text (diff2, print, set), not tagged records.
pub fn run_raw(conn: &P4Conn, args: &[&str]) -> Result<String, String> {
    let mut cmd = base_command(conn);
    for a in args {
        cmd.arg(a);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to launch p4: {e} (is p4 on PATH?)"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `p4 <global> <args>` feeding `input` on stdin (for `... -i` spec forms).
/// Returns stdout; errors with stderr on failure.
pub fn run_raw_stdin(conn: &P4Conn, args: &[&str], input: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut cmd = base_command(conn);
    for a in args {
        cmd.arg(a);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("failed to launch p4: {e}"))?;
    if let Some(mut si) = child.stdin.take() {
        si.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            return Err(err.trim().to_string());
        }
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The cached P4 ticket for this connection's user (`p4 tickets`), if any.
/// Used as the password for Swarm REST Basic auth (`user:ticket`). Returns None
/// when not logged in. Output lines look like: `<address> (<user>) <ticket>`.
pub fn ticket(conn: &P4Conn) -> Option<String> {
    let out = run_raw(conn, &["tickets"]).ok()?;
    let want_user = conn.user.trim();
    let port_tail = conn.port.trim().trim_start_matches("ssl:");
    let mut fallback: Option<String> = None;
    for line in out.lines() {
        let line = line.trim();
        let Some((addr, after)) = line.split_once(" (") else { continue };
        let Some((user, tick)) = after.split_once(") ") else { continue };
        let tick = tick.trim();
        if tick.is_empty() {
            continue;
        }
        if !want_user.is_empty() && user.trim() != want_user {
            continue;
        }
        // User matches (or no filter). Prefer the ticket whose address matches
        // the connection's port; otherwise keep the first as a fallback.
        if !port_tail.is_empty() && addr.trim().contains(port_tail) {
            return Some(tick.to_string());
        }
        if fallback.is_none() {
            fallback = Some(tick.to_string());
        }
    }
    fallback
}

/// Like `run_raw`, but with P4DIFF cleared so `p4 diff` writes the unified diff
/// to stdout instead of launching the external GUI diff tool.
pub fn run_raw_stdout_diff(conn: &P4Conn, args: &[&str]) -> Result<String, String> {
    let mut cmd = base_command(conn);
    cmd.env("P4DIFF", "");
    for a in args {
        cmd.arg(a);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to launch p4: {e} (is p4 on PATH?)"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `p4 -ztag -Mj <global> <args>` and return the data records.
///
/// Error records (severity >= E_FAILED) are collected; if there are no data
/// records we return them (joined) as an `Err`. Warnings are dropped silently.
pub fn run(conn: &P4Conn, args: &[&str]) -> Result<Vec<Record>, String> {
    let mut cmd = Command::new("p4");
    cmd.arg("-ztag").arg("-Mj");
    for g in conn.global_args() {
        cmd.arg(g);
    }
    for a in args {
        cmd.arg(a);
    }

    // Don't flash a console window when spawning p4 from the GUI process.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let out = cmd
        .output()
        .map_err(|e| format!("failed to launch p4: {e} (is p4 on PATH?)"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut records: Vec<Record> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let val: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // non-JSON noise; ignore
        };
        let Some(obj) = val.as_object() else { continue };

        if let Some(sev) = obj.get("severity").and_then(value_as_i64) {
            if sev >= E_FAILED {
                if let Some(d) = obj.get("data").and_then(|d| d.as_str()) {
                    errors.push(d.trim().to_string());
                }
                continue;
            }
            if sev >= E_WARN {
                continue; // warning, not fatal, not data
            }
        }
        records.push(obj.clone());
    }

    if records.is_empty() {
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.trim();
            if !stderr.is_empty() {
                return Err(stderr.to_string());
            }
        }
    }
    Ok(records)
}

/// Accept a severity encoded as either a JSON number or a numeric string.
fn value_as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
}

/// Several p4 commands pack list results into keys suffixed by row index:
/// filelog uses `rev0`, `change0`, `action0`, `desc0`, ...; describe uses
/// `depotFile0`, `action0`, `rev0`, `fileSize0`, .... Explode such a record
/// into one flat record per row.
///
/// `anchor` is a key that is guaranteed present for every row (`"rev"` for
/// filelog, `"depotFile"` for describe); iteration stops when `{anchor}{i}`
/// is absent. Non-indexed keys (no trailing digit) are treated as shared
/// header fields and copied onto every row — e.g. the top-level `depotFile`
/// in filelog, or `change`/`desc`/`user` in describe.
pub fn explode_indexed(rec: &Record, anchor: &str) -> Vec<Record> {
    // Header fields: keys with no trailing digit and no comma (integration
    // sub-arrays like "file0,0" are always skipped).
    let shared: Record = rec
        .iter()
        .filter(|(k, _)| {
            !k.contains(',') && k.chars().last().map(|c| !c.is_ascii_digit()) == Some(true)
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let mut rows = Vec::new();
    let mut i = 0usize;
    loop {
        if !rec.contains_key(&format!("{anchor}{i}")) {
            break;
        }
        let suffix = i.to_string();
        let mut out = shared.clone();
        for (k, v) in rec.iter() {
            if k.contains(',') {
                continue; // integration sub-array
            }
            let Some(base) = k.strip_suffix(&suffix) else {
                continue;
            };
            // Reject e.g. "rev11" when suffix is "1": the char before the
            // suffix must not itself be a digit.
            if base.is_empty() || base.chars().last().map(|c| c.is_ascii_digit()) == Some(true) {
                continue;
            }
            out.insert(base.to_string(), v.clone());
        }
        rows.push(out);
        i += 1;
    }
    rows
}
