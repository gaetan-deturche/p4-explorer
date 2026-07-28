//! Dev utility: end-to-end test of the Unreal remote-execution client — pings
//! for a running editor and runs a harmless log statement in it.
//! `cargo run --example uediscover` from src-tauri (an example, not a [[bin]],
//! so `tauri dev`'s plain `cargo run` never has to disambiguate).

fn main() {
    // Optional arg: a .py file to run instead of the default connectivity ping.
    let script = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path).expect("script file unreadable"),
        None => "import unreal\nunreal.log_warning('Auger remote-exec connectivity test')\n".into(),
    };
    match p4gui_lib::commands::unreal_remote::run_python_in_editor(&script) {
        Ok(Some(())) => println!("REMOTE OK — executed in the running editor (see its log)"),
        Ok(None) => println!("NO EDITOR FOUND (no pong on 239.0.0.1:6766)"),
        Err(e) => println!("ERROR: {e}"),
    }
}
