//! The menu bar (macOS) / notification area (Windows) icon: start or finish
//! a recording, open the window, quit.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::commands;
use crate::state::AppState;

const ID: &str = "main";

pub struct Tray {
    record: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let record = MenuItem::with_id(app, "record", "Start recording", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Notizli", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Notizli", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&record, &open, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id(ID).menu(&menu).tooltip("Notizli").on_menu_event(|app, event| match event.id().as_ref() {
        "record" => toggle_recording(app),
        "open" => commands::focus(app),
        "quit" => quit_app(app),
        _ => {}
    });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(Tray { record });
    Ok(())
}

/// Keep the menu in step with the recording.
pub fn refresh(app: &AppHandle) {
    let recording = app.state::<AppState>().is_recording();
    if let Some(t) = app.try_state::<Tray>() {
        let _ = t.record.set_text(if recording { "Finish recording" } else { "Start recording" });
    }
    if let Some(tray) = app.tray_by_id(ID) {
        let _ = tray.set_tooltip(Some(if recording { "Notizli — recording" } else { "Notizli" }));
    }
}

fn toggle_recording(app: &AppHandle) {
    if app.state::<AppState>().is_recording() {
        commands::finish_in_background(app);
        return;
    }
    commands::focus(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = commands::start_recording(app.clone(), None).await {
            commands::notice(&app, e);
        }
    });
}

fn quit_app(app: &AppHandle) {
    if app.state::<AppState>().is_recording() {
        commands::focus(app);
        let _ = app.emit("confirm-quit", ());
    } else {
        app.exit(0);
    }
}
