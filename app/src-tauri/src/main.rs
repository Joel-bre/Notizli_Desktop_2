//! Notizli desktop recorder.
//!
//! The Rust side does all the work (recording, saving, pairing, uploading);
//! the window (plain HTML in ../ui) shows state and sends button presses.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod detect;
mod logfile;
mod prompt;
mod state;
mod tray;
mod uploader;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;

use state::AppState;

fn main() {
    let app = tauri::Builder::default()
        // Must be first: a second launch (e.g. a notizli-sh:// link on
        // Windows) hands its link to this instance instead of opening a window.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| commands::focus(app)))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .manage(prompt::PromptState::default())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            logfile::init(&data_dir.join("logs"));
            log::info!("Notizli {} starting on {}", app.package_info().version, std::env::consts::OS);
            let state = AppState::new(&data_dir)?;
            let recovered = state.store.recover();
            app.manage(state);
            // First start of a release build: open at login, as agreed.
            // Never for development builds (it would register target/debug).
            let undecided = app.state::<AppState>().settings.lock().unwrap().open_at_login.is_none();
            if undecided && !cfg!(debug_assertions) {
                if let Err(e) = commands::apply_open_at_login(app.handle(), true) {
                    log::warn!("{e}");
                }
            }
            for r in &recovered {
                log::info!("recovered interrupted recording {} ({} ms)", r.id, r.duration_ms);
            }

            // Installers register the scheme; this also covers a copied app.
            #[cfg(windows)]
            if let Err(e) = app.deep_link().register_all() {
                log::warn!("registering notizli-sh://: {e}");
            }
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    commands::handle_link(&handle, url.as_str());
                }
            });
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for url in urls {
                    commands::handle_link(app.handle(), url.as_str());
                }
            }

            uploader::spawn(app.handle().clone());
            detect::spawn(app.handle().clone());
            tray::create(app.handle())?;
            // Started at login: stay in the menu bar / notification area.
            if std::env::args().any(|a| a == "--hidden") {
                commands::set_dock(app.handle(), false);
            } else {
                commands::focus(app.handle());
            }
            Ok(())
        })
        // Closing the window hides it; Notizli keeps running (and recording)
        // in the menu bar / notification area.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    commands::set_dock(window.app_handle(), false);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::list_mics,
            commands::set_mic,
            commands::open_pair_page,
            commands::open_meeting,
            commands::submit_pair_text,
            commands::cancel_pair,
            commands::confirm_pair,
            commands::unpair,
            commands::start_recording,
            commands::finish_recording,
            commands::discard_recording,
            commands::quit_app,
            commands::upload_now,
            commands::show_in_folder,
            commands::save_copy,
            commands::discard_unsent,
            commands::open_diagnostics,
            commands::set_ask_on_calls,
            commands::set_open_at_login,
            commands::prompt_view,
            commands::prompt_record,
            commands::prompt_dismiss,
            commands::prompt_keep,
            commands::prompt_finish_now,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Notizli");

    app.run(|app, event| {
        // macOS: clicking the Dock icon brings the window back.
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = &event {
            commands::focus(app);
        }
        // Cmd+Q / quitting from the dock while recording: ask first.
        if let RunEvent::ExitRequested { api, code, .. } = &event {
            if code.is_none() && app.state::<AppState>().is_recording() {
                api.prevent_exit();
                let _ = app.emit("confirm-quit", ());
            }
        }
    });
}
