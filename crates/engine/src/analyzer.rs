//! Live levels for the meters, the "other side is silent" warning, and the
//! health report written next to every recording.

use std::sync::Arc;

use serde::Serialize;

use crate::recorder::{Events, RecorderEvent};
use crate::source::{Source, SourceStats};

/// A 10 ms block of the microphone this loud counts as someone talking.
const MIC_SPEECH_DB: f32 = -45.0;
/// A 10 ms block of the call this loud counts as the other side being heard.
const OTHER_ACTIVE_DB: f32 = -55.0;
/// Of the 100 blocks in a second, this many active ones make an active second.
const ACTIVE_BLOCKS: u32 = 20;
/// Warn after this many seconds of the room talking while the call is silent.
const WARN_AFTER_S: u32 = 120;
/// Seconds of the call heard (within the last ten) that clear the warning;
/// one system beep is only one.
const HEARD_AGAIN_S: u32 = 2;
const HEALTH_EVERY_S: u32 = 10;

#[derive(Clone, Debug, Serialize)]
pub struct HealthPoint {
    /// Seconds from the start of the recording.
    pub t: u32,
    pub mic_db: f32,
    pub other_db: Option<f32>,
    pub mic_talking_s: u32,
    pub other_heard_s: u32,
    pub hearing: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceSummary {
    pub label: String,
    pub active_s: u32,
    pub loudest_db: f32,
    pub stats: SourceStats,
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthReport {
    pub duration_s: u32,
    /// Both sides were heard as expected.
    pub ok: bool,
    pub verdict: String,
    pub mic: SourceSummary,
    pub other: Option<SourceSummary>,
    /// Longest stretch (seconds of room talking) with the call silent.
    pub longest_silent_while_talking_s: u32,
    pub warned: bool,
    pub timeline: Vec<HealthPoint>,
    pub events: Vec<String>,
}

pub struct Analyzer {
    events: Events,
    mic: Arc<Source>,
    other: Option<Arc<Source>>,
    blocks: u64,
    // meters (every 10 blocks = 100 ms)
    meter: [(f32, f64); 2],
    meter_n: u32,
    // this second
    sec_active: [u32; 2],
    sec_n: u32,
    // warning
    recent_heard: Vec<bool>,
    silent_talking_s: u32,
    longest_silent_talking_s: u32,
    warned: bool,
    // health window
    win_sq: [f64; 2],
    win_n: u64,
    win_active_s: [u32; 2],
    seconds: u32,
    totals_active_s: [u32; 2],
    loudest_db: [f32; 2],
    timeline: Vec<HealthPoint>,
    notes: Vec<String>,
}

fn db(sq_mean: f64) -> f32 {
    (10.0 * sq_mean.max(1e-12).log10()) as f32
}

impl Analyzer {
    pub fn new(events: Events, mic: Arc<Source>, other: Option<Arc<Source>>) -> Self {
        Analyzer {
            events,
            mic,
            other,
            blocks: 0,
            meter: [(0.0, 0.0); 2],
            meter_n: 0,
            sec_active: [0; 2],
            sec_n: 0,
            recent_heard: Vec::new(),
            silent_talking_s: 0,
            longest_silent_talking_s: 0,
            warned: false,
            win_sq: [0.0; 2],
            win_n: 0,
            win_active_s: [0; 2],
            seconds: 0,
            totals_active_s: [0; 2],
            loudest_db: [-120.0; 2],
            timeline: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn note(&mut self, text: String) {
        let t = self.seconds;
        self.notes.push(format!("{:02}:{:02} {text}", t / 60, t % 60));
    }

    /// One 10 ms block of each channel.
    pub fn block(&mut self, mic: &[f32], other: Option<&[f32]>) {
        self.blocks += 1;
        let chans = [Some(mic), other];
        for (c, ch) in chans.iter().enumerate() {
            let Some(x) = ch else { continue };
            let (mut peak, mut sq) = (0f32, 0f64);
            for s in x.iter() {
                peak = peak.max(s.abs());
                sq += f64::from(*s) * f64::from(*s);
            }
            let block_db = db(sq / x.len().max(1) as f64);
            self.meter[c].0 = self.meter[c].0.max(peak);
            self.meter[c].1 += sq;
            self.win_sq[c] += sq;
            let threshold = if c == 0 { MIC_SPEECH_DB } else { OTHER_ACTIVE_DB };
            if block_db > threshold {
                self.sec_active[c] += 1;
            }
            self.loudest_db[c] = self.loudest_db[c].max(block_db);
        }
        self.win_n += mic.len() as u64;
        self.meter_n += 1;
        self.sec_n += 1;

        if self.meter_n == 10 {
            let n = (mic.len() * 10) as f64;
            (self.events)(RecorderEvent::Levels {
                mic_peak: self.meter[0].0,
                mic_db: db(self.meter[0].1 / n),
                other_peak: other.map(|_| self.meter[1].0),
                other_db: other.map(|_| db(self.meter[1].1 / n)),
            });
            self.meter = [(0.0, 0.0); 2];
            self.meter_n = 0;
        }
        if self.sec_n == 100 {
            self.end_second(other.is_some());
        }
    }

    fn end_second(&mut self, has_other: bool) {
        let talking = self.sec_active[0] >= ACTIVE_BLOCKS;
        let heard = self.sec_active[1] >= ACTIVE_BLOCKS;
        self.sec_active = [0; 2];
        self.sec_n = 0;
        self.seconds += 1;
        for (c, active) in [talking, heard].into_iter().enumerate() {
            if active {
                self.win_active_s[c] += 1;
                self.totals_active_s[c] += 1;
            }
        }

        if has_other {
            self.recent_heard.push(heard);
            if self.recent_heard.len() > 10 {
                self.recent_heard.remove(0);
            }
            let heard_recently = self.recent_heard.iter().filter(|h| **h).count() as u32 >= HEARD_AGAIN_S;
            if heard_recently {
                self.silent_talking_s = 0;
                if self.warned {
                    self.warned = false;
                    (self.events)(RecorderEvent::OtherSideSilent { silent: false, seconds: 0 });
                    self.note("the other side is heard again".into());
                }
            } else if talking {
                self.silent_talking_s += 1;
                self.longest_silent_talking_s = self.longest_silent_talking_s.max(self.silent_talking_s);
                if self.silent_talking_s >= WARN_AFTER_S && !self.warned {
                    self.warned = true;
                    (self.events)(RecorderEvent::OtherSideSilent { silent: true, seconds: self.silent_talking_s });
                    self.note("warning: the other side has been silent while the room talks".into());
                }
            }
        }

        if self.seconds.is_multiple_of(HEALTH_EVERY_S) {
            self.health_point(has_other);
        }
    }

    fn health_point(&mut self, has_other: bool) {
        let n = self.win_n.max(1) as f64;
        self.timeline.push(HealthPoint {
            t: self.seconds,
            mic_db: db(self.win_sq[0] / n),
            other_db: has_other.then(|| db(self.win_sq[1] / n)),
            mic_talking_s: self.win_active_s[0],
            other_heard_s: self.win_active_s[1],
            hearing: self.other.as_ref().map(|o| o.label()).unwrap_or_default(),
        });
        self.win_sq = [0.0; 2];
        self.win_n = 0;
        self.win_active_s = [0; 2];
    }

    pub fn report(mut self) -> HealthReport {
        let has_other = self.other.is_some();
        if self.win_n > 0 {
            self.health_point(has_other);
        }
        let duration_s = (self.blocks / 100) as u32;
        let mic = SourceSummary {
            label: self.mic.label(),
            active_s: self.totals_active_s[0],
            loudest_db: self.loudest_db[0],
            stats: self.mic.stats(),
        };
        let other = self.other.as_ref().map(|o| SourceSummary {
            label: o.label(),
            active_s: self.totals_active_s[1],
            loudest_db: self.loudest_db[1],
            stats: o.stats(),
        });
        let mic_silent = mic.active_s == 0 && duration_s >= 30;
        let (ok, verdict) = match &other {
            _ if mic_silent && other.as_ref().is_none_or(|o| o.active_s == 0) => {
                (false, "Nothing was heard on either side. Check the microphone and that the call played on this computer.".to_string())
            }
            _ if mic_silent => (false, "Your microphone didn't pick up anyone talking. Check the microphone (and its permission).".to_string()),
            None => (false, "Microphone only: the other side of the call could not be recorded.".to_string()),
            Some(o) if o.active_s == 0 && mic.active_s >= 30 => {
                (false, "The other side was never heard while the room talked. Check which speaker the call played on.".to_string())
            }
            Some(_) if self.longest_silent_talking_s >= WARN_AFTER_S => (
                false,
                format!(
                    "Both sides recorded, but the other side was silent for {} min at one point while the room talked.",
                    self.longest_silent_talking_s / 60
                ),
            ),
            Some(_) => (true, "Both sides recorded.".to_string()),
        };
        HealthReport {
            duration_s,
            ok,
            verdict,
            mic,
            other,
            longest_silent_while_talking_s: self.longest_silent_talking_s,
            warned: self.warned,
            timeline: self.timeline,
            events: self.notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn run(seconds: u32, mic_loud: bool, other_loud: impl Fn(u32) -> bool) -> (HealthReport, Vec<RecorderEvent>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let events: Events = Arc::new(move |e| {
            if !matches!(e, RecorderEvent::Levels { .. }) {
                s2.lock().unwrap().push(e);
            }
        });
        let mut a = Analyzer::new(events, Arc::new(Source::default()), Some(Arc::new(Source::default())));
        let loud = vec![0.2f32; 480];
        let quiet = vec![0.0f32; 480];
        for s in 0..seconds {
            for _ in 0..100 {
                a.block(if mic_loud { &loud } else { &quiet }, Some(if other_loud(s) { &loud } else { &quiet }));
            }
        }
        let evs = seen.lock().unwrap().drain(..).collect();
        (a.report(), evs)
    }

    #[test]
    fn warns_after_two_minutes_of_silent_call_while_talking() {
        let (r, evs) = run(150, true, |_| false);
        assert!(r.warned);
        assert!(evs.iter().any(|e| matches!(e, RecorderEvent::OtherSideSilent { silent: true, .. })));
        assert!(r.verdict.contains("never heard"), "{}", r.verdict);
        assert_eq!(r.timeline.len(), 15);
    }

    #[test]
    fn one_beep_does_not_count_as_heard() {
        // The call is heard for one second at 100 s only.
        let (r, _) = run(150, true, |s| s == 100);
        assert!(r.warned);
        assert_eq!(r.longest_silent_while_talking_s, 150);
    }

    #[test]
    fn a_silent_microphone_is_reported() {
        let (r, _) = run(60, false, |_| true);
        assert!(!r.ok);
        assert!(r.verdict.contains("microphone"), "{}", r.verdict);
    }

    #[test]
    fn a_normal_call_is_fine() {
        let (r, evs) = run(200, true, |s| s % 3 != 0);
        assert!(!r.warned);
        assert!(evs.is_empty());
        assert_eq!(r.verdict, "Both sides recorded.");
        assert!(r.ok);
    }
}
