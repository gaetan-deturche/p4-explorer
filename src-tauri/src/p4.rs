//! Thin wrapper around the `p4` command line.
//!
//! Everything goes through `p4 -ztag -Mj <cmd>`, which emits one JSON object
//! per output record (field-per-key). We parse each stdout line with serde and
//! split data records from error records (which carry a numeric `severity`).

use serde::Deserialize;
use serde_json::{Map, Value};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

/// App handle for emitting the p4-command log events (set once at startup).
static APP: OnceLock<AppHandle> = OnceLock::new();
pub fn set_app_handle(app: AppHandle) {
    let _ = APP.set(app);
}

/// Emit a `p4-command` event describing a command the app just ran, for the
/// Commands log view. `args` is the subcommand + args (connection globals and
/// `-ztag -Mj` are omitted for readability; stdin, e.g. a password, is never
/// included).
/// `err` carries the failure text so the Commands view can SHOW why a command
/// failed instead of just flagging it red (capped for the UI; "" when ok).
pub fn log_command_err(args: &[&str], ms: u128, ok: bool, err: &str) {
    if let Some(app) = APP.get() {
        let line = format!("p4 {}", args.join(" "));
        let err: String = err.trim().chars().take(500).collect();
        let _ = app
            .emit("p4-command", serde_json::json!({ "line": line, "ms": ms, "ok": ok, "err": err }));
    }
}

/// The human-readable error of a failed p4 run: stderr if present, else the
/// `data` of error-severity records on stdout (`-ztag -Mj` reports errors there).
pub fn extract_error(stdout: &[u8], stderr: &[u8]) -> String {
    let se = String::from_utf8_lossy(stderr);
    let se = se.trim();
    if !se.is_empty() {
        return se.to_string();
    }
    let mut msgs: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line.trim()) {
            let sev = obj.get("severity").and_then(value_as_i64).unwrap_or(0);
            if sev >= E_FAILED {
                if let Some(d) = obj.get("data").and_then(|d| d.as_str()) {
                    msgs.push(d.trim().to_string());
                }
            }
        }
    }
    msgs.join("\n")
}

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
    /// Per-server character set. "" = ambient (P4CHARSET as-is); "none" = force a
    /// non-unicode client (clear P4CHARSET); any other value → `-C <charset>`
    /// (unicode). Lets one server be unicode and another not.
    #[serde(default)]
    pub charset: String,
    /// Explicit auth ticket passed as `-P`. Normally empty (p4 looks tickets up
    /// itself); set when the automatic lookup fails even though a valid ticket
    /// exists — e.g. multi-edge servers whose auth.id keying doesn't match the
    /// edge a connection lands on (seen on Epic's licensee cluster).
    #[serde(default)]
    pub ticket: String,
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
        if !self.ticket.is_empty() {
            a.push("-P".into());
            a.push(self.ticket.clone());
        }
        a
    }
}

/// p4 message severity levels (from the C++ API `Error::Severity`).
const E_WARN: i64 = 2;
const E_FAILED: i64 = 3;

/// Apply the connection's charset choice to a p4 command via `-C <charset>`
/// AND the P4CHARSET environment variable. "" means leave the ambient P4CHARSET
/// alone; any explicit value ("none" for a non-unicode client, "utf8" for a
/// unicode one, …) overrides it — including a P4CHARSET set via `p4 set`
/// (registry), which clearing the env would NOT. The env var matters for
/// `sync --parallel`: the parallel transfer threads open their own connections
/// and re-read P4CHARSET from env/registry, IGNORING `-C` — with a registry
/// utf8 against a non-unicode server every thread fails "Unicode clients
/// require a unicode enabled server" and the sync transfers nothing.
fn apply_charset(cmd: &mut Command, conn: &P4Conn) {
    if !conn.charset.is_empty() {
        cmd.arg("-C").arg(&conn.charset);
        cmd.env("P4CHARSET", &conn.charset);
    }
}

pub fn base_command(conn: &P4Conn) -> Command {
    let mut cmd = Command::new("p4");
    for g in conn.global_args() {
        cmd.arg(g);
    }
    apply_charset(&mut cmd, conn);
    // Also hand the ticket to the child through the environment: `sync --parallel`
    // transfer threads open their own connections and re-authenticate from
    // env/tickets, IGNORING `-P` (same blind spot as `-C`). Servers whose ticket
    // lookup is unreliable per-connection (multi-edge auth.id keying) otherwise
    // fail those threads with "P4PASSWD invalid or unset".
    if !conn.ticket.is_empty() {
        cmd.env("P4PASSWD", &conn.ticket);
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

/// How many times to attempt a command that fails authentication. Auth failures
/// here are TRANSIENT and random, not command-specific: every p4 invocation is a
/// fresh connection, and on a clustered server (e.g. Epic's licensee cluster,
/// whose ticket is keyed by auth.id) an individual connection can fail to
/// validate the session while the next one succeeds — the identical command
/// alternates between working and "P4PASSWD invalid or unset" minutes apart.
const AUTH_ATTEMPTS: usize = 3;
const AUTH_RETRY_PAUSE_MS: u64 = 200;

/// Run a p4 command (built by `build`, so each attempt gets a fresh Command) and
/// retry it on an authentication failure, up to `AUTH_ATTEMPTS`. Retries pass the
/// cached ticket explicitly, and each attempt is logged with a `(auth retry N)`
/// marker so the Commands view shows what happened rather than mystery
/// duplicates. Every p4 entry point funnels through here.
fn run_output(
    conn: &P4Conn,
    args: &[&str],
    build: impl Fn(&P4Conn) -> std::io::Result<std::process::Output>,
) -> Result<std::process::Output, String> {
    let mut c = conn.clone();
    let mut attempt = 0usize;
    loop {
        let start = Instant::now();
        let out = build(&c).map_err(|e| format!("failed to launch p4: {e} (is p4 on PATH?)"))?;
        let ok = out.status.success();
        let err = if ok { String::new() } else { extract_error(&out.stdout, &out.stderr) };
        let mut logged: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if attempt > 0 {
            logged.push(format!("(auth retry {attempt})"));
        }
        let refs: Vec<&str> = logged.iter().map(String::as_str).collect();
        log_command_err(&refs, start.elapsed().as_millis(), ok, &err);

        attempt += 1;
        // Only retry when the command produced NOTHING: an auth failure after
        // partial output means it did some work, and re-running could apply a
        // mutation twice.
        let produced_output = !out.stdout.is_empty() && has_data_line(&out.stdout);
        if ok || !is_auth_error(&err) || produced_output || attempt >= AUTH_ATTEMPTS {
            return Ok(out);
        }
        // Pass the cached ticket explicitly from here on (the automatic lookup is
        // what's flaky), and give the cluster a moment before trying again.
        if c.ticket.is_empty() {
            if let Some(t) = ticket_no_retry(conn) {
                c.ticket = t;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(AUTH_RETRY_PAUSE_MS));
    }
}

/// Run `p4 <global> <args>` WITHOUT `-ztag -Mj` and return raw stdout. For
/// commands whose output is plain text (diff2, print, set), not tagged records.
pub fn run_raw(conn: &P4Conn, args: &[&str]) -> Result<String, String> {
    let out = run_output(conn, args, |c| {
        let mut cmd = base_command(c);
        for a in args {
            cmd.arg(a);
        }
        cmd.output()
    })?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Did the command emit any NON-error output? For tagged output that means a
/// record without an error severity; for plain output, any non-empty line. Used
/// to keep the auth retry from re-running something that already did work.
fn has_data_line(stdout: &[u8]) -> bool {
    for line in String::from_utf8_lossy(stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(obj)) => {
                let sev = obj.get("severity").and_then(value_as_i64).unwrap_or(0);
                if sev < E_FAILED {
                    return true; // a data (or warning) record
                }
            }
            _ => return true, // plain text output
        }
    }
    false
}

/// Run a plain (untagged) command for its EXIT STATUS, with the shared auth
/// retry: returns (succeeded, error text). For checks like `login -s` / `trust`
/// where the output doesn't matter but a transient auth failure must not be
/// mistaken for a real answer.
pub fn run_status(conn: &P4Conn, args: &[&str]) -> Result<(bool, String), String> {
    let out = run_output(conn, args, |c| {
        let mut cmd = base_command(c);
        for a in args {
            cmd.arg(a);
        }
        cmd.output()
    })?;
    let ok = out.status.success();
    let err = if ok { String::new() } else { extract_error(&out.stdout, &out.stderr) };
    Ok((ok, err))
}

/// `p4 tickets` without the retry wrapper — the retry path calls this to resolve
/// a ticket, so it must not recurse (it's a local file read; it can't auth-fail).
fn ticket_no_retry(conn: &P4Conn) -> Option<String> {
    let mut cmd = base_command(conn);
    cmd.arg("tickets");
    let out = cmd.output().ok()?;
    parse_ticket(&String::from_utf8_lossy(&out.stdout), conn)
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
    let start = Instant::now();
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    log_command_err(args, start.elapsed().as_millis(), out.status.success(), &if out.status.success() { String::new() } else { extract_error(&out.stdout, &out.stderr) });
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
    parse_ticket(&run_raw(conn, &["tickets"]).ok()?, conn)
}

/// Pick this connection's ticket out of `p4 tickets` output: prefer the entry
/// whose address matches the port, else the first one for `conn.user`.
fn parse_ticket(out: &str, conn: &P4Conn) -> Option<String> {
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
    let start = Instant::now();
    let out = cmd
        .output()
        .map_err(|e| format!("failed to launch p4: {e} (is p4 on PATH?)"))?;
    log_command_err(args, start.elapsed().as_millis(), out.status.success(), &if out.status.success() { String::new() } else { extract_error(&out.stdout, &out.stderr) });
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `p4 -ztag -Mj <global> <args>` and return the data records.
///
/// Error records (severity >= E_FAILED) are collected; if there are no data
/// records we return them (joined) as an `Err`. Warnings are dropped silently.
/// Parse `-ztag -Mj` stdout into records, turning a failure record into an
/// `Err` only when there are no data records (data masks errors — fine for
/// reads; `run_strict` is the strict variant for mutations).
fn parse_records(stdout: &[u8], success: bool, stderr: &[u8]) -> Result<Vec<Record>, String> {
    let (records, _errors) = parse_records_full(stdout, success, stderr)?;
    Ok(records)
}

/// As `parse_records`, but ALSO returns the error strings that data records
/// would otherwise mask — for batch mutations (e.g. a multi-file sync) where a
/// partial failure must not disappear behind the successes.
fn parse_records_full(
    stdout: &[u8],
    success: bool,
    stderr: &[u8],
) -> Result<(Vec<Record>, Vec<String>), String> {
    parse_tagged(stdout, success, stderr, false)
}

/// The tagged-output parser. `keep_warnings` decides whether severity-2 messages
/// join the returned message list or are dropped as noise.
fn parse_tagged(
    stdout: &[u8],
    success: bool,
    stderr: &[u8],
    keep_warnings: bool,
) -> Result<(Vec<Record>, Vec<String>), String> {
    let stdout = String::from_utf8_lossy(stdout);
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
                if keep_warnings {
                    if let Some(d) = obj.get("data").and_then(|d| d.as_str()) {
                        errors.push(d.trim().to_string());
                    }
                }
                continue; // warning, not fatal, not data
            }
        }
        records.push(obj.clone());
    }

    if records.is_empty() {
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        if !success {
            let stderr = String::from_utf8_lossy(stderr);
            let stderr = stderr.trim();
            if !stderr.is_empty() {
                return Err(stderr.to_string());
            }
        }
    }
    Ok((records, errors))
}

pub fn run(conn: &P4Conn, args: &[&str]) -> Result<Vec<Record>, String> {
    run_full(conn, args).map(|(records, _errors)| records)
}

/// True for the "not authenticated" family — the failure that appears at random
/// on servers whose per-connection ticket lookup is unreliable (multi-edge
/// auth.id keying), where an immediate retry usually lands on a good connection.
pub fn is_auth_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("p4passwd") || m.contains("your session has expired")
}

/// As `run`, but also surfaces the per-record errors a batch's data records
/// would mask (for multi-file mutations — partial failures must be reported).
/// Retries ONCE on an authentication error: each p4 invocation is a fresh
/// connection, and on multi-edge servers the ticket lookup fails at random —
/// one retry turns a red line + stale pane into an invisible hiccup.
pub fn run_full(conn: &P4Conn, args: &[&str]) -> Result<(Vec<Record>, Vec<String>), String> {
    let out = tagged_output(conn, args)?;
    parse_records_full(&out.stdout, out.status.success(), &out.stderr)
}

/// As `run_full`, but the returned messages ALSO include p4's warnings.
///
/// A per-file refusal is a warning, not a failure: `undo -n //nope@=123` prints
/// "no such file(s)." with severity 2 and exit status 0. `run_full` drops those,
/// which is right for a command read for its data and wrong for one whose
/// per-file outcome IS the answer — there, a dropped warning turns "p4 refused
/// this file" into a silent success.
pub fn run_notes(conn: &P4Conn, args: &[&str]) -> Result<(Vec<Record>, Vec<String>), String> {
    let out = tagged_output(conn, args)?;
    parse_tagged(&out.stdout, out.status.success(), &out.stderr, true)
}

/// `p4 -ztag -Mj <args>` with the shared auth retry. The retry lives in
/// `run_output`; tagged commands are idempotent reads or single-shot mutations
/// that fail closed on an auth error, so re-running them is safe.
fn tagged_output(conn: &P4Conn, args: &[&str]) -> Result<std::process::Output, String> {
    run_output(conn, args, |c| {
        let mut cmd = Command::new("p4");
        cmd.arg("-ztag").arg("-Mj");
        for g in c.global_args() {
            cmd.arg(g);
        }
        apply_charset(&mut cmd, c);
        if !c.ticket.is_empty() {
            cmd.env("P4PASSWD", &c.ticket); // see base_command
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
        cmd.output()
    })
}

/// Like `run`, but spawns the child and records its PID in `pid_slot` so a long
/// command (the offline-changes scan) can be killed mid-flight to release its
/// server locks — otherwise it blocks interactive writes (submit, reopen).
pub fn run_killable(
    conn: &P4Conn,
    args: &[&str],
    pid_slot: &std::sync::Arc<std::sync::Mutex<Option<u32>>>,
) -> Result<Vec<Record>, String> {
    // Shares the auth retry (run_output) while still publishing the child PID so
    // the scan can be killed mid-flight.
    let out = run_output(conn, args, |c| {
        let mut cmd = Command::new("p4");
        cmd.arg("-ztag").arg("-Mj");
        for g in c.global_args() {
            cmd.arg(g);
        }
        apply_charset(&mut cmd, c);
        if !c.ticket.is_empty() {
            cmd.env("P4PASSWD", &c.ticket); // see base_command
        }
        for a in args {
            cmd.arg(a);
        }
        cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn()?;
        let id = child.id();
        *pid_slot.lock().unwrap() = Some(id);
        let out = child.wait_with_output();
        let mut s = pid_slot.lock().unwrap();
        if *s == Some(id) {
            *s = None;
        }
        out
    })?;
    parse_records(&out.stdout, out.status.success(), &out.stderr)
}

/// Like `run`, but a failure record (severity >= E_FAILED) is ALWAYS surfaced as
/// an `Err`, even when the command also emitted data records. `run` lets data
/// records mask errors (fine for reads); for a mutation like `submit` that hides
/// real failures — e.g. "change has shelved files" arrives after progress
/// records, so the submit silently looks successful.
pub fn run_strict(conn: &P4Conn, args: &[&str]) -> Result<Vec<Record>, String> {
    // Auth retry via run_output: it only re-runs when the command produced NO
    // output and failed authentication, so a mutation is never applied twice.
    let out = run_output(conn, args, |c| {
        let mut cmd = Command::new("p4");
        cmd.arg("-ztag").arg("-Mj");
        for g in c.global_args() {
            cmd.arg(g);
        }
        apply_charset(&mut cmd, c);
        if !c.ticket.is_empty() {
            cmd.env("P4PASSWD", &c.ticket); // see base_command
        }
        for a in args {
            cmd.arg(a);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.output()
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut records: Vec<Record> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            continue;
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
                continue;
            }
        }
        records.push(obj.clone());
    }

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
