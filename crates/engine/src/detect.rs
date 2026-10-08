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

    /// The user answered the question (Record or Not now): the app it was about.
    pub fn answered(&mut self) -> Option<MicUser> {
        self.asking.take().map(|(u, _)| u)
    }

    /// "Keep recording" or "Finish now" in the countdown: stop counting; the
    /// countdown comes back only after a call app takes the microphone again
    /// and lets go.
    pub fn stop_countdown(&mut self) {
        self.deadline = None;
        self.released_at = None;
        self.heard_during_recording = false;
    }
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
    let id = bundle_id.to_ascii_lowercase();
    MAC_APPS
        .iter()
        .filter(|(prefix, _, _)| id == *prefix || id.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.')))
        .max_by_key(|(prefix, _, _)| prefix.len())
        .map(|(_, app, kind)| MicUser { app: (*app).to_string(), kind: *kind })
}

/// One entry per app, call apps first.
pub fn dedupe(mut users: Vec<MicUser>) -> Vec<MicUser> {
    users.sort_by(|a, b| (a.kind != AppKind::Call, &a.app).cmp(&(b.kind != AppKind::Call, &b.app)));
    users.dedup_by(|a, b| a.app == b.app);
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
