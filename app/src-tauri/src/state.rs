//! Everything the app holds while running.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use notizli_core::account::{self, Settings, SettingsFile};
use notizli_core::api::{Api, PairPreview};
use notizli_core::store::{Meta, Store};
use notizli_engine::Recorder;
use serde::Serialize;

pub struct AppState {
    pub data_dir: PathBuf,
    pub store: Store,
    pub settings_file: SettingsFile,
    pub settings: Mutex<Settings>,
    pub api: Api,
    /// Device token, read from the credential store once.
    token: Mutex<Option<String>>,
    pub token_error: Mutex<Option<String>>,
    pub recording: Mutex<Option<Active>>,
    pub pair: Mutex<Option<PendingPair>>,
    pub upload: UploadState,
}

pub struct Active {
    pub recorder: Recorder,
    pub meta: Meta,
    pub started: Instant,
    pub labels: Arc<Mutex<Labels>>,
    pub _awake: Awake,
}

#[derive(Default, Clone, Serialize)]
pub struct Labels {
    pub hearing: String,
    pub microphone: String,
    pub warning: bool,
}

pub struct PendingPair {
    pub token: String,
    pub preview: PairPreview,
}

#[derive(Default)]
pub struct UploadState {
    /// Id of the recording being uploaded.
    pub current: Mutex<Option<String>>,
    pub sent: Arc<AtomicU64>,
    pub total: AtomicU64,
    /// Wakes the uploader (after a recording, or "Upload now").
    pub kick: tokio::sync::Notify,
    /// The last recording the server accepted: (recording id, meeting id, title).
    pub last_done: Mutex<Option<(String, String, String)>>,
}

impl AppState {
    pub fn new(data_dir: &Path) -> std::io::Result<AppState> {
        std::fs::create_dir_all(data_dir)?;
        let store = Store::open(data_dir)?;
        let settings_file = SettingsFile::new(data_dir);
        let settings = settings_file.load();
        let (token, token_error) = match account::load_token() {
            Ok(t) => (t, None),
            Err(e) => (None, Some(e)),
        };
        Ok(AppState {
            data_dir: data_dir.to_path_buf(),
            store,
            settings_file,
            settings: Mutex::new(settings),
            api: Api::default(),
            token: Mutex::new(token),
            token_error: Mutex::new(token_error),
            recording: Mutex::new(None),
            pair: Mutex::new(None),
            upload: UploadState::default(),
        })
    }

    pub fn is_recording(&self) -> bool {
        self.recording.lock().unwrap().is_some()
    }

    pub fn recording_id(&self) -> Option<String> {
        self.recording.lock().unwrap().as_ref().map(|a| a.meta.id.clone())
    }

    pub fn token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    pub fn set_token(&self, token: Option<String>) -> Result<(), String> {
        match &token {
            Some(t) => account::save_token(t)?,
            None => account::delete_token()?,
        }
        *self.token.lock().unwrap() = token;
        *self.token_error.lock().unwrap() = None;
        Ok(())
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = self.settings.lock().unwrap();
        f(&mut s);
        if let Err(e) = self.settings_file.save(&s) {
            log::warn!("saving settings: {e}");
        }
    }
}

/// Keeps the computer from sleeping until dropped. Held on its own thread,
/// because Windows ties the request to the thread that made it.
pub struct Awake {
    _stop: mpsc::Sender<()>,
}

impl Awake {
    pub fn start() -> Awake {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let guard = keepawake::Builder::default()
                .idle(true)
                .sleep(true)
                .reason("Recording a meeting")
                .app_name("Notizli")
                .app_reverse_domain("ch.notizli.recorder")
                .create();
            if let Err(e) = &guard {
                log::warn!("could not keep the computer awake: {e}");
            }
            // Returns when the Awake (the sender) is dropped.
            let _ = rx.recv();
            drop(guard);
        });
        Awake { _stop: tx }
    }
}
