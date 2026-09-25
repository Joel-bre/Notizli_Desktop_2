//! Notizli recording engine.
//!
//! Records two sources at once — the room microphone and the sound of the call
//! (what the computer plays) — mixes them into one stereo stream
//! (channel 0 = microphone, channel 1 = the call) and writes it to a WebM/Opus
//! file as it goes, so a crash loses at most a couple of seconds.
//!
//! - Windows: the call is captured from the speaker the meeting app is playing
//!   on (WASAPI endpoint loopback), following device changes. Per-app capture
//!   (process loopback) returned only digital silence on a real laptop.
//! - macOS 14.4+: a Core Audio process tap on everything the Mac plays.
//!
//! The two sources run on their own device clocks; a wall-clock mixer pulls
//! 10 ms at a time from each, so one device disappearing never stalls the other.

// Linux builds only run the tests: the capture plumbing is unused there.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

mod analyzer;
mod capture;
mod error;
mod file;
mod mixer;
mod recorder;
mod resample;
mod source;
pub mod webm;

pub use analyzer::{HealthPoint, HealthReport, SourceSummary};
pub use capture::{input_devices, InputDevice};
pub use error::Error;
pub use recorder::{ChannelLayout, Recorder, RecorderConfig, RecorderEvent, Recording};

/// Sample rate of everything after capture.
pub const SAMPLE_RATE: u32 = 48_000;
