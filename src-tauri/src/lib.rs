mod commands;
mod index;
mod p4;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // SQLite DB in the app data dir: file index (+ future caches).
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir).ok();
            let db = rusqlite::Connection::open(dir.join("p4gui.db"))?;
            index::init_schema(&db)?;
            app.manage(index::AppState::new(db));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::p4_info,
            commands::p4_clients,
            commands::p4_dirs,
            commands::p4_files,
            commands::p4_changes,
            commands::p4_pending,
            commands::p4_have_change,
            commands::p4_describe,
            commands::p4_filelog,
            commands::p4_fstat,
            commands::p4_sync,
            commands::p4_sync_stream,
            commands::sync_cancel,
            commands::p4_search,
            commands::p4_diff2,
            commands::open_diff,
            commands::p4_describe_shelved,
            commands::p4_diff_shelved,
            commands::open_diff_shelved,
            commands::p4_opened,
            commands::p4_diff_local,
            commands::open_diff_local,
            commands::p4_streams,
            commands::p4_depots,
            commands::p4_switch,
            commands::p4_submit,
            commands::p4_shelve_delete,
            commands::p4_shelve,
            commands::p4_request_review,
            commands::swarm_url,
            commands::list_local_dir,
            index::index_status,
            index::index_build,
            index::index_search,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
