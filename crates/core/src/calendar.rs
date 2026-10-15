//! The user's upcoming calendar meetings, as notizli.ch gives them to the
//! recorder: titles and times only, never who is invited. The app keeps them
//! in memory only (docs/superpowers/specs/2026-10-14-calendar-design.md).

use std::collections::HashSet;

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime, UtcOffset};

/// Longer titles are cut so the "Record it?" box keeps its buttons.
const MAX_TITLE: usize = 48;

/// How long after its start an in-person meeting is still asked about (a
/// laptop that wakes later doesn't ask for a meeting long under way).
pub const ASK_WITHIN: Duration = Duration::minutes(2);

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Upcoming {
    /// The calendar is offered to this account.
    #[serde(default)]
    pub available: bool,
    /// At least one calendar is connected.
    #[serde(default)]
    pub connected: bool,
    /// The meeting a recording started now belongs to (the server's own rule);
    /// None when no meeting fits, two fit, or a calendar couldn't be read.
    #[serde(default)]
    pub now: Option<String>,
    /// In progress or starting within 3 hours, by start.
    #[serde(default)]
    pub meetings: Vec<Meeting>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Meeting {
    /// "g:…" / "m:…", sent back as `calendar_event_id` on upload.
    pub id: String,
    pub title: String,
    /// ISO 8601, UTC.
    pub start: String,
    pub end: String,
    /// teams, zoom, meet, webex, other — or None: no video link.
    #[serde(rename = "joinType", default)]
    pub join_type: Option<String>,
}

fn parse(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

impl Meeting {
    pub fn start_at(&self) -> Option<OffsetDateTime> {
        parse(&self.start)
    }

    /// No video link: in person or by phone, so call detection can't notice it.
    pub fn in_person(&self) -> bool {
        self.join_type.is_none()
    }

    /// "Weekly sync · 14:00 · Teams", the time in `offset` (the computer's zone).
    pub fn line(&self, offset: UtcOffset) -> String {
        let t = self.title.trim();
        let title = if t.is_empty() || t == "(no title)" {
            "Untitled meeting".to_string()
        } else if t.chars().count() > MAX_TITLE {
            format!("{}…", t.chars().take(MAX_TITLE - 1).collect::<String>())
        } else {
            t.to_string()
        };
        let join = match self.join_type.as_deref() {
            Some("teams") => "Teams",
            Some("zoom") => "Zoom",
            Some("meet") => "Google Meet",
            Some("webex") => "Webex",
            Some(_) => "Online",
            None => "In person or by phone",
        };
        match self.start_at() {
            Some(s) => {
                let s = s.to_offset(offset);
                format!("{title} · {:02}:{:02} · {join}", s.hour(), s.minute())
            }
            None => format!("{title} · {join}"),
        }
    }
}

impl Upcoming {
    pub fn meeting(&self, id: &str) -> Option<&Meeting> {
        self.meetings.iter().find(|m| m.id == id)
    }

    /// The meeting a recording started now belongs to, when the server is sure.
    pub fn now_meeting(&self) -> Option<&Meeting> {
        self.now.as_deref().and_then(|id| self.meeting(id))
    }

    /// The soonest meeting that hasn't started at `at`, other than the one on now.
    pub fn next_meeting(&self, at: OffsetDateTime) -> Option<&Meeting> {
        self.meetings
            .iter()
            .filter(|m| Some(m.id.as_str()) != self.now.as_deref())
            .filter_map(|m| Some((m.start_at()?, m)))
            .filter(|(s, _)| *s > at)
            .min_by_key(|(s, _)| *s)
            .map(|(_, m)| m)
    }

    /// A meeting with no video link that started less than [`ASK_WITHIN`]
    /// before `at` and wasn't asked about yet.
    pub fn in_person_due(&self, at: OffsetDateTime, asked: &HashSet<String>) -> Option<&Meeting> {
        self.meetings.iter().find(|m| {
            m.in_person() && !asked.contains(&m.id) && m.start_at().is_some_and(|s| s <= at && at - s < ASK_WITHIN)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::offset;

    fn meeting(title: &str, start: &str, join: Option<&str>) -> Meeting {
        Meeting {
            id: format!("g:{title}"),
            title: title.into(),
            start: start.into(),
            end: "2026-10-14T23:00:00Z".into(),
            join_type: join.map(str::to_string),
        }
    }

    #[test]
    fn reads_the_server_answer() {
        let u: Upcoming = serde_json::from_str(
            r#"{"available":true,"connected":true,"providers":[{"provider":"google","status":"ok"}],"now":"g:1",
                "meetings":[{"id":"g:1","provider":"google","title":"Weekly sync","start":"2026-10-14T12:00:00.000Z","end":"2026-10-14T12:30:00.000Z","joinType":"teams"}]}"#,
        )
        .unwrap();
        assert!(u.available && u.connected);
        let m = u.now_meeting().unwrap();
        assert_eq!(m.title, "Weekly sync");
        assert_eq!(m.join_type.as_deref(), Some("teams"));
        assert!(!m.in_person());
    }

    #[test]
    fn an_empty_answer_means_no_calendar() {
        let u: Upcoming = serde_json::from_str("{}").unwrap();
        assert_eq!(u, Upcoming::default());
        assert!(u.now_meeting().is_none());
        let u: Upcoming = serde_json::from_str(r#"{"now":"g:gone","meetings":[]}"#).unwrap();
        assert!(u.now_meeting().is_none());
    }

    #[test]
    fn the_line_shows_title_local_time_and_how_to_join() {
        let at = "2026-10-14T12:00:00.000Z";
        assert_eq!(meeting("Weekly sync", at, Some("teams")).line(offset!(+2)), "Weekly sync · 14:00 · Teams");
        assert_eq!(meeting("Weekly sync", at, None).line(offset!(+2)), "Weekly sync · 14:00 · In person or by phone");
        assert_eq!(meeting("X", at, Some("meet")).line(offset!(UTC)), "X · 12:00 · Google Meet");
        assert_eq!(meeting("X", at, Some("webex")).line(offset!(UTC)), "X · 12:00 · Webex");
        assert_eq!(meeting("X", at, Some("other")).line(offset!(UTC)), "X · 12:00 · Online");
        assert_eq!(meeting("(no title)", at, Some("zoom")).line(offset!(+1)), "Untitled meeting · 13:00 · Zoom");
        assert_eq!(meeting("X", "not a time", None).line(offset!(UTC)), "X · In person or by phone");
    }

    use std::collections::HashSet;

    fn at(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    fn list(now: Option<&str>, meetings: Vec<Meeting>) -> Upcoming {
        Upcoming { available: true, connected: true, now: now.map(str::to_string), meetings }
    }

    #[test]
    fn next_meeting_is_the_soonest_not_yet_started_one_other_than_now() {
        let u = list(
            Some("g:A"),
            vec![
                meeting("A", "2026-10-14T12:00:00Z", Some("teams")),
                meeting("B", "2026-10-14T15:00:00Z", None),
                meeting("C", "2026-10-14T13:00:00Z", Some("zoom")),
            ],
        );
        assert_eq!(u.next_meeting(at("2026-10-14T12:05:00Z")).unwrap().title, "C");
        assert_eq!(u.next_meeting(at("2026-10-14T13:30:00Z")).unwrap().title, "B");
        assert!(u.next_meeting(at("2026-10-14T16:00:00Z")).is_none());
    }

    #[test]
    fn an_in_person_meeting_is_due_in_its_first_two_minutes_once() {
        let u = list(
            None,
            vec![meeting("Call", "2026-10-14T12:00:00Z", Some("teams")), meeting("Visit", "2026-10-14T12:00:00Z", None)],
        );
        let mut asked = HashSet::new();
        assert!(u.in_person_due(at("2026-10-14T11:59:59Z"), &asked).is_none());
        assert_eq!(u.in_person_due(at("2026-10-14T12:00:00Z"), &asked).unwrap().title, "Visit");
        assert_eq!(u.in_person_due(at("2026-10-14T12:01:59Z"), &asked).unwrap().title, "Visit");
        assert!(u.in_person_due(at("2026-10-14T12:02:00Z"), &asked).is_none());
        asked.insert("g:Visit".to_string());
        assert!(u.in_person_due(at("2026-10-14T12:00:30Z"), &asked).is_none());
    }

    #[test]
    fn a_long_title_is_cut_so_the_box_keeps_its_buttons() {
        let long = "a".repeat(60);
        let line = meeting(&long, "2026-10-14T12:00:00Z", None).line(offset!(UTC));
        assert!(line.starts_with(&format!("{}… · 12:00", "a".repeat(47))), "{line}");
    }
}
