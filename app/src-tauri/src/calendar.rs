//! The user's upcoming meetings from notizli.ch, in memory only: they name
//! the meeting in the "Record it?" box and tell the server which meeting a
//! recording is for. Meeting titles never go to the log.
//! Design: docs/superpowers/specs/2026-10-14-calendar-design.md.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use notizli_core::calendar::{Meeting, Upcoming};
use tauri::{AppHandle, Manager};
use time::UtcOffset;

use crate::state::AppState;

/// A list younger than this is used as is (notizli.ch caches 2 minutes too).
pub const FRESH: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct CalendarState {
    latest: Mutex<Option<(Instant, Upcoming)>>,
}

impl CalendarState {
    pub fn cached(&self) -> Option<Upcoming> {
        self.latest.lock().unwrap().as_ref().map(|(_, u)| u.clone())
    }

    pub fn forget(&self) {
        *self.latest.lock().unwrap() = None;
    }

    fn keep(&self, u: &Upcoming) {
        *self.latest.lock().unwrap() = Some((Instant::now(), u.clone()));
    }
}

/// The meetings, fetched again when the list is older than `max_age`. None
/// when unpaired or notizli.ch didn't answer: the app then behaves as before.
pub async fn upcoming(app: &AppHandle, max_age: Duration) -> Option<Upcoming> {
    let state = app.state::<AppState>();
    if let Some(u) = fake() {
        state.calendar.keep(&u);
        return Some(u);
    }
    if let Some((at, u)) = state.calendar.latest.lock().unwrap().as_ref() {
        if at.elapsed() < max_age {
            return Some(u.clone());
        }
    }
    let token = state.token()?;
    match state.api.upcoming_meetings(&token).await {
        Ok(u) => {
            let on_now = if u.now_meeting().is_some() { "one on now" } else { "none on now" };
            log::info!("calendar: {} meetings, {on_now}", u.meetings.len());
            state.calendar.keep(&u);
            Some(u)
        }
        Err(e) => {
            log::info!("calendar not read: {e}");
            state.calendar.forget();
            None
        }
    }
}

/// "Weekly sync · 14:00 · Teams" in this computer's time zone.
pub fn line(m: &Meeting) -> String {
    m.line(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC))
}

/// A recording started without a choice (window, menu bar, a box that named
/// no meeting): attach the meeting notizli.ch says is on now, if it answers.
/// Never delays the recording; a late answer for a finished recording, or
/// after "Not this meeting", changes nothing.
pub fn attach_now(app: &AppHandle, recording_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(up) = upcoming(&app, FRESH).await else { return };
        let Some(m) = up.now_meeting() else { return };
        let state = app.state::<AppState>();
        let mut guard = state.recording.lock().unwrap();
        let Some(active) = guard.as_mut().filter(|a| a.meta.id == recording_id) else { return };
        if active.meta.calendar_ref.is_some() {
            return;
        }
        active.meta.calendar_ref = Some(m.id.clone());
        active.meta.calendar_line = Some(line(m));
        let _ = state.store.save(&active.meta);
        drop(guard);
        crate::commands::changed(&app);
    });
}

/// Debug builds only: `NOTIZLI_FAKE_UPCOMING=<file.json>` (the server's
/// answer, as in docs) is read instead of asking notizli.ch, also unpaired.
#[cfg(debug_assertions)]
fn fake() -> Option<Upcoming> {
    let path = std::env::var("NOTIZLI_FAKE_UPCOMING").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).map_err(|e| log::warn!("NOTIZLI_FAKE_UPCOMING: {e}")).ok()
}

#[cfg(not(debug_assertions))]
fn fake() -> Option<Upcoming> {
    None
}
