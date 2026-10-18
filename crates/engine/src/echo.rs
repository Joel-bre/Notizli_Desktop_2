//! Removes the call's sound from the microphone (acoustic echo cancellation).
//!
//! On laptop speakers the microphone hears the call (or a video) about as
//! loud as the call channel itself, which hides who is on which side. The call
//! channel is the exact sound the computer played, so it is the reference:
//! WebRTC's AEC3 (crate `aec3`, pure Rust) subtracts its echo from the mic,
//! every 10 ms, echo removal only (no noise suppression, no gain control).
//!
//! The suppressor is gentler than WebRTC's default: tested offline on a real
//! laptop-speaker recording (2026-10-18), the default chopped the user's voice
//! whenever both sides talked at once. This tuning still removed 27-44 dB of
//! echo while only the call played and kept the voice alone within ~1 dB.
//! `aec3`'s API is still changing; the version is pinned and this file is the
//! only place that uses it.

use aec3::api::config::{EchoCanceller3Config, MaskingThresholds};
use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};
use serde::Serialize;

use crate::SAMPLE_RATE;

/// Below this (mean square, about -60 dBFS) the call counts as silent.
const CALL_ACTIVE: f64 = 1e-6;

/// Echo cancellation in the health report: numbers only, no audio.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct EchoSummary {
    pub on: bool,
    /// How much quieter the microphone was after cancellation while the call
    /// played (includes the user's own voice, so it understates the echo).
    pub removed_db: Option<f32>,
}

pub struct EchoCanceller {
    pipeline: LinearPipeline,
    out: Vec<f32>,
    /// Mic energy before and after, over blocks where the call played.
    before: f64,
    after: f64,
    failed: bool,
}

fn gentle_config() -> EchoCanceller3Config {
    let mut cfg = EchoCanceller3Config::default();
    let sup = &mut cfg.suppressor;
    // Suppress only when the residual echo clearly dominates, and treat the
    // user as the dominant talker sooner.
    sup.normal_tuning.mask_lf = MaskingThresholds::new(0.6, 0.8, 0.3);
    sup.normal_tuning.mask_hf = MaskingThresholds::new(0.3, 0.5, 0.3);
    sup.dominant_nearend_detection.enr_threshold = 1.0;
    sup.dominant_nearend_detection.trigger_threshold = 6;
    cfg
}

impl EchoCanceller {
    /// None if the canceller can't be set up; the recording then keeps the raw mic.
    pub fn new() -> Option<EchoCanceller> {
        let format = AudioFormat::ten_ms(SAMPLE_RATE, 1);
        match linear::builder(format, format)
            .aec3_config(gentle_config())
            .enable_noise_suppression(false)
            .enable_gain_controller2(false)
            .build()
        {
            Ok(pipeline) => Some(EchoCanceller {
                pipeline,
                out: vec![0.0; format.sample_count()],
                before: 0.0,
                after: 0.0,
                failed: false,
            }),
            Err(e) => {
                log::warn!("echo cancellation unavailable: {e:?}");
                None
            }
        }
    }

    /// Clean one 10 ms block of `mic` in place, with `call` (same length) as the
    /// reference. Any failure leaves the mic as it was, for the rest of the recording.
    pub fn process(&mut self, mic: &mut [f32], call: &[f32]) {
        if self.failed || mic.len() != self.out.len() || call.len() != mic.len() {
            return;
        }
        let call_active = call.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / call.len() as f64 > CALL_ACTIVE;
        let produced = self
            .pipeline
            .handle_render_frame(call)
            .and_then(|_| self.pipeline.process_capture_frame(mic, &mut self.out));
        match produced {
            Ok(true) => {
                if call_active {
                    self.before += mic.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                    self.after += self.out.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                }
                mic.copy_from_slice(&self.out);
            }
            Ok(false) => {}
            Err(e) => {
                log::warn!("echo cancellation stopped, keeping the raw microphone: {e:?}");
                self.failed = true;
            }
        }
    }

    pub fn summary(&self) -> EchoSummary {
        let removed_db = (self.before > 0.0 && self.after > 0.0).then(|| (10.0 * (self.before / self.after).log10()) as f32);
        EchoSummary { on: true, removed_db }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SAMPLE_RATE;

    const STEP: usize = (SAMPLE_RATE / 100) as usize;

    /// Deterministic white noise in [-amp, amp].
    fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn energy(v: &[f32]) -> f64 {
        v.iter().map(|s| f64::from(*s) * f64::from(*s)).sum()
    }

    /// 0-10 s: only the call plays and reaches the mic 40 ms later at 60 %.
    /// 10-20 s: only the user talks.
    fn run(ec: &mut EchoCanceller) -> (Vec<f32>, Vec<f32>) {
        let n = SAMPLE_RATE as usize * 20;
        let half = n / 2;
        let delay = SAMPLE_RATE as usize / 25;
        let mut call = noise(1, n, 0.2);
        call[half..].iter_mut().for_each(|s| *s = 0.0);
        let voice = noise(7, n, 0.2);
        let mic: Vec<f32> = (0..n)
            .map(|i| {
                let echo = if i >= delay { 0.6 * call[i - delay] } else { 0.0 };
                echo + if i >= half { voice[i] } else { 0.0 }
            })
            .collect();
        let mut out = mic.clone();
        for (m, c) in out.chunks_mut(STEP).zip(call.chunks(STEP)) {
            ec.process(m, c);
        }
        (mic, out)
    }

    #[test]
    fn removes_the_call_from_the_mic_and_keeps_the_voice() {
        let mut ec = EchoCanceller::new().expect("echo canceller");
        let (mic, out) = run(&mut ec);
        let s = SAMPLE_RATE as usize;
        // Once it has adapted (5-10 s), the echo is at least 20 dB quieter.
        let removed = 10.0 * (energy(&mic[5 * s..10 * s]) / energy(&out[5 * s..10 * s])).log10();
        assert!(removed >= 20.0, "removed only {removed:.1} dB of echo");
        // The voice alone (12-20 s) loses at most 2 dB.
        let lost = 10.0 * (energy(&mic[12 * s..20 * s]) / energy(&out[12 * s..20 * s])).log10();
        assert!(lost <= 2.0, "voice lost {lost:.1} dB");
        let summary = ec.summary();
        assert!(summary.on);
        assert!(summary.removed_db.unwrap() >= 15.0, "{summary:?}");
    }

    #[test]
    fn a_silent_call_reports_nothing_removed() {
        let mut ec = EchoCanceller::new().expect("echo canceller");
        let mut mic = noise(3, STEP * 100, 0.1);
        let call = vec![0.0; STEP];
        for m in mic.chunks_mut(STEP) {
            ec.process(m, &call);
        }
        assert_eq!(ec.summary().removed_db, None);
    }
}
