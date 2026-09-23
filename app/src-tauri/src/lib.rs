mod commands;
mod probe;
mod state;
mod workers;

use tauri::Manager;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // CRATE_DIGGER_DATA_DIR overrides the data location (tests, smoke runs).
            let data_dir = match std::env::var_os("CRATE_DIGGER_DATA_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_data_dir()?,
            };
            let state = state::AppState::open(&data_dir)?;
            let handlers = workers::handlers(&state);
            state.start_workers(handlers)?;
            app.manage(state);

            // CRATE_DIGGER_SMOKE=1: prove the app starts, then exit cleanly.
            if std::env::var_os("CRATE_DIGGER_SMOKE").is_some() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    println!("crate-digger smoke: started");
                    handle.exit(0);
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::jobs::activity,
            commands::jobs::jobs_pause_all,
            commands::jobs::jobs_resume_all,
            commands::jobs::job_cancel,
            commands::jobs::job_retry,
            commands::library::library_roots,
            commands::library::library_add_root,
            commands::library::library_remove_root,
            commands::library::library_rescan,
            commands::library::library_search,
            commands::library::track_detail,
            commands::library::track_set_field,
            commands::library::file_set_primary,
            commands::library::relink_find,
            commands::library::relink_apply,
            commands::library::library_check_files,
            commands::library::duplicates_list,
            commands::library::duplicates_merge,
            commands::library::duplicates_dismiss,
            commands::player::player_play_track,
            commands::player::player_toggle,
            commands::player::player_pause,
            commands::player::player_seek,
            commands::player::player_set_volume,
            commands::player::player_status,
            commands::player::track_waveform,
            commands::playlists::playlists_list,
            commands::playlists::playlist_create,
            commands::playlists::playlist_rename,
            commands::playlists::playlist_delete,
            commands::playlists::playlist_entries,
            commands::playlists::playlist_add,
            commands::playlists::playlist_remove,
            commands::playlists::playlist_move,
            commands::review::review_next,
            commands::review::review_stats,
            commands::review::review_rate,
            commands::review::review_skip,
            commands::review::review_undo,
            commands::review::review_keep,
            commands::review::review_find_more,
            commands::review::demo_discovery_get,
            commands::review::demo_discovery_set,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Crate Digger")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<state::AppState>().shutdown();
            }
        });
}
