//! System tray: closing the window keeps background work running; Quit
//! stops it cleanly.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Crate Digger", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause background work", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "Resume background work", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Crate Digger", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &pause, &resume, &sep, &quit])?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Crate Digger")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "pause" => {
                if let Ok(conn) = app.state::<AppState>().db() {
                    let _ = cd_core::jobs::pause_all(&conn, cd_core::util::now_ms());
                }
            }
            "resume" => {
                let state = app.state::<AppState>();
                if let Ok(conn) = state.db() {
                    let _ = cd_core::jobs::resume_all(&conn, cd_core::util::now_ms());
                }
                state.notify_workers();
            }
            // Explicit Quit: RunEvent::Exit parks running jobs and stops workers.
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}
