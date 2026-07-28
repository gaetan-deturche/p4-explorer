//! Dev utility: end-to-end test of the Unreal remote-execution client — pings
//! for a running editor and runs a harmless log statement in it.
//! `cargo run --bin uediscover` from src-tauri.

fn main() {
    let script = "import unreal\nunreal.log_warning('Auger remote-exec connectivity test')\n";
    match p4gui_lib::commands::unreal_remote::run_python_in_editor(script) {
        Ok(Some(())) => println!("REMOTE OK — executed in the running editor (see its log)"),
        Ok(None) => println!("NO EDITOR FOUND (no pong on 239.0.0.1:6766)"),
        Err(e) => println!("ERROR: {e}"),
    }
}
