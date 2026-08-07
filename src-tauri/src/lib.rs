pub mod commands; // pub: the uediscover dev bin exercises unreal_remote directly
mod index;
mod wingeom;
mod merge3;
mod p4;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            p4::set_app_handle(app.handle().clone()); // for p4-command log events
            // SQLite DB in the app data dir: file index (+ future caches).
            // AUGER_DATA_DIR: dev aid — run against a copy of the app data so
            // a dev instance never contends with the installed app's live DB.
            let dir = match std::env::var("AUGER_DATA_DIR") {
                Ok(d) if !d.is_empty() => std::path::PathBuf::from(d),
                _ => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&dir).ok();
            // TWO connections to the same file, in WAL mode: the tiny cache
            // table (source of truth for every view) must never queue behind
            // the multi-million-row file index. One mutex-guarded connection
            // made every cache read wait ~1s at boot while the index counted
            // or loaded; WAL lets the pair read/write concurrently.
            //
            // A cache must never brick the app: if the file won't open cleanly
            // (seen live: a second process got SQLITE_CORRUPT from the WAL
            // handshake while the first ran fine), boot on an in-memory pair —
            // everything works, refilled from the server, nothing persists
            // that session, and the real file is left untouched for the next
            // clean start.
            let open_pair = || -> Result<(rusqlite::Connection, rusqlite::Connection), String> {
                let db = rusqlite::Connection::open(dir.join("p4gui.db"))
                    .map_err(|e| format!("open(index): {e}"))?;
                index::init_conn(&db).map_err(|e| format!("init_conn(index): {e}"))?;
                index::init_schema(&db).map_err(|e| format!("init_schema: {e}"))?;
                let cache_db = rusqlite::Connection::open(dir.join("p4gui.db"))
                    .map_err(|e| format!("open(cache): {e}"))?;
                index::init_conn(&cache_db).map_err(|e| format!("init_conn(cache): {e}"))?;
                Ok((db, cache_db))
            };
            let (db, cache_db) = open_pair()
                .or_else(|e| {
                    eprintln!("cache DB open failed ({e}); retrying once");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    open_pair()
                })
                .unwrap_or_else(|e| {
                    eprintln!("cache DB unavailable ({e}); running on an in-memory cache");
                    let mk = || {
                        let c = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
                        let _ = index::init_schema(&c);
                        c
                    };
                    (mk(), mk())
                });
            app.manage(index::AppState::new(db, cache_db));
            // Put the main window back where it was (config has visible=false so
            // this never shows as a jump), then reveal it.
            if let Some(win) = app.get_webview_window("main") {
                wingeom::apply(&win, "main");
            }
            // Idle janitor: a TRUNCATE checkpoint every 10 minutes on its own
            // connection, so the WAL returns to zero whenever the app quiets
            // down. Best-effort — busy readers just defer it to the next tick.
            let janitor_db = dir.join("p4gui.db");
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(600));
                if let Ok(c) = rusqlite::Connection::open(&janitor_db) {
                    let _ = c.pragma_update(None, "busy_timeout", 2000);
                    index::checkpoint(&c);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::p4_info,
            commands::p4_clients,
            commands::p4_new_client,
            commands::swarm_reviews,
            commands::review_patch,
            commands::review_copy_files,
            commands::export_patch,
            commands::pick_patch_file,
            commands::preview_patch,
            commands::apply_patch,
            commands::merge_start_patch,
            commands::merge_start_resolve,
            commands::merge_data,
            commands::merge_save,
            commands::merge_cancel,
            commands::merge_external,
            commands::open_merge_window,
            commands::resolve_needed,
            commands::pick_folder,
            commands::p4_dirs,
            commands::p4_files,
            commands::p4_changes,
            commands::p4_pending,
            commands::p4_have_change,
            commands::p4_changes_exact,
            commands::p4_describe,
            commands::p4_filelog,
            commands::p4_fstat,
            commands::p4_sync,
            commands::p4_resync,
            commands::p4_reconcile,
            commands::p4_reconcile_files,
            commands::p4_clean,
            commands::p4_status,
            commands::p4_flush,
            commands::cancel_offline_scan,
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
            commands::p4_diff_local_forced,
            commands::open_diff_local,
            commands::set_clipboard,
            commands::detect_editors,
            commands::default_editor_id,
            commands::open_in_editor,
            commands::print_to_temp,
            commands::diff_pair_rev,
            commands::diff_pair_shelved,
            commands::diff_pair_local,
            commands::read_text_file,
            commands::open_diff_window,
            commands::write_local_file,
            commands::open_unreal_diff,
            commands::p4_submit,
            commands::p4_shelve,
            commands::p4_shelve_delete,
            commands::p4_request_review,
            commands::p4_set_description,
            commands::p4_revert,
            commands::p4_shelved_changes,
            commands::p4_delete_change,
            commands::p4_revert_change,
            commands::p4_undo_change,
            commands::p4_undo_preview,
            commands::p4_revert_keep,
            commands::p4_reopen,
            commands::p4_new_changelist,
            commands::swarm_url,
            commands::swarm_review,
            commands::p4_login_status,
            commands::p4_login,
            commands::p4_ticket_user,
            commands::p4_ticket_value,
            commands::p4_trust,
            commands::paths_exist,
            commands::p4_streams,
            commands::p4_depots,
            commands::p4_switch,
            commands::list_local_dir,
            commands::is_release_build,
            commands::p4_env_port,
            index::cache_get,
            index::cache_get_scope,
            index::cache_set,
            index::cache_clear,
            index::cache_del,
            index::index_status,
            index::index_build,
            index::index_build_depot,
            index::index_build_local,
            index::index_search,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
