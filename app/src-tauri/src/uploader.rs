//! Uploads waiting recordings: at start, after each recording, every minute
//! (for the ones whose back-off has passed), and on "Upload now".
//! A file is deleted only after the server answered 202.

use std::sync::atomic::Ordering;
use std::time::Duration;

use notizli_core::api::{self, ApiError, UploadMeta};
use notizli_core::store::{now_unix, Status};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{changed, retry_later, upload_error_view};
use crate::state::AppState;

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            upload_due(&app).await;
            let state = app.state::<AppState>();
            tokio::select! {
                _ = state.upload.kick.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    });
}

async fn upload_due(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(token) = state.token() else { return };
    let recording = state.recording_id();
    for meta in state.store.list() {
        if meta.status != Status::Ready || meta.retry_after > now_unix() || Some(&meta.id) == recording.as_ref() {
            continue;
        }
        if !upload_one(app, &token, meta.id.clone()).await {
            break;
        }
    }
}

/// Returns false when uploading should stop for now (token revoked).
async fn upload_one(app: &AppHandle, token: &str, id: String) -> bool {
    let state = app.state::<AppState>();
    let Some(mut meta) = state.store.get(&id) else { return true };
    let path = state.store.audio_path(&id);
    let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(meta.size);
    *state.upload.current.lock().unwrap() = Some(id.clone());
    state.upload.total.store(total, Ordering::Relaxed);
    state.upload.sent.store(0, Ordering::Relaxed);
    let _ = app.emit("upload", serde_json::json!({ "id": id, "phase": "start", "sent": 0, "total": total, "title": meta.title }));

    // Progress for the window while the upload runs.
    let ticker = {
        let app = app.clone();
        let id = id.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let state = app.state::<AppState>();
                let sent = state.upload.sent.load(Ordering::Relaxed);
                let _ = app.emit("upload", serde_json::json!({ "id": id, "phase": "progress", "sent": sent, "total": total }));
            }
        })
    };

    let upload_url = state.settings.lock().unwrap().upload_url.clone();
    let fields = UploadMeta { title: meta.title.clone(), started_at: meta.started_at.clone(), channel_layout: meta.channel_layout.clone() };
    let result = state.api.upload(upload_url.as_deref(), token, &path, &meta.mime, &fields, state.upload.sent.clone()).await;
    ticker.abort();
    *state.upload.current.lock().unwrap() = None;

    let keep_going = match result {
        Ok(done) => {
            log::info!("uploaded {} as meeting {}", id, done.meeting_id);
            if let Err(e) = state.store.accepted(&id) {
                log::warn!("uploaded, but could not delete the local copy: {e}");
            }
            *state.upload.last_done.lock().unwrap() = Some((id.clone(), done.meeting_id.clone(), meta.title.clone()));
            let _ = app.emit(
                "upload",
                serde_json::json!({ "id": id, "phase": "done", "meeting_id": done.meeting_id, "meeting_url": api::meeting_url(&done.meeting_id), "title": meta.title }),
            );
            true
        }
        Err(e) => {
            let (kind, message) = upload_error_view(&e);
            log::warn!("upload of {id} failed ({kind}): {e:?}");
            match e {
                ApiError::Unauthorized => {
                    meta.last_error = Some(message.clone());
                    let _ = state.store.save(&meta);
                    // The token was revoked: forget it; the recording stays.
                    let _ = state.set_token(None);
                    state.update_settings(|s| s.paired_email = None);
                }
                ApiError::TooLarge => {
                    meta.status = Status::TooLarge;
                    meta.last_error = Some(message.clone());
                    let _ = state.store.save(&meta);
                }
                _ => {
                    retry_later(&mut meta, &message);
                    let _ = state.store.save(&meta);
                }
            }
            let _ = app.emit("upload", serde_json::json!({ "id": id, "phase": "failed", "kind": kind, "error": message, "title": meta.title }));
            !matches!(kind, "unauthorized")
        }
    };
    changed(app);
    keep_going
}
