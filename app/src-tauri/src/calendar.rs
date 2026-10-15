//! The user's upcoming meetings from notizli.ch, in memory only: they name
//! the meeting in the "Record it?" box, the window and the menu bar, and tell
//! the server which meeting a recording is for. Meeting titles never go to
//! the log.
//! Design: docs/superpowers/specs/2026-10-14-calendar-design.md.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use notizli_core::calendar::{Meeting, Upcoming};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use time::{OffsetDateTime, UtcOffset};

use crate::state::AppState;

/// A list younger than this is used as is (notizli.ch caches 2 minutes too).
pub const FRESH: Duration = Duration::from_secs(120);
/// While paired, the list is fetched again when older than this.
const EVERY: Duration = Duration::from_secs(5 * 60);
/// How often the loop looks at the list (the in-person box needs ~15 s).
const TICK: Duration = Duration::from_secs(15);

#[derive(Default)]
pub struct CalendarState {
    /// When the list was last asked for, and the answer (None: it failed).
    latest: Mutex<Option<(Instant, Option<Upcoming>)>>,
}

impl CalendarState {
    pub fn cached(&self) -> Option<Upcoming> {
        self.latest.lock().unwrap().as_ref().and_then(|(_, u)| u.clone())
    }

    pub fn forget(&self) {
        *self.latest.lock().unwrap() = None;
    }

    fn keep(&self, u: &Upcoming) {
        *self.latest.lock().unwrap() = Some((Instant::now(), Some(u.clone())));
    }

    /// notizli.ch didn't answer: no calendar until the next try.
    fn failed(&self) {
        *self.latest.lock().unwrap() = Some((Instant::now(), None));
    }

    /// The last answer (None: it failed) when asked for within `max_age`;
    /// None: ask again.
    fn fresh(&self, max_age: Duration) -> Option<Option<Upcoming>> {
        self.latest.lock().unwrap().as_ref().filter(|(at, _)| at.elapsed() < max_age).map(|(_, u)| u.clone())
    }
}

/// The meetings, fetched again when the list is older than `max_age`. None
/// when unpaired or notizli.ch didn't answer: the app then behaves as before.
/// A failed fetch isn't repeated within `max_age` either.
pub async fn upcoming(app: &AppHandle, max_age: Duration) -> Option<Upcoming> {
    let state = app.state::<AppState>();
    if let Some(u) = fake() {
        state.calendar.keep(&u);
        return Some(u);
    }
    if let Some(u) = state.calendar.fresh(max_age) {
        return u;
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
            state.calendar.failed();
            None
        }
    }
}

/// Keeps the list fresh for the window and the menu bar.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last: Option<CalendarView> = None;
        loop {
            tokio::time::sleep(TICK).await;
            if app.state::<AppState>().token().is_none() && fake().is_none() {
                continue;
            }
            upcoming(&app, EVERY).await;
            // The view, not the list: "Next" also changes when a meeting starts.
            let v = view(&app);
            if v != last {
                last = v;
                crate::commands::changed(&app);
            }
        }
    });
}

/// "Weekly sync · 14:00 · Teams" in this computer's time zone.
pub fn line(m: &Meeting) -> String {
    m.line(local_offset())
}

fn local_offset() -> UtcOffset {
    UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct NowView {
    pub id: String,
    pub line: String,
}

/// What the idle window and the menu bar show.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CalendarView {
    /// The meeting on now, offered as "This recording is for: …".
    pub now: Option<NowView>,
    /// The next meeting's line, information only (when none is on now).
    pub next: Option<String>,
    /// Offered but not connected: suggest connecting.
    pub connect: bool,
}

/// None: nothing calendar-related to show (not offered, or no answer).
pub fn view(app: &AppHandle) -> Option<CalendarView> {
    view_of(&app.state::<AppState>().calendar.cached()?, OffsetDateTime::now_utc(), local_offset())
}

fn view_of(u: &Upcoming, at: OffsetDateTime, offset: UtcOffset) -> Option<CalendarView> {
    if !u.available {
        return None;
    }
    let now = u.now_meeting().map(|m| NowView { id: m.id.clone(), line: m.line(offset) });
    let next = if now.is_none() { u.next_meeting(at).map(|m| m.line(offset)) } else { None };
    Some(CalendarView { now, next, connect: !u.connected })
}

/// The menu bar's greyed-out first line: "Now: …" or "Next: …".
pub fn menu_line(v: &CalendarView) -> Option<String> {
    match (&v.now, &v.next) {
        (Some(n), _) => Some(format!("Now: {}", n.line)),
        (None, Some(l)) => Some(format!("Next: {l}")),
        (None, None) => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use notizli_core::calendar::Meeting;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    fn meeting(title: &str, start: &str, join: Option<&str>) -> Meeting {
        Meeting {
            id: format!("g:{title}"),
            title: title.into(),
            start: start.into(),
            end: "2026-10-14T23:00:00Z".into(),
            join_type: join.map(str::to_string),
        }
    }

    fn at(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    fn list(now: Option<&str>) -> Upcoming {
        Upcoming {
            available: true,
            connected: true,
            now: now.map(str::to_string),
            meetings: vec![meeting("A", "2026-10-14T12:00:00Z", Some("teams")), meeting("C", "2026-10-14T13:00:00Z", Some("zoom"))],
        }
    }

    #[test]
    fn the_window_offers_the_meeting_on_now_else_the_next_one() {
        let v = view_of(&list(Some("g:A")), at("2026-10-14T12:05:00Z"), UtcOffset::UTC).unwrap();
        assert_eq!(v.now, Some(NowView { id: "g:A".into(), line: "A · 12:00 · Teams".into() }));
        assert_eq!(v.next, None);
        assert!(!v.connect);
        let v = view_of(&list(None), at("2026-10-14T12:05:00Z"), UtcOffset::UTC).unwrap();
        assert_eq!(v.now, None);
        assert_eq!(v.next.as_deref(), Some("C · 13:00 · Zoom"));
        // Later, the next meeting has started: nothing to show any more.
        assert_eq!(view_of(&list(None), at("2026-10-14T13:01:00Z"), UtcOffset::UTC).unwrap().next, None);
    }

    #[test]
    fn a_calendar_not_offered_shows_nothing_and_one_not_connected_suggests_connecting() {
        let t = at("2026-10-14T12:05:00Z");
        assert!(view_of(&Upcoming { available: false, ..list(Some("g:A")) }, t, UtcOffset::UTC).is_none());
        let v = view_of(&Upcoming { available: true, ..Default::default() }, t, UtcOffset::UTC).unwrap();
        assert_eq!(v, CalendarView { now: None, next: None, connect: true });
    }

    #[test]
    fn the_menu_says_now_or_next_or_nothing() {
        let now = CalendarView { now: Some(NowView { id: "g:A".into(), line: "A · 12:00 · Teams".into() }), next: None, connect: false };
        assert_eq!(menu_line(&now).as_deref(), Some("Now: A · 12:00 · Teams"));
        let next = CalendarView { now: None, next: Some("C · 13:00 · Zoom".into()), connect: false };
        assert_eq!(menu_line(&next).as_deref(), Some("Next: C · 13:00 · Zoom"));
        assert_eq!(menu_line(&CalendarView { now: None, next: None, connect: true }), None);
    }

    #[test]
    fn a_failed_fetch_is_not_repeated_until_the_list_would_be_stale() {
        let c = CalendarState::default();
        assert!(c.fresh(FRESH).is_none(), "nothing fetched yet");
        c.failed();
        assert_eq!(c.fresh(FRESH), Some(None), "failed just now: no calendar, no new request");
        assert!(c.fresh(Duration::ZERO).is_none(), "stale: ask again");
        assert!(c.cached().is_none());
        c.keep(&Upcoming { available: true, ..Default::default() });
        assert!(c.fresh(FRESH).unwrap().unwrap().available);
        c.forget();
        assert!(c.fresh(FRESH).is_none());
    }
}
