//! "Open in editor" support: detect installed text editors, resolve the one
//! Windows uses by default, launch a file in a chosen editor, and materialize a
//! server revision (`p4 print`) to a temp file so depot/shelved/history files
//! can be opened too.

use serde::Serialize;

use crate::p4::{self, P4Conn};

#[derive(Serialize, Clone)]
pub struct Editor {
    pub id: String,
    pub name: String,
    pub path: String,
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

/// First existing candidate path wins.
fn probe(id: &str, name: &str, candidates: &[String]) -> Option<Editor> {
    candidates
        .iter()
        .find(|p| !p.is_empty() && std::path::Path::new(p).is_file())
        .map(|p| Editor { id: id.into(), name: name.into(), path: p.clone() })
}

fn known_editors() -> Vec<Editor> {
    let lad = env("LOCALAPPDATA");
    let pf = env("ProgramFiles");
    let pf86 = env("ProgramFiles(x86)");
    let windir = env("WINDIR");
    let mut out = Vec::new();

    // Notepad ships with Windows — the guaranteed fallback.
    if let Some(e) = probe("notepad", "Notepad", &[format!("{windir}\\System32\\notepad.exe")]) {
        out.push(e);
    }
    if let Some(e) = probe(
        "notepad++",
        "Notepad++",
        &[format!("{pf}\\Notepad++\\notepad++.exe"), format!("{pf86}\\Notepad++\\notepad++.exe")],
    ) {
        out.push(e);
    }
    if let Some(e) = probe(
        "vscode",
        "Visual Studio Code",
        &[
            format!("{lad}\\Programs\\Microsoft VS Code\\Code.exe"),
            format!("{pf}\\Microsoft VS Code\\Code.exe"),
        ],
    ) {
        out.push(e);
    }
    if let Some(e) = probe(
        "vscodium",
        "VSCodium",
        &[format!("{lad}\\Programs\\VSCodium\\VSCodium.exe"), format!("{pf}\\VSCodium\\VSCodium.exe")],
    ) {
        out.push(e);
    }
    if let Some(e) = probe("cursor", "Cursor", &[format!("{lad}\\Programs\\cursor\\Cursor.exe")]) {
        out.push(e);
    }
    if let Some(e) = probe(
        "sublime",
        "Sublime Text",
        &[
            format!("{pf}\\Sublime Text\\sublime_text.exe"),
            format!("{pf}\\Sublime Text 3\\sublime_text.exe"),
        ],
    ) {
        out.push(e);
    }
    // gVim installs under a versioned dir (Vim\vim91\gvim.exe) — scan for it.
    for root in [&pf, &pf86] {
        if let Ok(rd) = std::fs::read_dir(format!("{root}\\Vim")) {
            let mut dirs: Vec<_> = rd.flatten().map(|d| d.path()).collect();
            dirs.sort(); // highest version last
            if let Some(gvim) = dirs
                .iter()
                .rev()
                .map(|d| d.join("gvim.exe"))
                .find(|p| p.is_file())
                .and_then(|p| p.to_str().map(String::from))
            {
                out.push(Editor { id: "gvim".into(), name: "gVim".into(), path: gvim });
                break;
            }
        }
    }
    out
}

/// Installed editors we know how to launch.
#[tauri::command]
pub async fn detect_editors() -> Vec<Editor> {
    tauri::async_runtime::spawn_blocking(known_editors).await.unwrap_or_default()
}

/// The id (from `detect_editors`) of the editor Windows opens .txt files with,
/// or "" when it isn't one we recognize. Read from the registry UserChoice.
#[tauri::command]
pub async fn default_editor_id() -> String {
    tauri::async_runtime::spawn_blocking(|| {
        let mut c = std::process::Command::new("reg");
        c.args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice",
            "/v",
            "ProgId",
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        let out = match c.output() {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_lowercase(),
            Err(_) => return String::new(),
        };
        // e.g. "ProgId REG_SZ Applications\notepad++.exe" / "VSCode.txt" / "txtfile"
        if out.contains("notepad++") {
            "notepad++".into()
        } else if out.contains("vscodium") {
            "vscodium".into()
        } else if out.contains("vscode") || out.contains("code.exe") {
            "vscode".into()
        } else if out.contains("cursor") {
            "cursor".into()
        } else if out.contains("sublime") {
            "sublime".into()
        } else if out.contains("gvim") || out.contains("vim") {
            "gvim".into()
        } else if out.contains("txtfile") || out.contains("notepad") {
            "notepad".into()
        } else {
            String::new()
        }
    })
    .await
    .unwrap_or_default()
}

/// Launch `exe file` detached (the editor owns its window; we don't wait).
#[tauri::command]
pub async fn open_in_editor(exe: String, file: String) -> Result<(), String> {
    if !std::path::Path::new(&file).exists() {
        return Err(format!("File not found: {file}"));
    }
    let mut c = std::process::Command::new(&exe);
    c.arg(&file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.spawn().map_err(|e| format!("failed to launch {exe}: {e}"))?;
    Ok(())
}

/// `p4 print` a server revision to a temp file and return its path. `spec` is a
/// full file spec (`//path`, `//path#4`, `//path@=12345`, `//path#have`).
#[tauri::command]
pub async fn print_to_temp(conn: P4Conn, spec: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Temp name: `<stem>_<rev-tag>.<ext>` — the rev tag keeps two revisions of
        // one file distinct, and the real extension stays LAST so editors pick the
        // right syntax (foo.cpp#4 → p4gui_open_foo_4.cpp, not ..._foo.cpp_4).
        let last = spec.rsplit('/').next().unwrap_or("file");
        let (fname, revpart) = match last.find(['#', '@']) {
            Some(i) => (&last[..i], &last[i..]),
            None => (last, ""),
        };
        let sane = |s: &str| {
            s.replace(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '_', "_")
        };
        let (stem, ext) = match fname.rfind('.') {
            Some(i) => (&fname[..i], &fname[i..]),
            None => (fname, ""),
        };
        let name = format!("{}{}{}", sane(stem), sane(revpart), sane(ext));
        let tmp = std::env::temp_dir().join(format!("p4gui_open_{name}"));
        let tmp_s = tmp.to_str().ok_or("bad temp path")?.to_string();
        p4::run_raw(&conn, &["print", "-q", "-o", &tmp_s, &spec])?;
        if !tmp.is_file() {
            return Err(format!("p4 print produced no file for {spec}"));
        }
        Ok(tmp_s)
    })
    .await
    .map_err(|e| format!("print task failed: {e}"))?
}
