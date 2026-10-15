//! The small "Record it?" box at the top right of the screen, naming the
//! calendar meeting when notizli.ch knows it. It never
//! takes the keyboard from the call and is hidden from screen sharing where
//! the system allows it.

use std::sync::Mutex;

use notizli_engine::{AppKind, MicUser};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::calendar;

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
}

/// What the box shows (the page asks for it when it loads).
#[derive(Default)]
pub struct PromptState(Mutex<Option<View>>);

pub fn ask(app: &AppHandle, user: &MicUser) {
    let title = match user.kind {
        AppKind::Call => format!("You're in a call ({}).", user.app),
        AppKind::Browser => format!("Your browser ({}) is using the microphone.", user.app),
    };
    show(app, View::Ask { title: title.clone(), question: "Record it?".into(), meeting: None });
    // Name the meeting once notizli.ch answers, if this box is still up.
    let app = app.clone();
    let user = user.clone();
    tauri::async_runtime::spawn(async move {
        let Some(up) = calendar::upcoming(&app, calendar::FRESH).await else { return };
        let Some(m) = up.now_meeting() else { return };
        let still_asking = matches!(current(&app), Some(View::Ask { meeting: None, title: t, .. }) if t == title);
        if still_asking {
            show(&app, View::Ask { title: calendar::line(m), question: named_question(&user), meeting: Some(m.id.clone()) });
        }
    });
}

/// The line under a named meeting: short enough that "Record it?" stays
/// visible in the box (about 45 characters fit).
fn named_question(user: &MicUser) -> String {
    match user.kind {
        AppKind::Call => format!("In a call ({}). Record it?", user.app),
        AppKind::Browser => format!("{} is using the mic. Record it?", user.app),
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
        let named = View::Ask { title: "Weekly sync · 14:00 · Teams".into(), question: "Record it?".into(), meeting: Some("g:1".into()) };
        assert_eq!(named.meeting(), Some(("g:1".to_string(), "Weekly sync · 14:00 · Teams".to_string())));
        let plain = View::Ask { title: "You're in a call (Zoom).".into(), question: "Record it?".into(), meeting: None };
        assert_eq!(plain.meeting(), None);
        assert_eq!(View::Countdown { seconds: 60 }.meeting(), None);
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
