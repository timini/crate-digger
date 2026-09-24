mod central;
mod commands;
mod models;
mod probe;
mod secrets;
mod soulseek;
mod state;
mod tray;
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
            let default_archive = app
                .path()
                .audio_dir()
                .map(|d| d.join("Crate Digger"))
                .unwrap_or_else(|_| data_dir.join("archive"));
            let state = state::AppState::open(&data_dir, default_archive)?;
            state.recover_archive();
            state.apply_limits();
            if let Ok(conn) = state.db() {
                use cd_core::analysis::handler::Analyzer;
                if let Err(e) = workers::plan_analysis(&conn, &state.analyzer.version()) {
                    tracing::warn!("could not plan analysis: {e}");
                }
            }
            if let Ok(conn) = state.db() {
                // Tracks analysed before metadata lookup existed.
                if let Err(e) = cd_core::metadata_lookup::queue_all(&conn, cd_core::util::now_ms()) {
                    tracing::warn!("could not queue metadata lookups: {e}");
                }
            }
            let handlers = workers::handlers(&state);
            state.start_workers(handlers)?;
            app.manage(state);
            tray::install(app.handle())?;
            workers::spawn_refresh(app.handle().clone());
            {
                // Signing in can take half a minute; do not hold up the window.
                let soulseek = app.state::<state::AppState>().soulseek.clone();
                std::thread::spawn(move || soulseek.start_if_ready());
            }

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
            commands::connections::connections_get,
            commands::connections::connections_save,
            commands::connections::credential_set,
            commands::connections::connection_test,
            commands::connections::discovery_seeds,
            commands::connections::discovery_seeds_save,
            commands::connections::discovery_page,
            commands::connections::discovery_paste,
            commands::connections::discovery_runs,
            commands::connections::youtube_set,
            commands::connections::youtube_prefer,
            commands::connections::youtube_reject,
            commands::connections::youtube_refresh,
            commands::central::central_status,
            commands::central::central_sign_in,
            commands::central::central_sign_out,
            commands::central::sharing_set,
            commands::central::backup_now,
            commands::central::backups_list,
            commands::central::backup_restore,
            commands::central::backup_delete,
            commands::connections::soulseek_status,
            commands::connections::metadata_status,
            commands::connections::metadata_identify_all,
            commands::connections::metadata_identify,
            commands::connections::metadata_suggestions,
            commands::connections::metadata_accept,
            commands::connections::metadata_dismiss,
            commands::connections::soulseek_setup,
            commands::connections::download_choices,
            commands::connections::download_choose,
            commands::connections::download_decline,
            commands::connections::unattended_downloads_get,
            commands::connections::unattended_downloads_set,
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
            commands::playlists::playlist_export_check,
            commands::playlists::playlist_export,
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
            commands::review::storage_status,
            commands::review::staging_clear,
            commands::settings::settings_get,
            commands::settings::settings_set_archive_dir,
            commands::settings::settings_set_staging_dir,
            commands::settings::settings_set_limits,
            commands::settings::settings_set_close_to_tray,
            commands::settings::onboarding_complete,
            commands::identity::identity_conflicts,
            commands::identity::identity_conflict_count,
            commands::identity::identity_resolve,
            commands::analysis::models_list,
            commands::analysis::model_download,
            commands::analysis::model_download_progress,
            commands::analysis::model_choose,
            commands::review::demo_discovery_get,
            commands::review::queue_health,
            commands::review::library_rate,
            commands::review::demo_discovery_set,
        ])
        .on_window_event(|window, event| {
            // Closing the window hides it; work continues in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.app_handle().state::<state::AppState>().close_to_tray() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Crate Digger")
        .run(|app, event| match event {
            tauri::RunEvent::Exit => app.state::<state::AppState>().shutdown(),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => tray::show_main_window(app),
            _ => {}
        });
}
