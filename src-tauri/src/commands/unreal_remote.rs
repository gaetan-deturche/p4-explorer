//! Minimal client for Unreal's Python remote-execution protocol (the one
//! `remote_execution.py` implements; see PythonScriptRemoteExecution.cpp):
//! UDP-multicast discovery (ping → pong), then an `open_connection` handshake
//! where WE host a TCP socket the editor connects to, then one `command`
//! message → `command_result`. Used to run a diff inside an ALREADY-RUNNING
//! editor instead of booting a second editor instance.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 0, 0, 1);
const MULTICAST_PORT: u16 = 6766;
const BIND_ADDR: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
const MAGIC: &str = "ue_py";
const VERSION: u64 = 1;
const RECV_CHUNK: usize = 8192;

fn msg(node: &str, type_: &str, dest: Option<&str>, data: Option<Value>) -> Vec<u8> {
    let mut m = json!({ "version": VERSION, "magic": MAGIC, "type": type_, "source": node });
    if let Some(d) = dest {
        m["dest"] = json!(d);
    }
    if let Some(d) = data {
        m["data"] = d;
    }
    m.to_string().into_bytes()
}

/// The multicast discovery socket: SO_REUSEADDR (the editor holds the same
/// port), bound to 127.0.0.1:6766, member of the group, loopback enabled.
fn discovery_socket() -> std::io::Result<UdpSocket> {
    let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    s.set_reuse_address(true)?;
    s.bind(&SocketAddr::from((BIND_ADDR, MULTICAST_PORT)).into())?;
    let sock: UdpSocket = s.into();
    sock.join_multicast_v4(&MULTICAST_GROUP, &BIND_ADDR)?;
    sock.set_multicast_loop_v4(true)?;
    sock.set_multicast_ttl_v4(0)?; // local host only (matches the editor default)
    Ok(sock)
}

/// Run `script` (a multi-statement Python source, ExecuteFile mode) in a
/// running editor. Ok(None) = no editor found (caller should fall back);
/// Ok(Some(())) = executed successfully; Err = editor found but the run failed.
pub fn run_python_in_editor(script: &str) -> Result<Option<()>, String> {
    let node = format!("auger-{}", std::process::id());
    let group = SocketAddr::from((MULTICAST_GROUP, MULTICAST_PORT));

    let sock = discovery_socket().map_err(|e| format!("multicast socket: {e}"))?;
    sock.set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| e.to_string())?;

    // Discover: ping every 250ms for up to ~1.5s, take the first pong.
    let mut remote: Option<String> = None;
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut buf = [0u8; RECV_CHUNK];
    while Instant::now() < deadline && remote.is_none() {
        let _ = sock.send_to(&msg(&node, "ping", None, None), group);
        while let Ok((n, _)) = sock.recv_from(&mut buf) {
            if let Ok(v) = serde_json::from_slice::<Value>(&buf[..n]) {
                let from = v["source"].as_str().unwrap_or_default();
                if v["magic"] == MAGIC && v["type"] == "pong" && from != node && !from.is_empty() {
                    remote = Some(from.to_string());
                    break;
                }
            }
        }
    }
    let Some(remote) = remote else {
        return Ok(None); // no running editor with remote execution
    };

    // Host the TCP command socket on an ephemeral port; ask the editor to connect.
    let listener = TcpListener::bind((BIND_ADDR, 0)).map_err(|e| format!("tcp listen: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;

    let open = msg(
        &node,
        "open_connection",
        Some(&remote),
        Some(json!({ "command_ip": BIND_ADDR.to_string(), "command_port": port })),
    );
    let mut stream = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let _ = sock.send_to(&open, group);
        std::thread::sleep(Duration::from_millis(100));
        if let Ok((s, _)) = listener.accept() {
            stream = Some(s);
            break;
        }
    }
    let Some(mut stream) = stream else {
        return Err("the running editor did not open a command connection".into());
    };
    stream.set_nodelay(true).ok();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    // One command, one result.
    let cmd = msg(
        &node,
        "command",
        Some(&remote),
        Some(json!({ "command": script, "unattended": true, "exec_mode": "ExecuteFile" })),
    );
    stream.write_all(&cmd).map_err(|e| format!("send command: {e}"))?;

    let mut data = Vec::new();
    loop {
        let mut chunk = [0u8; RECV_CHUNK];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&chunk[..n]);
                if n < RECV_CHUNK {
                    break; // same short-read framing as remote_execution.py
                }
            }
            Err(e) => return Err(format!("read result: {e}")),
        }
    }
    let _ = sock.send_to(&msg(&node, "close_connection", Some(&remote), None), group);

    let v: Value = serde_json::from_slice(&data).map_err(|e| format!("bad result: {e}"))?;
    if v["type"] != "command_result" {
        return Err(format!("unexpected reply type {}", v["type"]));
    }
    if v["data"]["success"].as_bool() == Some(true) {
        Ok(Some(()))
    } else {
        Err(v["data"]["result"].as_str().unwrap_or("remote command failed").to_string())
    }
}
