//! The small "Record it?" box at the top right of the screen, naming the
//! calendar meeting when notizli.ch knows it, also when a meeting with no
//! video link starts. It never
//! takes the keyboard from the call and is hidden from screen sharing where
//! the system allows it.

use std::sync::Mutex;

use notizli_core::calendar::Upcoming;
use notizli_engine::{AppKind, MicUser};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::calendar;
use crate::state::AppState;

const LABEL: &str = "prompt";
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 132.0;
const MARGIN: f64 = 16.0;

#[derive(Clone, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum View {
    Ask {
        title: String,
        question: String,
        /// The calendar meeting the box names; Record records for it.
        #[serde(skip)]
        meeting: Option<String>,
        /// Asked because a meeting with no video link started, not because of a call.
        #[serde(skip)]
        from_calendar: bool,
    },
    Countdown { seconds: u64 },
}

impl View {
    /// The meeting the box names and its line ("Weekly sync · 14:00 ·
    /// Teams"): Record records for it.
    pub fn meeting(&self) -> Option<(String, String)> {
        match self {
            View::Ask { title, meeting: Some(id), .. } => Some((id.clone(), title.clone())),
            _ => None,
        }
    }

    /// The box asks about a meeting with no video link (not about a call).
    pub fn asks_about_meeting(&self) -> bool {
        matches!(self, View::Ask { from_calendar: true, .. })
    }

    /// The in-person box for this meeting.
    fn is_in_person(&self, id: &str) -> bool {
        matches!(self, View::Ask { meeting: Some(m), from_calendar: true, .. } if m == id)
    }
}

/// What the box shows (the page asks for it when it loads).
#[derive(Default)]
pub struct PromptState(Mutex<Option<View>>);

pub fn ask(app: &AppHandle, user: &MicUser) {
    let title = match user.kind {
        AppKind::Call => format!("You're in a call ({}).", user.app),
        AppKind::Browser => format!("Your browser ({}) is using the microphone.", user.app),
    };
    show(app, View::Ask { title: title.clone(), question: "Record it?".into(), meeting: None, from_calendar: false });
    // Name the meeting once notizli.ch answers, if this box is still up.
    let app = app.clone();
    let user = user.clone();
    tauri::async_runtime::spawn(async move {
        let Some(up) = calendar::upcoming(&app, calendar::FRESH).await else { return };
        let Some(m) = up.now_meeting_at(time::OffsetDateTime::now_utc()) else { return };
        let still_asking = matches!(current(&app), Some(View::Ask { meeting: None, from_calendar: false, title: t, .. }) if t == title);
        if still_asking {
            show(&app, View::Ask { title: calendar::line(m), question: named_question(&user), meeting: Some(m.id.clone()), from_calendar: false });
        }
    });
}

/// A meeting with no video link just started: call detection can't notice
/// it, so ask here. Only with the switch on, not recording, no box showing;
/// once per meeting; gone after 60 s like the call box. The caller (the
/// calendar loop) runs only while paired.
pub fn maybe_ask_in_person(app: &AppHandle, up: &Upcoming) {
    let state = app.state::<AppState>();
    if state.is_recording() || current(app).is_some() || !state.settings.lock().unwrap().ask_on_calls {
        return;
    }
    let mut asked = state.calendar.asked.lock().unwrap();
    let Some(m) = up.in_person_due(time::OffsetDateTime::now_utc(), &asked) else { return };
    asked.insert(m.id.clone());
    drop(asked);
    log::info!("asking whether to record (a meeting with no video link started)");
    let id = m.id.clone();
    show(app, in_person_view(&id, calendar::line(m)));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        if current(&app).is_some_and(|v| v.is_in_person(&id)) {
            hide(&app);
        }
    });
}

fn in_person_view(id: &str, line: String) -> View {
    View::Ask { title: line, question: "Starting now. Record it?".into(), meeting: Some(id.to_string()), from_calendar: true }
}

/// The line under a named meeting: short enough that "Record it?" stays
/// visible in the box (about 45 characters fit).
fn named_question(user: &MicUser) -> String {
    match user.kind {
        AppKind::Call => format!("In a call ({}). Record it?", user.app),
        AppKind::Browser => format!("{} is using the mic. Record it?", user.app),
    }
}

/// A recording started some other way: the in-person box has no more question.
pub fn hide_in_person(app: &AppHandle) {
    if current(app).is_some_and(|v| v.asks_about_meeting()) {
        hide(app);
    }
}

pub fn countdown(app: &AppHandle, seconds: u64) {
    show(app, View::Countdown { seconds });
}

pub fn hide(app: &AppHandle) {
    *app.state::<PromptState>().0.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

pub fn current(app: &AppHandle) -> Option<View> {
    app.state::<PromptState>().0.lock().unwrap().clone()
}

fn show(app: &AppHandle, view: View) {
    *app.state::<PromptState>().0.lock().unwrap() = Some(view.clone());
    match window(app) {
        Ok(w) => {
            place(&w);
            let _ = app.emit_to(LABEL, "prompt", &view);
            let _ = w.show();
        }
        Err(e) => log::warn!("prompt window: {e}"),
    }
}

fn window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Ok(w);
    }
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("prompt.html".into()))
        .title("Notizli")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .content_protected(true)
        .shadow(true)
        .visible(false)
        .build()
}

/// Top right of the main screen's usable area.
fn place(w: &WebviewWindow) {
    let Ok(Some(m)) = w.primary_monitor() else { return };
    let (area, scale) = (m.work_area(), m.scale_factor());
    let x = area.position.x as f64 + area.size.width as f64 - (WIDTH + MARGIN) * scale;
    let y = area.position.y as f64 + MARGIN * scale;
    let _ = w.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(app: &str, kind: AppKind) -> MicUser {
        MicUser { app: app.into(), kind }
    }

    #[test]
    fn record_takes_the_meeting_and_its_line_from_the_box() {
        let named = View::Ask { title: "Weekly sync · 14:00 · Teams".into(), question: "Record it?".into(), meeting: Some("g:1".into()), from_calendar: false };
        assert_eq!(named.meeting(), Some(("g:1".to_string(), "Weekly sync · 14:00 · Teams".to_string())));
        let plain = View::Ask { title: "You're in a call (Zoom).".into(), question: "Record it?".into(), meeting: None, from_calendar: false };
        assert_eq!(plain.meeting(), None);
        assert_eq!(View::Countdown { seconds: 60 }.meeting(), None);
    }

    #[test]
    fn the_in_person_box_names_the_meeting_and_records_for_it() {
        let v = in_person_view("g:V", "Visit · 12:00 · In person or by phone".into());
        assert!(matches!(&v, View::Ask { question, from_calendar: true, .. } if question == "Starting now. Record it?"));
        assert_eq!(v.meeting(), Some(("g:V".to_string(), "Visit · 12:00 · In person or by phone".to_string())));
        assert!(v.is_in_person("g:V"));
        assert!(v.asks_about_meeting());
        assert!(!View::Countdown { seconds: 60 }.asks_about_meeting());
        assert!(!v.is_in_person("g:other"));
        let call = View::Ask { title: "t".into(), question: "q".into(), meeting: Some("g:V".into()), from_calendar: false };
        assert!(!call.is_in_person("g:V"), "a call box that names the meeting isn't the in-person box");
    }

    #[test]
    fn the_question_under_a_named_meeting_fits_on_one_line() {
        assert_eq!(named_question(&user("Zoom", AppKind::Call)), "In a call (Zoom). Record it?");
        assert_eq!(named_question(&user("Google Chrome", AppKind::Browser)), "Google Chrome is using the mic. Record it?");
        // About 45 characters fit in the box at 14 px; the question must survive.
        for u in [user("Microsoft Teams", AppKind::Call), user("Microsoft Edge", AppKind::Browser)] {
            assert!(named_question(&u).chars().count() <= 45, "{}", named_question(&u));
        }
    }
}
