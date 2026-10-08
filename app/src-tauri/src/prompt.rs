//! The small "Record this call?" box at the top right of the screen. It never
//! takes the keyboard from the call and is hidden from screen sharing where
//! the system allows it.

use std::sync::Mutex;

use notizli_engine::{AppKind, MicUser};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "prompt";
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 132.0;
const MARGIN: f64 = 16.0;

#[derive(Clone, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum View {
    Ask { title: String },
    Countdown { seconds: u64 },
}

/// What the box shows (the page asks for it when it loads).
#[derive(Default)]
pub struct PromptState(Mutex<Option<View>>);

pub fn ask(app: &AppHandle, user: &MicUser) {
    let title = match user.kind {
        AppKind::Call => format!("You're in a call ({}).", user.app),
        AppKind::Browser => format!("Your browser ({}) is using the microphone.", user.app),
    };
    show(app, View::Ask { title });
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
