//! Automatic updates from the newest published GitHub release: checked at
//! start and every 6 hours, downloaded in the background, installed when the
//! user clicks "Restart to update" or right away at a start at login. Never
//! while recording.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::commands;
use crate::state::AppState;

const EVERY: Duration = Duration::from_secs(6 * 3600);
/// After a failed check (e.g. no network yet right after login).
const RETRY: Duration = Duration::from_secs(5 * 60);
/// A start at login installs an update found within this time.
const AT_LOGIN: Duration = Duration::from_secs(15 * 60);

/// A downloaded update, waiting for a calm moment.
#[derive(Default)]
pub struct Pending(Mutex<Option<(Update, Vec<u8>)>>);

/// The version ready to install, if any.
pub fn ready(app: &AppHandle) -> Option<String> {
    app.state::<Pending>().0.lock().unwrap().as_ref().map(|(u, _)| u.version.clone())
}

pub fn spawn(app: AppHandle, at_login: bool) {
    let started = Instant::now();
    tauri::async_runtime::spawn(async move {
        loop {
            let mut wait = EVERY;
            if ready(&app).is_none() && !app.state::<AppState>().is_recording() {
                match fetch(&app).await {
                    Ok(Some(version)) => {
                        log::info!("update {version} downloaded");
                        if at_login && started.elapsed() < AT_LOGIN {
                            if let Err(e) = install(&app) {
                                log::warn!("{e}");
                            }
                        }
                        commands::changed(&app);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!("checking for updates: {e}");
                        wait = RETRY;
                    }
                }
            }
            tokio::time::sleep(wait).await;
        }
    });
}

async fn fetch(app: &AppHandle) -> Result<Option<String>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else { return Ok(None) };
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    let version = update.version.clone();
    *app.state::<Pending>().0.lock().unwrap() = Some((update, bytes));
    Ok(Some(version))
}

/// Install the downloaded update and restart (Windows: the installer
/// restarts Notizli). Refused while recording.
pub fn install(app: &AppHandle) -> Result<(), String> {
    if app.state::<AppState>().is_recording() {
        return Err("Finish the recording before updating.".into());
    }
    let Some((update, bytes)) = app.state::<Pending>().0.lock().unwrap().take() else {
        return Err("No update is ready.".into());
    };
    log::info!("installing update {}", update.version);
    update.install(&bytes).map_err(|e| format!("The update could not be installed: {e}"))?;
    app.restart();
}
