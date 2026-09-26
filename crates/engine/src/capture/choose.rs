//! Which speaker to record on Windows, decided once a second from the active
//! audio sessions. Kept apart from the WASAPI code so it can be tested anywhere.
//!
//! Rules:
//! - Only the most relevant kind of app counts: a call app beats a browser,
//!   which beats anything else. With nothing playing, the default speaker.
//! - Sticky: while the current speaker still has a session of that kind, stay,
//!   even through long pauses. Leave it only when another speaker is clearly
//!   playing (3 checks in a row) while the current one is silent.
//! - A new speaker is taken once it has been chosen on 2 checks in a row, so a
//!   one-off notification sound elsewhere never moves the capture.

/// One active playback session.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub device_id: String,
    pub device_name: String,
    /// 0 call app, 1 browser, 2 anything else.
    pub priority: u8,
    pub app: Option<String>,
    pub peak: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub device_id: String,
    pub device_name: String,
    pub app: Option<String>,
}

impl Target {
    pub fn label(&self) -> String {
        match &self.app {
            Some(app) => format!("{app} — {}", self.device_name),
            None => self.device_name.clone(),
        }
    }
}

const PLAYING: f32 = 0.02;
const SILENT: f32 = 0.005;
const LOUDER_CHECKS: u8 = 3;
const CONFIRM_CHECKS: u8 = 2;

#[derive(Default)]
pub struct Chooser {
    current: Option<Target>,
    pending: Option<(String, u8)>,
    louder_elsewhere: Option<(String, u8)>,
}

impl Chooser {
    /// Forget the current speaker (its capture failed): the next decision
    /// takes whatever is wanted at once.
    pub fn reset(&mut self) {
        self.current = None;
        self.pending = None;
        self.louder_elsewhere = None;
    }

    /// Decide for this check. Returns the target to record and whether the
    /// capture must be (re)opened on it.
    pub fn decide(&mut self, candidates: &[Candidate], default: &Target) -> (Target, bool) {
        let wanted = self.wanted(candidates, default);
        let Some(cur) = &self.current else {
            self.current = Some(wanted.clone());
            self.pending = None;
            return (wanted, true);
        };
        if cur.device_id == wanted.device_id {
            self.pending = None;
            // Same speaker; the app label may have changed (e.g. the call moved to a browser).
            let t = Target { app: wanted.app.or_else(|| cur.app.clone()), ..cur.clone() };
            self.current = Some(t.clone());
            return (t, false);
        }
        let n = match &mut self.pending {
            Some((id, n)) if *id == wanted.device_id => {
                *n += 1;
                *n
            }
            _ => {
                self.pending = Some((wanted.device_id.clone(), 1));
                1
            }
        };
        if n >= CONFIRM_CHECKS {
            self.pending = None;
            self.louder_elsewhere = None;
            self.current = Some(wanted.clone());
            return (wanted, true);
        }
        (cur.clone(), false)
    }

    fn wanted(&mut self, candidates: &[Candidate], default: &Target) -> Target {
        let Some(best) = candidates.iter().map(|c| c.priority).min() else {
            self.louder_elsewhere = None;
            return default.clone();
        };
        let top: Vec<&Candidate> = candidates.iter().filter(|c| c.priority == best).collect();
        let loudest = |pred: &dyn Fn(&Candidate) -> bool| {
            top.iter()
                .copied()
                .filter(|c| pred(c))
                .fold(None::<&Candidate>, |acc, c| match acc {
                    Some(a) if a.peak >= c.peak => Some(a),
                    _ => Some(c),
                })
        };
        let to_target = |c: &Candidate| Target { device_id: c.device_id.clone(), device_name: c.device_name.clone(), app: c.app.clone() };

        if let Some(cur) = &self.current {
            if let Some(here) = loudest(&|c: &Candidate| c.device_id == cur.device_id) {
                // Stay, unless another speaker keeps playing while this one is silent.
                let elsewhere = loudest(&|c: &Candidate| c.device_id != cur.device_id);
                match elsewhere {
                    Some(other) if other.peak > PLAYING && here.peak < SILENT => {
                        let n = match &mut self.louder_elsewhere {
                            Some((id, n)) if *id == other.device_id => {
                                *n += 1;
                                *n
                            }
                            _ => {
                                self.louder_elsewhere = Some((other.device_id.clone(), 1));
                                1
                            }
                        };
                        if n >= LOUDER_CHECKS {
                            return to_target(other);
                        }
                    }
                    _ => self.louder_elsewhere = None,
                }
                return to_target(here);
            }
        }
        self.louder_elsewhere = None;
        to_target(loudest(&|_: &Candidate| true).expect("top is not empty"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(dev: &str, prio: u8, app: &str, peak: f32) -> Candidate {
        Candidate { device_id: dev.into(), device_name: dev.into(), priority: prio, app: Some(app.into()), peak }
    }

    fn default() -> Target {
        Target { device_id: "speakers".into(), device_name: "speakers".into(), app: None }
    }

    #[test]
    fn starts_on_the_call_app_speaker() {
        let mut ch = Chooser::default();
        let (t, open) = ch.decide(&[c("monitor", 2, "spotify", 0.8), c("speakers", 0, "Teams", 0.0)], &default());
        assert_eq!((t.device_id.as_str(), open), ("speakers", true));
        assert_eq!(t.label(), "Teams — speakers");
    }

    #[test]
    fn nothing_playing_means_default_speaker() {
        let mut ch = Chooser::default();
        let (t, open) = ch.decide(&[], &default());
        assert_eq!((t.device_id.as_str(), t.app, open), ("speakers", None, true));
    }

    #[test]
    fn stays_through_pauses_even_with_a_quiet_session_elsewhere() {
        let mut ch = Chooser::default();
        ch.decide(&[c("airpods", 0, "Teams", 0.5)], &default());
        // Teams also has a silent session on the speakers; long pause in the call.
        for _ in 0..10 {
            let (t, open) = ch.decide(&[c("airpods", 0, "Teams", 0.0), c("speakers", 0, "Teams", 0.0)], &default());
            assert_eq!((t.device_id.as_str(), open), ("airpods", false));
        }
    }

    #[test]
    fn follows_the_call_to_another_speaker() {
        let mut ch = Chooser::default();
        ch.decide(&[c("speakers", 0, "Teams", 0.4)], &default());
        // The headset is plugged in: Teams' session moves there.
        let (_, open) = ch.decide(&[c("headset", 0, "Teams", 0.3)], &default());
        assert!(!open, "needs a second check");
        let (t, open) = ch.decide(&[c("headset", 0, "Teams", 0.3)], &default());
        assert_eq!((t.device_id.as_str(), open), ("headset", true));
    }

    #[test]
    fn leaves_a_silent_speaker_when_another_keeps_playing() {
        let mut ch = Chooser::default();
        ch.decide(&[c("speakers", 0, "Teams", 0.4)], &default());
        let mut opened_at = None;
        for i in 0..6 {
            let (t, open) = ch.decide(&[c("speakers", 0, "Teams", 0.0), c("headset", 0, "Teams", 0.3)], &default());
            if open {
                assert_eq!(t.device_id, "headset");
                opened_at = Some(i);
                break;
            }
        }
        // 3 checks to decide it's elsewhere, then 2 to confirm.
        assert_eq!(opened_at, Some(3));
    }

    #[test]
    fn a_notification_elsewhere_does_not_move_it() {
        let mut ch = Chooser::default();
        ch.decide(&[c("speakers", 2, "spotify", 0.5)], &default());
        let (t, open) = ch.decide(&[c("speakers", 2, "spotify", 0.5), c("monitor", 1, "Chrome", 0.9)], &default());
        assert_eq!((t.device_id.as_str(), open), ("speakers", false));
        let (t, _) = ch.decide(&[c("speakers", 2, "spotify", 0.5)], &default());
        assert_eq!(t.device_id, "speakers");
    }
}
