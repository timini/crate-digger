mod commands;
mod state;

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
        .invoke_handler(tauri::generate_handler![commands::app_info])
        .run(tauri::generate_context!())
        .expect("error while running Crate Digger");
}
