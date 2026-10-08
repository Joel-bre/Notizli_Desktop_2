//! Once a second: which call apps and browsers use the microphone, and
//! whether to ask "Record this call?" or count down after the call ended.
//! Only while paired and "Ask me to record when a call starts" is on.

use std::time::{Duration, Instant};

use notizli_engine::{mic_users, CallWatcher, MicUser, WatchEvent};
use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::{commands, prompt};

const EVERY: Duration = Duration::from_secs(1);

pub fn spawn(app: AppHandle) {
    let started = Instant::now();
    if let Err(e) = std::thread::Builder::new().name("notizli-detect".into()).spawn(move || run(&app, started)) {
        log::warn!("call detection did not start: {e}");
    }
}

fn run(app: &AppHandle, started: Instant) {
    let mut last: Vec<String> = Vec::new();
    loop {
        std::thread::sleep(EVERY);
        let state = app.state::<AppState>();
        let fake = fake_users(started);
        let enabled = (state.token().is_some() || fake.is_some()) && state.settings.lock().unwrap().ask_on_calls;
        if !enabled {
            *state.watcher.lock().unwrap() = CallWatcher::default();
            if prompt::current(app).is_some() {
                prompt::hide(app);
            }
            continue;
        }
        let users = fake.unwrap_or_else(mic_users);
        let names: Vec<String> = users.iter().map(|u| u.app.clone()).collect();
        if names != last {
            log::info!("using the microphone: {}", if names.is_empty() { "no call app or browser".to_string() } else { names.join(", ") });
            last = names;
        }
        let recording = state.is_recording();
        let event = state.watcher.lock().unwrap().update(Instant::now(), &users, recording);
        match event {
            Some(WatchEvent::Ask(user)) => {
                log::info!("asking whether to record ({})", user.app);
                prompt::ask(app, &user);
            }
            Some(WatchEvent::Countdown { seconds }) => {
                log::info!("the call ended: finishing in {seconds} s unless kept");
                prompt::countdown(app, seconds);
            }
            Some(WatchEvent::Hide) => prompt::hide(app),
            Some(WatchEvent::Finish) => {
                log::info!("finishing: nobody answered the countdown");
                prompt::hide(app);
                commands::finish_in_background(app);
            }
            None => {}
        }
    }
}

/// Debug builds only: `NOTIZLI_FAKE_CALL=<seconds>` pretends Zoom uses the
/// microphone for that long after start, to try the box without a call
/// (also unpaired).
#[cfg(debug_assertions)]
fn fake_users(started: Instant) -> Option<Vec<MicUser>> {
    let secs: u64 = std::env::var("NOTIZLI_FAKE_CALL").ok()?.parse().ok()?;
    let zoom = MicUser { app: "Zoom".into(), kind: notizli_engine::AppKind::Call };
    Some(if started.elapsed() < Duration::from_secs(secs) { vec![zoom] } else { Vec::new() })
}

#[cfg(not(debug_assertions))]
fn fake_users(_: Instant) -> Option<Vec<MicUser>> {
    None
}
