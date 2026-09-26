//! What the window can ask for. The device token never leaves this process:
//! the window only gets results.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use notizli_core::api::{self, ApiError};
use notizli_core::pairing;
use notizli_core::store::{now_unix, Meta, Status};
use notizli_engine::{input_devices, InputDevice, Recorder, RecorderConfig, RecorderEvent};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::state::{Active, AppState, Awake, Labels, PendingPair};

type Res<T> = Result<T, String>;

#[derive(Serialize)]
pub struct StateView {
    version: String,
    os: &'static str,
    paired: bool,
    email: Option<String>,
    label: Option<String>,
    token_error: Option<String>,
    mic_device: Option<String>,
    recording: Option<RecordingView>,
    unsent: Vec<UnsentView>,
    upload: Option<UploadView>,
    pair_request: Option<PairRequestView>,
    last_done: Option<DoneView>,
    folder: String,
}

#[derive(Serialize, Clone)]
pub struct RecordingView {
    id: String,
    title: String,
    elapsed_ms: u64,
    layout: &'static str,
    labels: Labels,
}

#[derive(Serialize)]
pub struct UnsentView {
    id: String,
    title: String,
    started_at: String,
    duration_ms: u64,
    size: u64,
    status: Status,
    last_error: Option<String>,
    recovered: bool,
}

#[derive(Serialize)]
pub struct UploadView {
    id: String,
    sent: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
pub struct PairRequestView {
    email: String,
    current_email: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct DoneView {
    id: String,
    meeting_id: String,
    meeting_url: String,
    title: String,
}

fn recording_view(a: &Active) -> RecordingView {
    RecordingView {
        id: a.meta.id.clone(),
        title: a.meta.title.clone(),
        elapsed_ms: a.started.elapsed().as_millis() as u64,
        layout: a.recorder.layout().as_str(),
        labels: a.labels.lock().unwrap().clone(),
    }
}

#[tauri::command]
pub fn get_state(app: AppHandle, state: State<'_, AppState>) -> StateView {
    let settings = state.settings.lock().unwrap().clone();
    let recording = state.recording.lock().unwrap().as_ref().map(recording_view);
    let current_upload = state.upload.current.lock().unwrap().clone();
    StateView {
        version: app.package_info().version.to_string(),
        os: std::env::consts::OS,
        paired: state.token().is_some(),
        email: settings.paired_email.clone(),
        label: settings.device_label.clone(),
        token_error: state.token_error.lock().unwrap().clone(),
        mic_device: settings.mic_device.clone(),
        recording,
        unsent: state
            .store
            .list()
            .into_iter()
            .filter(|m| m.status != Status::Recording)
            .map(|m| UnsentView {
                id: m.id,
                title: m.title,
                started_at: m.started_at,
                duration_ms: m.duration_ms,
                size: m.size,
                status: m.status,
                last_error: m.last_error,
                recovered: m.recovered,
            })
            .collect(),
        upload: current_upload.map(|id| UploadView {
            id,
            sent: state.upload.sent.load(Ordering::Relaxed),
            total: state.upload.total.load(Ordering::Relaxed),
        }),
        pair_request: state.pair.lock().unwrap().as_ref().map(|p| PairRequestView {
            email: p.preview.email.clone(),
            current_email: settings.paired_email.clone().filter(|_| state.token().is_some()),
        }),
        last_done: state.upload.last_done.lock().unwrap().as_ref().map(|(id, meeting_id, title)| DoneView {
            id: id.clone(),
            meeting_id: meeting_id.clone(),
            meeting_url: api::meeting_url(meeting_id),
            title: title.clone(),
        }),
        folder: state.store.folder().display().to_string(),
    }
}

pub fn notice(app: &AppHandle, text: impl Into<String>) {
    let _ = app.emit("notice", serde_json::json!({ "text": text.into() }));
}

pub fn changed(app: &AppHandle) {
    let _ = app.emit("state", ());
}

fn focus(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ---- microphone -------------------------------------------------------------

#[tauri::command]
pub async fn list_mics() -> Res<Vec<InputDevice>> {
    tauri::async_runtime::spawn_blocking(|| input_devices().map_err(|e| e.to_string())).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn set_mic(state: State<'_, AppState>, device: Option<String>) {
    state.update_settings(|s| s.mic_device = device.filter(|d| !d.is_empty()));
}

// ---- links --------------------------------------------------------------------

#[tauri::command]
pub fn open_pair_page(app: AppHandle) -> Res<()> {
    app.opener().open_url(api::pair_page_url(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_meeting(app: AppHandle, meeting_id: String) -> Res<()> {
    if meeting_id.is_empty() || !meeting_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("invalid meeting".into());
    }
    app.opener().open_url(api::meeting_url(&meeting_id), None::<&str>).map_err(|e| e.to_string())
}

// ---- pairing ------------------------------------------------------------------

/// A notizli-sh:// link arrived (app started by it, or a second launch).
pub fn handle_link(app: &AppHandle, link: &str) {
    focus(app);
    let Some(token) = pairing::token_from_link(link) else {
        log::warn!("ignored a link that is not a pairing link");
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = request_pairing(&app, token).await {
            notice(&app, e);
        }
    });
}

async fn request_pairing(app: &AppHandle, token: String) -> Res<PairRequestView> {
    let state = app.state::<AppState>();
    if state.is_recording() {
        return Err("Finish the recording before pairing this recorder.".into());
    }
    let preview = state.api.pair_preview(&token).await.map_err(|e| e.to_string())?;
    let view = PairRequestView {
        email: preview.email.clone(),
        current_email: state.settings.lock().unwrap().paired_email.clone().filter(|_| state.token().is_some()),
    };
    *state.pair.lock().unwrap() = Some(PendingPair { token, preview });
    let _ = app.emit("pair-request", view.clone());
    Ok(view)
}

/// The user pasted a pairing token (or link).
#[tauri::command]
pub async fn submit_pair_text(app: AppHandle, text: String) -> Res<PairRequestView> {
    let token = pairing::token_from_paste(&text).ok_or("That doesn't look like a pairing token. It starts with ccp_pair_.")?;
    request_pairing(&app, token).await
}

#[tauri::command]
pub fn cancel_pair(state: State<'_, AppState>) {
    *state.pair.lock().unwrap() = None;
}

/// The user confirmed "Pair this recorder with …?".
#[tauri::command]
pub async fn confirm_pair(app: AppHandle) -> Res<String> {
    let state = app.state::<AppState>();
    if state.is_recording() {
        return Err("Finish the recording before pairing this recorder.".into());
    }
    let pending = state.pair.lock().unwrap().take().ok_or("Start pairing again from notizli.ch/pair.")?;
    let label = pairing::device_label();
    let paired = state.api.pair(&pending.token, &label).await.map_err(|e| e.to_string())?;
    state.set_token(Some(paired.device_token.clone())).map_err(|e| format!("Couldn't store the pairing securely: {e}"))?;
    state.update_settings(|s| {
        s.paired_email = Some(pending.preview.email.clone());
        s.device_label = paired.label.clone().or(Some(label.clone()));
        s.upload_url = paired.upload_url.clone();
    });
    changed(&app);
    state.upload.kick.notify_one();
    Ok(pending.preview.email)
}

#[tauri::command]
pub fn unpair(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    state.set_token(None)?;
    state.update_settings(|s| {
        s.paired_email = None;
        s.device_label = None;
        s.upload_url = None;
    });
    changed(&app);
    Ok(())
}

// ---- recording ----------------------------------------------------------------

fn default_title() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let f = time::macros::format_description!("[day] [month repr:short] [year], [hour]:[minute]");
    format!("Desktop recording — {}", now.format(&f).unwrap_or_default())
}

#[tauri::command]
pub async fn start_recording(app: AppHandle, title: Option<String>) -> Res<RecordingView> {
    let state = app.state::<AppState>();
    if state.is_recording() {
        return Err("Already recording.".into());
    }
    let title = title.map(|t| t.trim().chars().take(200).collect::<String>()).filter(|t| !t.is_empty()).unwrap_or_else(default_title);
    let mut meta = state.store.create(&title, time::OffsetDateTime::now_utc()).map_err(|e| format!("Can't save recordings: {e}"))?;
    let labels = Arc::new(Mutex::new(Labels::default()));
    let (ev_app, ev_labels) = (app.clone(), labels.clone());
    let events = Arc::new(move |e: RecorderEvent| {
        match &e {
            RecorderEvent::Hearing { label } => ev_labels.lock().unwrap().hearing = label.clone(),
            RecorderEvent::Microphone { label } => ev_labels.lock().unwrap().microphone = label.clone(),
            RecorderEvent::OtherSideSilent { silent, .. } => ev_labels.lock().unwrap().warning = *silent,
            _ => {}
        }
        let _ = ev_app.emit("rec", &e);
    });
    let cfg = RecorderConfig {
        path: state.store.audio_path(&meta.id),
        mic_device: state.settings.lock().unwrap().mic_device.clone(),
        capture_other_side: true,
        writing_app: format!("Notizli {}", app.package_info().version),
    };
    let started = tauri::async_runtime::spawn_blocking(move || Recorder::start(cfg, events)).await.map_err(|e| e.to_string())?;
    let recorder = match started {
        Ok(r) => r,
        Err(e) => {
            let _ = state.store.discard(&meta.id);
            return Err(e.to_string());
        }
    };
    meta.channel_layout = recorder.layout().as_str().to_string();
    let _ = state.store.save(&meta);
    let active = Active { recorder, meta, started: Instant::now(), labels, _awake: Awake::start() };
    let view = recording_view(&active);
    *state.recording.lock().unwrap() = Some(active);
    changed(&app);
    Ok(view)
}

#[derive(Serialize)]
pub struct FinishView {
    id: String,
    title: String,
    paired: bool,
    saved_elsewhere: Option<String>,
}

/// Stop, save to disk, and hand the recording to the uploader.
#[tauri::command]
pub async fn finish_recording(app: AppHandle) -> Res<FinishView> {
    let state = app.state::<AppState>();
    let active = state.recording.lock().unwrap().take().ok_or("Not recording.")?;
    let Active { recorder, mut meta, .. } = active;
    let result = tauri::async_runtime::spawn_blocking(move || recorder.stop()).await.map_err(|e| e.to_string())?;
    let rec = result.map_err(|e| format!("The recording could not be finished: {e}"))?;
    meta.channel_layout = rec.layout.as_str().to_string();
    meta.duration_ms = rec.duration_ms;
    meta.size = rec.size;
    meta.status = Status::Ready;
    let _ = std::fs::rename(&rec.health_path, state.store.health_path(&meta.id));

    let mut saved_elsewhere = None;
    if let Some(bytes) = rec.unsaved {
        // The disk failed during the meeting: the whole file is in memory.
        let target = state.store.audio_path(&meta.id);
        if std::fs::write(&target, &bytes).is_err() {
            let dir = app.path().download_dir().or_else(|_| app.path().desktop_dir()).unwrap_or_else(|_| state.data_dir.clone());
            let fallback = dir.join(format!("Notizli recording {}.webm", meta.id));
            std::fs::write(&fallback, &bytes).map_err(|e| format!("The recording could not be saved anywhere: {e}"))?;
            saved_elsewhere = Some(fallback.display().to_string());
            let _ = state.store.discard(&meta.id);
        }
    }
    if saved_elsewhere.is_none() {
        state.store.save(&meta).map_err(|e| format!("Couldn't save the recording's details: {e}"))?;
        state.upload.kick.notify_one();
    }
    changed(&app);
    Ok(FinishView { id: meta.id, title: meta.title, paired: state.token().is_some(), saved_elsewhere })
}

#[tauri::command]
pub async fn discard_recording(app: AppHandle) -> Res<()> {
    let state = app.state::<AppState>();
    let active = state.recording.lock().unwrap().take().ok_or("Not recording.")?;
    let Active { recorder, meta, .. } = active;
    let _ = tauri::async_runtime::spawn_blocking(move || recorder.discard()).await;
    let _ = state.store.discard(&meta.id);
    changed(&app);
    Ok(())
}

/// Quit, saving a recording in progress first (it uploads on next start).
#[tauri::command]
pub async fn quit_app(app: AppHandle) -> Res<()> {
    if app.state::<AppState>().is_recording() {
        finish_recording(app.clone()).await?;
    }
    app.exit(0);
    Ok(())
}

// ---- unsent recordings --------------------------------------------------------

/// Try now instead of waiting for the next automatic retry.
#[tauri::command]
pub fn upload_now(state: State<'_, AppState>, id: Option<String>) {
    for mut m in state.store.list() {
        if m.status == Status::Ready && id.as_deref().is_none_or(|i| i == m.id) {
            m.retry_after = 0;
            let _ = state.store.save(&m);
        }
    }
    state.upload.kick.notify_one();
}

#[tauri::command]
pub fn show_in_folder(app: AppHandle, state: State<'_, AppState>, id: Option<String>) -> Res<()> {
    let target = match id {
        Some(id) => state.store.audio_path(&id),
        None => state.store.folder().to_path_buf(),
    };
    if target.is_file() {
        app.opener().reveal_item_in_dir(&target).map_err(|e| e.to_string())
    } else {
        app.opener().open_path(target.display().to_string(), None::<&str>).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn save_copy(app: AppHandle, id: String) -> Res<Option<String>> {
    let state = app.state::<AppState>();
    let meta: Meta = state.store.get(&id).ok_or("That recording no longer exists.")?;
    let source = state.store.audio_path(&id);
    let name: String = meta.title.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '-' } else { c }).collect();
    let dialog = app.dialog().file().set_file_name(format!("{name}.webm")).add_filter("WebM audio", &["webm"]);
    let chosen = tauri::async_runtime::spawn_blocking(move || dialog.blocking_save_file()).await.map_err(|e| e.to_string())?;
    let Some(path) = chosen.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::copy(&source, &path).map_err(|e| format!("Couldn't save the copy: {e}"))?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub fn discard_unsent(app: AppHandle, state: State<'_, AppState>, id: String) -> Res<()> {
    if state.upload.current.lock().unwrap().as_deref() == Some(id.as_str()) {
        return Err("This recording is uploading right now.".into());
    }
    state.store.discard(&id).map_err(|e| e.to_string())?;
    changed(&app);
    Ok(())
}

/// Message for a failed upload, and whether it is final.
pub fn upload_error_view(e: &ApiError) -> (&'static str, String) {
    let kind = match e {
        ApiError::Unauthorized => "unauthorized",
        ApiError::TooLarge => "too_large",
        e if e.retryable() => "retry",
        _ => "rejected",
    };
    (kind, e.to_string())
}

pub fn retry_later(meta: &mut Meta, error: &str) {
    meta.attempts += 1;
    meta.last_error = Some(error.to_string());
    meta.retry_after = now_unix() + notizli_core::store::backoff_secs(meta.attempts);
}
