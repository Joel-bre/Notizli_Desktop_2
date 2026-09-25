//! A capture source's buffer: the capture thread pushes audio at its device's
//! pace, the mixer takes 10 ms at a time at wall-clock pace.
//!
//! The two clocks never agree exactly, and devices deliver in bursts, so the
//! buffer keeps ~60 ms in reserve: it waits for that much before playing
//! (priming), trims 10 ms when it runs more than 150 ms ahead, and re-primes
//! with silence when it runs dry. For speech this costs a 10 ms skip every
//! minute or two at worst, and never lets the channels drift apart.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::resample::Resampler;
use crate::SAMPLE_RATE;

const MS: usize = (SAMPLE_RATE / 1000) as usize;
const PRIME: usize = 60 * MS;
const HIGH: usize = 150 * MS;
const TRIM: usize = 10 * MS;
/// No audio for this long means the device went quiet (a loopback with nothing
/// playing delivers no packets) or went away: output silence and re-prime.
const IDLE_AFTER: Duration = Duration::from_millis(250);
/// Never hold more than this (e.g. while the mixer is stalled).
const MAX_BUFFERED: usize = 3_000 * MS;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct SourceStats {
    /// Times the buffer ran dry while audio was flowing.
    pub underruns: u64,
    /// Milliseconds dropped to keep latency bounded.
    pub dropped_ms: u64,
    /// Times the capture was (re)opened, e.g. after a device change.
    pub opens: u64,
    /// Samples delivered by the device (after resampling to 48 kHz).
    pub samples: u64,
}

pub struct Source {
    inner: Mutex<Inner>,
}

struct Inner {
    buf: VecDeque<f32>,
    resampler: Option<(u32, Resampler)>,
    scratch: Vec<f32>,
    last_push: Option<Instant>,
    primed: bool,
    label: String,
    stats: SourceStats,
}

impl Default for Source {
    fn default() -> Self {
        Source {
            inner: Mutex::new(Inner {
                buf: VecDeque::with_capacity(PRIME * 4),
                resampler: None,
                scratch: Vec::new(),
                last_push: None,
                primed: false,
                label: String::new(),
                stats: SourceStats::default(),
            }),
        }
    }
}

impl Source {
    /// Add mono samples captured at `rate` Hz.
    pub fn push(&self, mono: &[f32], rate: u32) {
        let mut g = self.inner.lock().unwrap();
        let inner = &mut *g;
        if rate == SAMPLE_RATE {
            inner.buf.extend(mono);
            inner.stats.samples += mono.len() as u64;
        } else {
            if inner.resampler.as_ref().map(|(r, _)| *r) != Some(rate) {
                inner.resampler = Some((rate, Resampler::new(rate, SAMPLE_RATE)));
            }
            inner.scratch.clear();
            let (_, r) = inner.resampler.as_mut().unwrap();
            r.process(mono, &mut inner.scratch);
            inner.buf.extend(inner.scratch.iter());
            inner.stats.samples += inner.scratch.len() as u64;
        }
        if inner.buf.len() > MAX_BUFFERED {
            let excess = inner.buf.len() - MAX_BUFFERED;
            inner.buf.drain(..excess);
            inner.stats.dropped_ms += (excess / MS) as u64;
        }
        inner.last_push = Some(Instant::now());
    }

    /// A new device stream was opened: forget the old one's leftovers.
    pub fn opened(&self, label: &str) {
        let mut g = self.inner.lock().unwrap();
        g.buf.clear();
        g.resampler = None;
        g.primed = false;
        g.label = label.to_string();
        g.stats.opens += 1;
    }

    pub fn set_label(&self, label: &str) {
        self.inner.lock().unwrap().label = label.to_string();
    }

    pub fn label(&self) -> String {
        self.inner.lock().unwrap().label.clone()
    }

    pub fn stats(&self) -> SourceStats {
        self.inner.lock().unwrap().stats.clone()
    }

    /// Fill `out` (mono, 48 kHz) for the mixer; silence where nothing is available.
    pub fn take(&self, out: &mut [f32], now: Instant) {
        let mut g = self.inner.lock().unwrap();
        let idle = g.last_push.is_none_or(|t| now.duration_since(t) > IDLE_AFTER);
        if idle {
            g.primed = false;
            if g.buf.len() > PRIME {
                let excess = g.buf.len() - PRIME;
                g.buf.drain(..excess);
            }
            out.fill(0.0);
            return;
        }
        if !g.primed {
            if g.buf.len() < PRIME {
                out.fill(0.0);
                return;
            }
            g.primed = true;
        }
        let n = out.len().min(g.buf.len());
        for (o, s) in out.iter_mut().zip(g.buf.drain(..n)) {
            *o = s;
        }
        if n < out.len() {
            out[n..].fill(0.0);
            g.primed = false;
            g.stats.underruns += 1;
        }
    }

    /// Keep latency bounded when the device runs faster than the wall clock.
    /// Called by the mixer once it has caught up.
    pub fn trim(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.primed && g.buf.len() > HIGH {
            g.buf.drain(..TRIM);
            g.stats.dropped_ms += 10;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primes_then_plays_then_goes_idle() {
        let s = Source::default();
        let t0 = Instant::now();
        let mut out = vec![1.0f32; 480];
        s.take(&mut out, t0);
        assert!(out.iter().all(|x| *x == 0.0), "nothing pushed yet");

        s.push(&vec![0.5; 480], SAMPLE_RATE);
        s.take(&mut out, Instant::now());
        assert!(out.iter().all(|x| *x == 0.0), "still priming");

        s.push(&vec![0.5; PRIME], SAMPLE_RATE);
        s.take(&mut out, Instant::now());
        assert!(out.iter().all(|x| *x == 0.5), "primed and playing");

        let later = Instant::now() + Duration::from_millis(400);
        s.take(&mut out, later);
        assert!(out.iter().all(|x| *x == 0.0), "idle after no pushes");
    }

    #[test]
    fn trims_when_running_ahead() {
        let s = Source::default();
        s.push(&vec![0.1; HIGH + 2 * TRIM], SAMPLE_RATE);
        let mut out = vec![0.0f32; 480];
        s.take(&mut out, Instant::now());
        s.trim();
        assert_eq!(s.stats().dropped_ms, 10);
    }

    #[test]
    fn resamples_other_rates() {
        let s = Source::default();
        s.push(&vec![0.2; 16_000], 16_000);
        let n = s.stats().samples;
        assert!((47_990..=48_000).contains(&n), "{n}");
    }
}
