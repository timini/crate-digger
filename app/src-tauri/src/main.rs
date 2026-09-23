// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The app starts copies of itself as isolated analysis workers.
    if std::env::args().nth(1).as_deref() == Some(cd_analyzer::WORKER_FLAG) {
        cd_analyzer::worker_main();
        return;
    }
    crate_digger_lib::run();
}
