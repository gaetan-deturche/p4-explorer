//! Tauri commands exposed to the front-end, grouped by feature.
//!
//! These are `async` on purpose: a synchronous `#[tauri::command]` runs on the
//! main thread, so a blocking `p4` subprocess call would serialize every invoke
//! and freeze the webview until all of them finish (making concurrent front-end
//! requests pointless). Each command instead offloads the blocking `p4::run` to
//! a blocking thread pool via `spawn_blocking`, so multiple invokes run in
//! parallel and each result returns — and paints — independently.

use crate::p4::{self, P4Conn, Record};

mod annotate;
mod browse;
mod clipboard;
mod diff;
mod diffwin;
mod editor;
mod gitbin;
mod histwin;
mod merge;
mod openfiles;
mod patch;
mod pending;
mod server;
mod swarm;
mod sync;
mod undo;
pub mod unreal_remote; // pub: exercised by the uediscover dev bin

// Re-export every command so the registration in `lib.rs` can keep referring to
// them as `commands::<name>` regardless of which submodule they live in.
pub use annotate::*;
pub use browse::*;
pub use clipboard::*;
pub use diff::*;
pub use diffwin::*;
pub use editor::*;
pub use histwin::*;
pub use merge::*;
pub use openfiles::*;
pub use patch::*;
pub use pending::*;
pub use server::*;
pub use swarm::*;
pub use sync::*;
pub use undo::*;

/// Result of a tagged-record `p4` command.
pub(crate) type Res = Result<Vec<Record>, String>;

/// Run `p4` off the main thread so commands don't serialize / block the UI.
pub(crate) async fn run(conn: P4Conn, args: Vec<String>) -> Res {
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        p4::run(&conn, &refs)
    })
    .await
    .map_err(|e| format!("p4 task failed: {e}"))?
}

/// Build an owned arg vector from string literals.
pub(crate) fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}
