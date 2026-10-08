# Call Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Notizli notices when a call app or browser uses the microphone and asks "Record this call?"; when the call ends during a recording it counts down 60 s and finishes. The app runs in the menu bar / notification area and starts at login.

**Architecture:** The engine gets a platform-neutral `detect` module (`MicUser`, `AppKind`, the pure `CallWatcher` state machine, macOS bundle-id classification) plus per-platform `mic_users()` (Core Audio process objects on macOS, capture-endpoint audio sessions on Windows). The app polls `mic_users()` once a second on its own thread, feeds the `CallWatcher` held in `AppState`, and shows a small always-on-top `prompt` window; finishing from Rust emits a `finished` event the main window uses like its own Finish button. A tray icon, close-to-hide and `tauri-plugin-autostart` make it a background app.

**Tech Stack:** Rust (objc2-core-audio 0.3, wasapi 0.24, sysinfo 0.39), Tauri 2.11 (`tray-icon`), tauri-plugin-autostart 2, plain HTML/JS.

Spec: `docs/superpowers/specs/2026-10-08-call-detection-design.md`.

Deviations from the spec, decided while planning (spec updated in Task 9):
- Poll every 1 s (not 2 s) so the 60 s countdown finishes on time; the call is cheap.
- The Windows app lists stay in `capture/windows.rs` (reused through its existing `classify`); the macOS bundle list lives in `detect.rs`, where it is unit-tested on every OS. No new shared module.
- The prompt window's capability grants events only; restricting app commands per window would need an app manifest for every command (not worth it for a local page).
- *Record* shows the main window without focusing it.

Verification commands used throughout:
- `cargo test --workspace --exclude notizli` (engine, core, rec-cli) — fast.
- `cargo check -p notizli-engine --no-default-features --target x86_64-pc-windows-msvc` — Windows engine code compiles (the target is installed).
- `cargo build -p notizli` — the app compiles on macOS.

---

### Task 1: `CallWatcher` and app classification (engine, pure)

**Files:**
- Create: `crates/engine/src/detect.rs`
- Modify: `crates/engine/src/lib.rs`

- [ ] **Step 1: Write `detect.rs` with types, empty logic and the tests**

```rust
//! Call detection: which apps are using a microphone, and when to ask
//! "Record this call?" or to say "The call ended". No OS calls here: the
//! platform code reports [`MicUser`]s, [`CallWatcher`] decides.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    Call,
    Browser,
}

/// A call app or browser that is using a microphone right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicUser {
    pub app: String,
    pub kind: AppKind,
}

/// How long an app must hold the microphone before we ask.
pub const ASK_AFTER: Duration = Duration::from_secs(10);
/// How long the question stays up.
pub const ASK_FOR: Duration = Duration::from_secs(60);
/// How long every call app must have let go before the call counts as ended.
pub const ENDED_AFTER: Duration = Duration::from_secs(5);
/// The countdown before finishing by itself.
pub const COUNTDOWN: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// Show "Record this call?" about this app.
    Ask(MicUser),
    /// Show "The call ended. Finishing in N seconds".
    Countdown { seconds: u64 },
    /// Close the box.
    Hide,
    /// The countdown ran out: finish the recording.
    Finish,
}

/// Fed once a second with the apps using a microphone; says what to show.
#[derive(Default)]
pub struct CallWatcher {
    since: HashMap<String, Instant>,
    asked: HashSet<String>,
    asking: Option<(MicUser, Instant)>,
    heard_during_recording: bool,
    released_at: Option<Instant>,
    deadline: Option<Instant>,
}

impl CallWatcher {
    pub fn update(&mut self, now: Instant, users: &[MicUser], recording: bool) -> Option<WatchEvent> {
        let _ = (now, users, recording);
        None
    }

    /// The user answered the question (Record or Not now): the app it was about.
    pub fn answered(&mut self) -> Option<MicUser> {
        None
    }

    /// "Keep recording" or "Finish now" in the countdown: stop counting; the
    /// countdown comes back only after a call app takes the microphone again
    /// and lets go.
    pub fn stop_countdown(&mut self) {}
}

/// macOS bundle ids (and their helpers' ids) of call apps and browsers.
const MAC_APPS: &[(&str, &str, AppKind)] = &[
    ("com.microsoft.teams", "Microsoft Teams", AppKind::Call),
    ("com.microsoft.teams2", "Microsoft Teams", AppKind::Call),
    ("us.zoom", "Zoom", AppKind::Call),
    ("com.webex", "Webex", AppKind::Call),
    ("com.cisco.webex", "Webex", AppKind::Call),
    ("cisco-systems.spark", "Webex", AppKind::Call),
    ("com.tinyspeck.slackmacgap", "Slack", AppKind::Call),
    ("net.whatsapp.whatsapp", "WhatsApp", AppKind::Call),
    ("desktop.whatsapp", "WhatsApp", AppKind::Call),
    ("com.hnc.discord", "Discord", AppKind::Call),
    ("com.skype.skype", "Skype", AppKind::Call),
    ("com.apple.facetime", "FaceTime", AppKind::Call),
    ("com.apple.avconferenced", "FaceTime", AppKind::Call),
    ("com.google.chrome", "Google Chrome", AppKind::Browser),
    ("com.microsoft.edgemac", "Microsoft Edge", AppKind::Browser),
    ("org.mozilla.firefox", "Firefox", AppKind::Browser),
    ("com.apple.safari", "Safari", AppKind::Browser),
    // Safari (and other WebKit views) capture in WebKit's own processes.
    ("com.apple.webkit", "Safari", AppKind::Browser),
    ("com.brave.browser", "Brave", AppKind::Browser),
    ("com.operasoftware.opera", "Opera", AppKind::Browser),
    ("com.vivaldi.vivaldi", "Vivaldi", AppKind::Browser),
    ("company.thebrowser.browser", "Arc", AppKind::Browser),
    ("ai.perplexity.comet", "Comet", AppKind::Browser),
];

/// The call app or browser a macOS bundle id belongs to: an exact match or a
/// helper below it (`com.google.Chrome.helper` → Google Chrome).
pub fn classify_bundle(bundle_id: &str) -> Option<MicUser> {
    let _ = bundle_id;
    None
}

/// One entry per app, call apps first.
pub fn dedupe(mut users: Vec<MicUser>) -> Vec<MicUser> {
    users
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom() -> MicUser {
        MicUser { app: "Zoom".into(), kind: AppKind::Call }
    }
    fn chrome() -> MicUser {
        MicUser { app: "Google Chrome".into(), kind: AppKind::Browser }
    }
    fn at(t0: Instant, s: u64) -> Instant {
        t0 + Duration::from_secs(s)
    }

    /// Feed `users` every second from `from` to `to` (inclusive) and collect events.
    fn run(w: &mut CallWatcher, t0: Instant, from: u64, to: u64, users: &[MicUser], recording: bool) -> Vec<(u64, WatchEvent)> {
        (from..=to).filter_map(|s| w.update(at(t0, s), users, recording).map(|e| (s, e))).collect()
    }

    #[test]
    fn asks_once_after_ten_seconds() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        assert_eq!(run(&mut w, t0, 0, 20, &[zoom()], false), vec![(10, WatchEvent::Ask(zoom()))]);
        assert_eq!(w.answered(), Some(zoom()));
        assert_eq!(run(&mut w, t0, 21, 40, &[zoom()], false), vec![]);
        // Zoom lets go; a new call asks again.
        assert_eq!(run(&mut w, t0, 41, 41, &[], false), vec![]);
        assert_eq!(run(&mut w, t0, 42, 52, &[zoom()], false), vec![(52, WatchEvent::Ask(zoom()))]);
    }

    #[test]
    fn question_closes_after_a_minute_and_is_not_repeated() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 10, &[zoom()], false);
        assert_eq!(run(&mut w, t0, 11, 80, &[zoom()], false), vec![(70, WatchEvent::Hide)]);
        assert_eq!(w.answered(), None);
    }

    #[test]
    fn question_closes_when_the_app_lets_go() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 10, &[zoom()], false);
        assert_eq!(run(&mut w, t0, 11, 11, &[], false), vec![(11, WatchEvent::Hide)]);
    }

    #[test]
    fn call_apps_come_before_browsers() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        assert_eq!(run(&mut w, t0, 0, 10, &[chrome(), zoom()], false), vec![(10, WatchEvent::Ask(zoom()))]);
    }

    #[test]
    fn starting_to_record_closes_the_question() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 10, &[zoom()], false);
        assert_eq!(run(&mut w, t0, 11, 11, &[zoom()], true), vec![(11, WatchEvent::Hide)]);
    }

    #[test]
    fn counts_down_and_finishes_after_the_call_ends() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        assert_eq!(run(&mut w, t0, 0, 29, &[zoom()], true), vec![]);
        let events = run(&mut w, t0, 30, 120, &[], true);
        assert_eq!(events, vec![(35, WatchEvent::Countdown { seconds: 60 }), (95, WatchEvent::Finish)]);
    }

    #[test]
    fn a_short_gap_is_not_the_end() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 9, &[zoom()], true);
        assert_eq!(run(&mut w, t0, 10, 12, &[], true), vec![]);
        assert_eq!(run(&mut w, t0, 13, 14, &[zoom()], true), vec![]);
        assert_eq!(run(&mut w, t0, 15, 20, &[], true), vec![(20, WatchEvent::Countdown { seconds: 60 })]);
    }

    #[test]
    fn the_call_coming_back_cancels_the_countdown() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 9, &[zoom()], true);
        run(&mut w, t0, 10, 15, &[], true);
        assert_eq!(run(&mut w, t0, 16, 16, &[zoom()], true), vec![(16, WatchEvent::Hide)]);
    }

    #[test]
    fn a_room_only_recording_never_ends_by_itself() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        assert_eq!(run(&mut w, t0, 0, 600, &[], true), vec![]);
    }

    #[test]
    fn keep_recording_waits_for_the_next_call() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 9, &[zoom()], true);
        run(&mut w, t0, 10, 15, &[], true);
        w.stop_countdown();
        assert_eq!(run(&mut w, t0, 16, 200, &[], true), vec![]);
        run(&mut w, t0, 201, 210, &[zoom()], true);
        assert_eq!(run(&mut w, t0, 211, 216, &[], true), vec![(216, WatchEvent::Countdown { seconds: 60 })]);
    }

    #[test]
    fn finishing_by_hand_closes_the_countdown() {
        let (mut w, t0) = (CallWatcher::default(), Instant::now());
        run(&mut w, t0, 0, 9, &[zoom()], true);
        run(&mut w, t0, 10, 15, &[], true);
        assert_eq!(run(&mut w, t0, 16, 16, &[], false), vec![(16, WatchEvent::Hide)]);
    }

    #[test]
    fn classifies_mac_bundle_ids() {
        let c = |id| classify_bundle(id).map(|u| (u.app, u.kind));
        assert_eq!(c("com.microsoft.teams2"), Some(("Microsoft Teams".into(), AppKind::Call)));
        assert_eq!(c("us.zoom.xos"), Some(("Zoom".into(), AppKind::Call)));
        assert_eq!(c("com.google.Chrome.helper"), Some(("Google Chrome".into(), AppKind::Browser)));
        assert_eq!(c("com.apple.WebKit.GPU"), Some(("Safari".into(), AppKind::Browser)));
        assert_eq!(c("com.microsoft.edgemac.helper"), Some(("Microsoft Edge".into(), AppKind::Browser)));
        assert_eq!(c("com.apple.Notes"), None);
        assert_eq!(c("ch.notizli.recorder"), None);
        assert_eq!(c("com.google.chromecast"), None);
        assert_eq!(c(""), None);
    }

    #[test]
    fn dedupes_call_apps_first() {
        let users = vec![chrome(), zoom(), chrome(), zoom()];
        assert_eq!(dedupe(users), vec![zoom(), chrome()]);
    }
}
```

In `lib.rs` add `mod detect;` (alphabetical, after `mod capture;`) and
`pub use detect::{classify_bundle, AppKind, CallWatcher, MicUser, WatchEvent};` after the `pub use capture::…` line.

- [ ] **Step 2: Run the tests, expect failures**

Run: `cargo test -p notizli-engine --lib detect`
Expected: compiles; most tests FAIL (empty logic).

- [ ] **Step 3: Implement**

Replace the stub bodies:

```rust
impl CallWatcher {
    pub fn update(&mut self, now: Instant, users: &[MicUser], recording: bool) -> Option<WatchEvent> {
        let holds = |app: &String| users.iter().any(|u| &u.app == app);
        self.since.retain(|app, _| holds(app));
        self.asked.retain(|app| holds(app));
        for u in users {
            self.since.entry(u.app.clone()).or_insert(now);
        }
        if recording {
            self.while_recording(now, users)
        } else {
            self.while_idle(now, users)
        }
    }

    fn while_recording(&mut self, now: Instant, users: &[MicUser]) -> Option<WatchEvent> {
        if self.asking.take().is_some() {
            return Some(WatchEvent::Hide);
        }
        if !users.is_empty() {
            self.heard_during_recording = true;
            self.released_at = None;
            return self.deadline.take().map(|_| WatchEvent::Hide);
        }
        if let Some(deadline) = self.deadline {
            if now >= deadline {
                self.stop_countdown();
                return Some(WatchEvent::Finish);
            }
            return None;
        }
        if !self.heard_during_recording {
            return None;
        }
        let released = *self.released_at.get_or_insert(now);
        if now.duration_since(released) >= ENDED_AFTER {
            self.released_at = None;
            self.deadline = Some(now + COUNTDOWN);
            return Some(WatchEvent::Countdown { seconds: COUNTDOWN.as_secs() });
        }
        None
    }

    fn while_idle(&mut self, now: Instant, users: &[MicUser]) -> Option<WatchEvent> {
        self.heard_during_recording = false;
        self.released_at = None;
        if self.deadline.take().is_some() {
            return Some(WatchEvent::Hide);
        }
        if let Some((user, shown)) = &self.asking {
            if !users.iter().any(|u| u.app == user.app) || now.duration_since(*shown) >= ASK_FOR {
                self.asking = None;
                return Some(WatchEvent::Hide);
            }
            return None;
        }
        let next = users
            .iter()
            .filter(|u| !self.asked.contains(&u.app))
            .filter(|u| self.since.get(&u.app).is_some_and(|t| now.duration_since(*t) >= ASK_AFTER))
            .min_by_key(|u| u.kind != AppKind::Call)?
            .clone();
        self.asked.insert(next.app.clone());
        self.asking = Some((next.clone(), now));
        Some(WatchEvent::Ask(next))
    }

    pub fn answered(&mut self) -> Option<MicUser> {
        self.asking.take().map(|(u, _)| u)
    }

    pub fn stop_countdown(&mut self) {
        self.deadline = None;
        self.released_at = None;
        self.heard_during_recording = false;
    }
}

pub fn classify_bundle(bundle_id: &str) -> Option<MicUser> {
    let id = bundle_id.to_ascii_lowercase();
    MAC_APPS
        .iter()
        .filter(|(prefix, _, _)| id == *prefix || id.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.')))
        .max_by_key(|(prefix, _, _)| prefix.len())
        .map(|(_, app, kind)| MicUser { app: (*app).to_string(), kind: *kind })
}

pub fn dedupe(mut users: Vec<MicUser>) -> Vec<MicUser> {
    users.sort_by(|a, b| (a.kind != AppKind::Call, &a.app).cmp(&(b.kind != AppKind::Call, &b.app)));
    users.dedup_by(|a, b| a.app == b.app);
    users
}
```

- [ ] **Step 4: Run the tests, expect PASS**

Run: `cargo test -p notizli-engine --lib detect` → all 13 pass.

- [ ] **Step 5: Commit** — `git add crates/engine && git commit -m "Engine: decide when to ask to record a call and when a call ended"`

### Task 2: `mic_users()` on macOS and Windows

**Files:**
- Modify: `crates/engine/src/capture/macos.rs` (imports, `devices()`, new `mic_users`)
- Modify: `crates/engine/src/capture/windows.rs` (new `mic_users`)
- Modify: `crates/engine/src/capture/mod.rs` (dispatcher)
- Modify: `crates/engine/src/lib.rs` (export)

- [ ] **Step 1: macOS.** Add `kAudioHardwarePropertyProcessObjectList, kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningInput, kAudioProcessPropertyPID` to the `objc2_core_audio` import. Replace `devices()` with a shared list reader and add `mic_users`:

```rust
/// An AudioObjectID list property of the system object.
fn object_list(selector: u32) -> Vec<AudioObjectID> {
    let Ok(buf) = get_bytes(system(), selector, kAudioObjectPropertyScopeGlobal) else { return Vec::new() };
    // SAFETY: the property is an array of AudioObjectID (u32).
    let ids = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<AudioObjectID>(), buf.len() * 2) };
    ids.iter().copied().filter(|id| *id != 0).collect()
}

fn devices() -> Vec<AudioObjectID> {
    object_list(kAudioHardwarePropertyDevices)
}

/// Call apps and browsers using any microphone now (Core Audio process
/// objects, macOS 14+; no permission needed). Notizli itself is skipped.
pub fn mic_users() -> Vec<MicUser> {
    let own = std::process::id() as i32;
    let users = object_list(kAudioHardwarePropertyProcessObjectList)
        .into_iter()
        .filter(|p| get::<u32>(*p, kAudioProcessPropertyIsRunningInput, kAudioObjectPropertyScopeGlobal).is_ok_and(|v| v != 0))
        .filter(|p| get::<i32>(*p, kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal).is_ok_and(|pid| pid != own))
        .filter_map(|p| get_string(p, kAudioProcessPropertyBundleID, kAudioObjectPropertyScopeGlobal).ok())
        .filter_map(|id| classify_bundle(&id))
        .collect();
    dedupe(users)
}
```
with `use crate::detect::{classify_bundle, dedupe, MicUser};`.

- [ ] **Step 2: Windows.** Add to `windows.rs` (after `input_devices`), with `use crate::detect::{dedupe, AppKind, MicUser};`:

```rust
/// Call apps and browsers capturing from any microphone now: the active
/// audio sessions on every capture endpoint. Notizli itself is skipped.
pub fn mic_users() -> Vec<MicUser> {
    com();
    let own_pid = std::process::id();
    let Ok(enumerator) = DeviceEnumerator::new() else { return Vec::new() };
    let mut pids = Vec::new();
    if let Ok(devices) = enumerator.get_device_collection(&Direction::Capture) {
        for device in &devices {
            let Ok(device) = device else { continue };
            let Ok(manager) = device.get_iaudiosessionmanager() else { continue };
            let Ok(list) = manager.get_audiosessionenumerator() else { continue };
            for i in 0..list.get_count().unwrap_or(0) {
                let Ok(control) = list.get_session(i) else { continue };
                if control.get_state().ok() != Some(SessionState::Active) {
                    continue;
                }
                let pid = control.get_process_id().unwrap_or(0);
                if pid != 0 && pid != own_pid {
                    pids.push(pid);
                }
            }
        }
    }
    let mut system = System::new();
    let names = process_names(&mut system, pids.iter().copied());
    let users = pids
        .into_iter()
        .filter_map(|pid| match classify(&names, pid, own_pid) {
            (0, Some(app)) => Some(MicUser { app, kind: AppKind::Call }),
            (1, Some(app)) => Some(MicUser { app, kind: AppKind::Browser }),
            _ => None,
        })
        .collect();
    dedupe(users)
}
```

- [ ] **Step 3: Dispatcher** in `capture/mod.rs` after `input_devices`:

```rust
/// Call apps and browsers using a microphone right now (empty where unsupported).
pub fn mic_users() -> Vec<crate::detect::MicUser> {
    #[cfg(windows)]
    return windows::mic_users();
    #[cfg(target_os = "macos")]
    return macos::mic_users();
    #[cfg(not(any(windows, target_os = "macos")))]
    Vec::new()
}
```
and in `lib.rs`: `pub use capture::{input_devices, mic_users, request_microphone_access, InputDevice};`.

- [ ] **Step 4: Test-tool command** `notizli-rec calls` in `crates/rec-cli/src/main.rs`: prints the apps using the microphone whenever the list changes, for 2 minutes. Add `Some("calls") => calls(),` to the match and to both usage strings (`notizli-rec calls`), and:

```rust
/// Show which call apps and browsers use the microphone (call detection).
fn calls() -> i32 {
    println!("Apps using the microphone (call apps and browsers), for 2 minutes:");
    let mut last = None;
    for _ in 0..120 {
        let now: Vec<String> = notizli_engine::mic_users().into_iter().map(|u| format!("{} ({:?})", u.app, u.kind)).collect();
        if last.as_ref() != Some(&now) {
            println!("{}", if now.is_empty() { "  none".to_string() } else { format!("  {}", now.join(", ")) });
            last = Some(now);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    0
}
```

- [ ] **Step 5: Verify**
  - `cargo test --workspace --exclude notizli` → pass.
  - `cargo check -p notizli-engine --no-default-features --target x86_64-pc-windows-msvc` → ok.
  - `cargo run -q -p notizli-rec -- calls` on this Mac for a few seconds (Ctrl+C / timeout): prints "none" or the apps using the mic. Expected: no crash, plausible output.

- [ ] **Step 6: Commit** — "Engine: list the call apps and browsers using the microphone (macOS, Windows); notizli-rec calls"

### Task 3: Settings

**Files:** Modify `crates/core/src/account.rs`

- [ ] **Step 1: Failing test** (in the existing `#[cfg(test)]` module of `account.rs`, or a new one):

```rust
#[test]
fn new_settings_ask_on_calls() {
    assert!(Settings::default().ask_on_calls);
    let old: Settings = serde_json::from_str(r#"{"paired_email":"a@b.ch"}"#).unwrap();
    assert!(old.ask_on_calls);
    assert_eq!(old.open_at_login, None);
}
```

- [ ] **Step 2:** `cargo test -p notizli-core new_settings` → FAIL (no field).

- [ ] **Step 3: Implement.** Add to `Settings` and replace `Default` in the derive with a manual impl:

```rust
    /// Ask "Record this call?" when a call app or browser uses the microphone.
    #[serde(default = "yes")]
    pub ask_on_calls: bool,
    /// Start at login; `None` until decided (turned on at the first start).
    #[serde(default)]
    pub open_at_login: Option<bool>,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { paired_email: None, device_label: None, upload_url: None, mic_device: None, ask_on_calls: true, open_at_login: None }
    }
}
```

- [ ] **Step 4:** `cargo test -p notizli-core` → PASS. **Commit** — "Settings: ask on calls (default on), open at login"

### Task 4: The prompt window

**Files:**
- Create: `app/src-tauri/src/prompt.rs`, `app/ui/prompt.html`, `app/ui/prompt.js`, `app/src-tauri/capabilities/prompt.json`
- Modify: `app/src-tauri/src/main.rs` (`mod prompt;`, `.manage(prompt::PromptState::default())`)

- [ ] **Step 1: `prompt.rs`**

```rust
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
```

- [ ] **Step 2: `prompt.html`**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <title>Notizli</title>
  <link rel="stylesheet" href="./styles.css" />
  <style>
    body { padding: 0; background: var(--paper); overflow: hidden; }
    .box { border: 1px solid var(--ink); height: 100vh; padding: 14px 16px; display: flex; flex-direction: column; gap: 4px; }
    .head { font-family: var(--font-display); font-weight: 600; font-size: 12px; letter-spacing: -0.01em; }
    .head .accent { color: var(--signal); }
    #title { font-weight: 500; }
    #question { color: var(--ink-muted); }
    .actions { display: flex; gap: 8px; margin-top: auto; }
    .actions button { flex: 1; }
  </style>
</head>
<body>
  <div class="box">
    <div class="head">Notizli<span class="accent">.</span></div>
    <div id="title"></div>
    <div id="question"></div>
    <div class="actions">
      <button class="primary" id="yes"></button>
      <button id="no"></button>
    </div>
  </div>
  <script src="./prompt.js"></script>
</body>
</html>
```

- [ ] **Step 3: `prompt.js`**

```js
// The "Record this call?" box. Rust decides what it shows; buttons call back.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
let tick = null;

function render(v) {
  clearInterval(tick);
  if (!v) return;
  if (v.mode === "ask") {
    $("title").textContent = v.title;
    $("question").textContent = "Record it?";
    $("yes").textContent = "Record";
    $("no").textContent = "Not now";
    $("yes").onclick = () => invoke("prompt_record");
    $("no").onclick = () => invoke("prompt_dismiss");
    return;
  }
  let left = v.seconds;
  const update = () => ($("question").textContent = `Finishing in ${left} ${left === 1 ? "second" : "seconds"}`);
  $("title").textContent = "The call ended.";
  update();
  tick = setInterval(() => {
    left = Math.max(0, left - 1);
    update();
  }, 1000);
  $("yes").textContent = "Finish now";
  $("no").textContent = "Keep recording";
  $("yes").onclick = () => invoke("prompt_finish_now");
  $("no").onclick = () => invoke("prompt_keep");
}

listen("prompt", ({ payload }) => render(payload));
invoke("prompt_view").then(render);
```

- [ ] **Step 4: `capabilities/prompt.json`**

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "prompt",
  "description": "The \"Record this call?\" box: app commands and events only.",
  "windows": ["prompt"],
  "permissions": ["core:event:default"]
}
```

- [ ] **Step 5:** In `main.rs`: `mod prompt;` and `.manage(prompt::PromptState::default())` on the builder (before `.setup`). `cargo build -p notizli` → compiles (warnings about unused fns are expected until Task 5). No commit yet (Task 5 wires it).

### Task 5: Detector thread, prompt commands, finishing from Rust

**Files:**
- Create: `app/src-tauri/src/detect.rs`
- Modify: `app/src-tauri/src/state.rs`, `app/src-tauri/src/commands.rs`, `app/src-tauri/src/main.rs`, `app/ui/app.js`

- [ ] **Step 1: State.** In `state.rs`: `use notizli_engine::{CallWatcher, Recorder};`, field `pub watcher: Mutex<CallWatcher>,` on `AppState`, initialised `watcher: Mutex::new(CallWatcher::default()),`.

- [ ] **Step 2: `detect.rs`**

```rust
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
        let enabled = state.token().is_some() && state.settings.lock().unwrap().ask_on_calls;
        if !enabled {
            *state.watcher.lock().unwrap() = CallWatcher::default();
            if prompt::current(app).is_some() {
                prompt::hide(app);
            }
            continue;
        }
        let users = fake_users(started).unwrap_or_else(mic_users);
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
/// microphone for that long after start, to try the box without a call.
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
```

- [ ] **Step 3: Commands** in `commands.rs`:
  - Replace `default_title` with:

```rust
fn now_text() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let f = time::macros::format_description!("[day] [month repr:short] [year], [hour]:[minute]");
    now.format(&f).unwrap_or_default()
}

fn default_title() -> String {
    format!("Desktop recording — {}", now_text())
}

fn call_title(app: &str) -> String {
    format!("{app} call — {}", now_text())
}
```
  - Make `focus` public, add `reveal` and `set_dock`:

```rust
/// Show the window and bring it to the front.
pub fn focus(app: &AppHandle) {
    set_dock(app, true);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Show the window without taking the keyboard from the call.
pub fn reveal(app: &AppHandle) {
    set_dock(app, true);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
    }
}

/// macOS: the Dock icon only while the window is open.
pub fn set_dock(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    let _ = app.set_dock_visibility(visible);
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}
```
  - `changed()` also calls `crate::tray::refresh(app);` (added in Task 6; until then leave it out).
  - Derive `Clone` on `FinishView`. Add the call-detection section:

```rust
// ---- call detection ---------------------------------------------------------

#[tauri::command]
pub fn prompt_view(app: AppHandle) -> Option<prompt::View> {
    prompt::current(&app)
}

/// "Record" in the box.
#[tauri::command]
pub async fn prompt_record(app: AppHandle) -> Res<()> {
    let user = app.state::<AppState>().watcher.lock().unwrap().answered();
    prompt::hide(&app);
    let name = user.map(|u| u.app).unwrap_or_else(|| "Call".into());
    log::info!("record ({name})");
    match start_recording(app.clone(), Some(call_title(&name))).await {
        Ok(_) => {
            reveal(&app);
            Ok(())
        }
        Err(e) => {
            focus(&app);
            notice(&app, e.clone());
            Err(e)
        }
    }
}

/// "Not now" in the box.
#[tauri::command]
pub fn prompt_dismiss(app: AppHandle) {
    if let Some(u) = app.state::<AppState>().watcher.lock().unwrap().answered() {
        log::info!("not now ({})", u.app);
    }
    prompt::hide(&app);
}

/// "Keep recording" in the countdown.
#[tauri::command]
pub fn prompt_keep(app: AppHandle) {
    app.state::<AppState>().watcher.lock().unwrap().stop_countdown();
    log::info!("keep recording");
    prompt::hide(&app);
}

/// "Finish now" in the countdown.
#[tauri::command]
pub fn prompt_finish_now(app: AppHandle) {
    app.state::<AppState>().watcher.lock().unwrap().stop_countdown();
    log::info!("finish now");
    prompt::hide(&app);
    finish_in_background(&app);
}

/// Finish without the window asking (countdown, tray menu). The window
/// learns the result from the "finished" event.
pub fn finish_in_background(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match finish_recording(app.clone()).await {
            Ok(view) => {
                let _ = app.emit("finished", view);
            }
            Err(e) => {
                log::warn!("finishing: {e}");
                notice(&app, e);
            }
        }
    });
}
```
  with `use crate::prompt;`.

- [ ] **Step 4: main.rs**: `mod detect;`, register `commands::prompt_view, commands::prompt_record, commands::prompt_dismiss, commands::prompt_keep, commands::prompt_finish_now` in `generate_handler!`, and `detect::spawn(app.handle().clone());` after `uploader::spawn(…)`.

- [ ] **Step 5: app.js** — finishing from Rust reuses the window's own flow. Replace the body of `finishRecording` after `show("s-uploading");`:

```js
  try {
    applyFinish(await invoke("finish_recording"));
  } catch (e) {
    // Finished at the same moment from the countdown or the menu: the "finished" event shows it.
    if (errText(e) === "Not recording.") return;
    mode = { error: { title: "The recording could not be finished", message: errText(e) } };
  } finally {
    $("stop-btn").disabled = false;
  }
  await refresh();
  settleUpload();
}

// The recording was finished (here, by the countdown or from the menu).
function applyFinish(r) {
  $("meeting-name").value = "";
  lastCheck = { id: r.id, ok: r.heard_ok, text: r.verdict };
  if (r.saved_elsewhere) {
    mode = {
      error: {
        title: "Saved outside Notizli's folder",
        message: `The disk failed during the recording, so it was saved here instead: ${r.saved_elsewhere}`,
        note: "Upload it on notizli.ch, or keep the file.",
      },
    };
  } else if (!r.paired) {
    mode = { done: { id: r.id, meeting_id: null } };
  } else {
    mode = { uploading: r.id };
  }
}
```
and next to the other listeners:

```js
listen("finished", async ({ payload }) => {
  applyFinish(payload);
  await refresh();
  settleUpload();
});
```

- [ ] **Step 6: Verify** — `cargo build -p notizli` compiles without warnings in the new code; `cargo test --workspace --exclude notizli` passes.

- [ ] **Step 7: Commit** — "App: ask \"Record this call?\" when a call starts, count down when it ends"

### Task 6: Menu bar / notification area icon, close to hide, hidden start

**Files:**
- Create: `app/src-tauri/src/tray.rs`
- Modify: `app/src-tauri/Cargo.toml` (`tauri = { version = "2", features = ["tray-icon"] }`), `app/src-tauri/src/main.rs`, `app/src-tauri/src/commands.rs` (`changed`), `app/src-tauri/tauri.conf.json` (`"visible": false` on the main window)

- [ ] **Step 1: `tray.rs`**

```rust
//! The menu bar (macOS) / notification area (Windows) icon: start or finish
//! a recording, open the window, quit.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::commands;
use crate::state::AppState;

const ID: &str = "main";

pub struct Tray {
    record: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let record = MenuItem::with_id(app, "record", "Start recording", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Notizli", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Notizli", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&record, &open, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id(ID).menu(&menu).tooltip("Notizli").on_menu_event(|app, event| match event.id().as_ref() {
        "record" => toggle_recording(app),
        "open" => commands::focus(app),
        "quit" => quit_app(app),
        _ => {}
    });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(Tray { record });
    Ok(())
}

/// Keep the menu in step with the recording.
pub fn refresh(app: &AppHandle) {
    let recording = app.state::<AppState>().is_recording();
    if let Some(t) = app.try_state::<Tray>() {
        let _ = t.record.set_text(if recording { "Finish recording" } else { "Start recording" });
    }
    if let Some(tray) = app.tray_by_id(ID) {
        let _ = tray.set_tooltip(Some(if recording { "Notizli — recording" } else { "Notizli" }));
    }
}

fn toggle_recording(app: &AppHandle) {
    if app.state::<AppState>().is_recording() {
        commands::finish_in_background(app);
        return;
    }
    commands::focus(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = commands::start_recording(app.clone(), None).await {
            commands::notice(&app, e);
        }
    });
}

fn quit_app(app: &AppHandle) {
    if app.state::<AppState>().is_recording() {
        commands::focus(app);
        let _ = app.emit("confirm-quit", ());
    } else {
        app.exit(0);
    }
}
```

- [ ] **Step 2: main.rs**
  - `mod tray;`
  - single-instance callback body becomes `commands::focus(app);`
  - in `setup`, after `detect::spawn(…)`:

```rust
            tray::create(app.handle())?;
            // Started at login: stay in the menu bar / notification area.
            if std::env::args().any(|a| a == "--hidden") {
                commands::set_dock(app.handle(), false);
            } else {
                commands::focus(app.handle());
            }
```
  - `on_window_event`: closing the window hides it (the recording continues):

```rust
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    commands::set_dock(window.app_handle(), false);
                }
            }
        })
```
  - `app.run`: add `if let RunEvent::Reopen { .. } = &event { commands::focus(app); }` (macOS Dock click). The `Reopen` variant only exists on macOS: wrap in `#[cfg(target_os = "macos")]`.
  - `commands::changed` gets `crate::tray::refresh(app);`.
  - `tauri.conf.json` main window: add `"visible": false`.

- [ ] **Step 3:** `cargo build -p notizli` → ok. **Commit** — "App: lives in the menu bar / notification area; closing the window hides it"

### Task 7: Open at login + the two switches

**Files:**
- Modify: `app/src-tauri/Cargo.toml` (`tauri-plugin-autostart = "2"`), `main.rs`, `commands.rs`, `app/ui/index.html`, `app/ui/app.js`, `app/ui/styles.css`

- [ ] **Step 1: commands.rs** — `StateView` gets `ask_on_calls: bool, open_at_login: bool` (filled from settings: `settings.ask_on_calls`, `settings.open_at_login.unwrap_or(false)`), and:

```rust
// ---- settings -------------------------------------------------------------------

#[tauri::command]
pub fn set_ask_on_calls(app: AppHandle, state: State<'_, AppState>, on: bool) {
    state.update_settings(|s| s.ask_on_calls = on);
    log::info!("ask to record when a call starts: {on}");
    changed(&app);
}

#[tauri::command]
pub fn set_open_at_login(app: AppHandle, on: bool) -> Res<()> {
    apply_open_at_login(&app, on)?;
    changed(&app);
    Ok(())
}

/// Register (or remove) Notizli as a login item, started with --hidden.
pub fn apply_open_at_login(app: &AppHandle, on: bool) -> Res<()> {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    if on { launcher.enable() } else { launcher.disable() }.map_err(|e| format!("Couldn't change the login item: {e}"))?;
    app.state::<AppState>().update_settings(|s| s.open_at_login = Some(on));
    log::info!("open at login: {on}");
    Ok(())
}
```

- [ ] **Step 2: main.rs** — plugin `.plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))`, register `commands::set_ask_on_calls, commands::set_open_at_login`, and in `setup` after `app.manage(state)`:

```rust
            // First start of a release build: open at login, as agreed.
            // Never for development builds (it would register target/debug).
            let undecided = app.state::<AppState>().settings.lock().unwrap().open_at_login.is_none();
            if undecided && !cfg!(debug_assertions) {
                if let Err(e) = commands::apply_open_at_login(app.handle(), true) {
                    log::warn!("{e}");
                }
            }
```

- [ ] **Step 3: index.html** — in the idle section, after the microphone `<select>`:

```html
    <label class="check-row"><input type="checkbox" id="ask-on-calls" /> Ask me to record when a call starts</label>
    <label class="check-row"><input type="checkbox" id="open-at-login" /> Open Notizli when I log in</label>
```
styles.css:

```css
label.check-row { display: flex; align-items: center; gap: 8px; margin-top: 12px; font-size: 13px; cursor: pointer; }
label.check-row input { accent-color: var(--signal); }
```
app.js — in `renderIdle()`: `$("ask-on-calls").checked = S.ask_on_calls; $("open-at-login").checked = S.open_at_login;` and with the buttons:

```js
$("ask-on-calls").onchange = () => invoke("set_ask_on_calls", { on: $("ask-on-calls").checked }).catch((e) => toast(errText(e)));
$("open-at-login").onchange = () =>
  invoke("set_open_at_login", { on: $("open-at-login").checked }).catch((e) => {
    toast(errText(e));
    refresh();
  });
```

- [ ] **Step 4:** `cargo build -p notizli` → ok. **Commit** — "App: open at login (on by default) and the two switches"

### Task 8: Try it locally

- [ ] `cd app && npx tauri build --debug --bundles app` (or `cargo build -p notizli`), then run `NOTIZLI_FAKE_CALL=40 target/debug/notizli` with the real (paired) settings. Expected log in `~/Library/Application Support/ch.notizli.recorder/logs/notizli.log`: "using the microphone: Zoom", after 10 s "asking whether to record (Zoom)", and the box at the top right (screenshot to check it). Click **Not now**: box closes, log "not now (Zoom)". After 40 s "using the microphone: no call app or browser". Check the menu bar icon and its menu, closing the window hides it, Open Notizli brings it back. Quit from the menu.
- [ ] Also run `cargo run -q -p notizli-rec -- calls` while a browser tab uses the microphone, if one can be arranged; otherwise rely on the user's test.

### Task 9: Docs

- [ ] README: new section "Call detection" under "How recording works" (what is watched, no permission, timings, never records without a click); Layout table unchanged; "Not done yet" unchanged.
- [ ] TESTING.md: new section **D. Call detection** (Teams app, Zoom, Meet in Chrome, Safari; Not now; let the countdown run out; Keep recording; share the screen while the box shows; restart and check the icon), and `notizli-rec calls` for Windows.
- [ ] Spec: apply the deviations listed at the top of this plan.
- [ ] Commit — "Docs: call detection", then `git push`.
