//! Notizli desktop recorder.
//!
//! The Rust side does all the work (recording, saving, pairing, uploading);
//! the window (plain HTML in ../ui) shows state and sends button presses.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod logfile;
mod state;
mod uploader;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;

use state::AppState;

fn main() {
    let app = tauri::Builder::default()
        // Must be first: a second launch (e.g. a notizli-sh:// link on
        // Windows) hands its link to this instance instead of opening a window.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            logfile::init(&data_dir.join("logs"));
            log::info!("Notizli {} starting on {}", app.package_info().version, std::env::consts::OS);
            let state = AppState::new(&data_dir)?;
            let recovered = state.store.recover();
            app.manage(state);
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
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<AppState>().is_recording() {
                    api.prevent_close();
                    let _ = window.emit("confirm-quit", ());
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
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Notizli");

    app.run(|app, event| {
        // Cmd+Q / quitting from the dock while recording: ask first.
        if let RunEvent::ExitRequested { api, code, .. } = &event {
            if code.is_none() && app.state::<AppState>().is_recording() {
                api.prevent_exit();
                let _ = app.emit("confirm-quit", ());
            }
        }
    });
}
