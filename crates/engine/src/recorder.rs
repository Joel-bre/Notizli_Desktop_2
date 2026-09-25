//! The public recording API: start, watch events, stop or discard.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::analyzer::{Analyzer, HealthReport};
use crate::capture::{self, Capture, Starters};
use crate::error::Error;
use crate::file::OpusFile;
use crate::mixer;
use crate::source::Source;
use crate::webm::Finished;

/// Receives [`RecorderEvent`]s, from the engine's own threads.
pub type Events = Arc<dyn Fn(RecorderEvent) + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecorderEvent {
    /// Every 100 ms. Peaks are 0..1; levels are RMS in dBFS.
    Levels { mic_peak: f32, mic_db: f32, other_peak: Option<f32>, other_db: Option<f32> },
    /// What the other-side channel is recording, e.g. "Microsoft Teams — Speakers (Realtek(R) Audio)".
    Hearing { label: String },
    /// The microphone in use.
    Microphone { label: String },
    /// The call has been silent for `seconds` while the room talked (or is heard again).
    OtherSideSilent { silent: bool, seconds: u32 },
    /// Something the user may want to know (a device switch, a fallback).
    Notice { text: String },
    /// The file can no longer be written; the recording continues in memory.
    DiskError { text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelLayout {
    /// Stereo: channel 0 microphone, channel 1 the call.
    #[serde(rename = "mic_remote")]
    MicRemote,
    /// Microphone only.
    #[serde(rename = "mono")]
    Mono,
}

impl ChannelLayout {
    /// The value the Notizli upload API expects.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelLayout::MicRemote => "mic_remote",
            ChannelLayout::Mono => "mono",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    /// Where to write the WebM file; must not exist yet.
    pub path: PathBuf,
    /// Microphone device id from [`crate::input_devices`]; `None` follows the
    /// system default, also when it changes mid-recording.
    pub mic_device: Option<String>,
    /// Record the call's sound as channel 1.
    pub capture_other_side: bool,
    /// Stored in the file, e.g. "Notizli 0.1.0".
    pub writing_app: String,
}

/// A finished recording.
#[derive(Debug)]
pub struct Recording {
    pub path: PathBuf,
    pub health_path: PathBuf,
    pub layout: ChannelLayout,
    pub duration_ms: u64,
    pub size: u64,
    pub health: HealthReport,
    /// Set when the disk failed during the recording: `unsaved` then holds the
    /// whole file, which the caller must save somewhere else.
    pub disk_error: Option<String>,
    pub unsaved: Option<Vec<u8>>,
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
    mixer: Option<JoinHandle<Analyzer>>,
    writer: Option<JoinHandle<Result<Finished, Error>>>,
    captures: Vec<Box<dyn Capture>>,
    layout: ChannelLayout,
    path: PathBuf,
    started: Instant,
}

impl Recorder {
    /// Open the devices and start writing `cfg.path`. Fails if the microphone
    /// can't be opened or the file can't be created; if only the call's sound
    /// can't be captured, records the microphone alone (and says so).
    pub fn start(cfg: RecorderConfig, events: Events) -> Result<Recorder, Error> {
        Self::start_with(cfg, events, capture::platform())
    }

    pub(crate) fn start_with(cfg: RecorderConfig, events: Events, starters: Starters) -> Result<Recorder, Error> {
        let mic = Arc::new(Source::default());
        let mut captures = vec![(starters.mic)(cfg.mic_device.clone(), mic.clone(), events.clone())?];
        let other = if cfg.capture_other_side {
            let src = Arc::new(Source::default());
            match (starters.other)(src.clone(), events.clone()) {
                Ok(c) => {
                    captures.push(c);
                    Some(src)
                }
                Err(e) => {
                    events(RecorderEvent::Notice { text: format!("The other side can't be recorded ({e}). Recording your microphone only.") });
                    None
                }
            }
        } else {
            None
        };
        let layout = if other.is_some() { ChannelLayout::MicRemote } else { ChannelLayout::Mono };
        let channels = if other.is_some() { 2 } else { 1 };
        let file = match OpusFile::create(&cfg.path, channels, &cfg.writing_app) {
            Ok(f) => f,
            Err(e) => {
                for c in captures {
                    c.stop();
                }
                return Err(e);
            }
        };

        let (tx, rx) = mpsc::channel();
        let writer_events = events.clone();
        let writer = thread::Builder::new().name("notizli-writer".into()).spawn(move || write_all(file, rx, writer_events))?;
        let stop = Arc::new(AtomicBool::new(false));
        let analyzer = Analyzer::new(events, mic.clone(), other.clone());
        let mixer_stop = stop.clone();
        let mixer = thread::Builder::new()
            .name("notizli-mixer".into())
            .spawn(move || mixer::run(mic, other, analyzer, tx, mixer_stop))?;
        Ok(Recorder {
            stop,
            mixer: Some(mixer),
            writer: Some(writer),
            captures,
            layout,
            path: cfg.path,
            started: Instant::now(),
        })
    }

    pub fn layout(&self) -> ChannelLayout {
        self.layout
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stop, finish the file and write the health report next to it
    /// (`<name>.health.json`).
    pub fn stop(mut self) -> Result<Recording, Error> {
        self.stop.store(true, Ordering::SeqCst);
        let analyzer = self.mixer.take().expect("mixer").join().map_err(|_| Error::Encoder("the mixer stopped unexpectedly".into()));
        for c in self.captures.drain(..) {
            c.stop();
        }
        let finished = self.writer.take().expect("writer").join().map_err(|_| Error::Encoder("the writer stopped unexpectedly".into()))??;
        let health = analyzer?.report();
        let health_path = health_path(&self.path);
        if let Ok(json) = serde_json::to_vec_pretty(&health) {
            let _ = std::fs::write(&health_path, json);
        }
        Ok(Recording {
            path: self.path.clone(),
            health_path,
            layout: self.layout,
            duration_ms: finished.duration_ms,
            size: finished.size,
            health,
            disk_error: finished.disk_error,
            unsaved: finished.unsaved,
        })
    }

    /// Stop and delete everything written.
    pub fn discard(self) -> Result<(), Error> {
        let path = self.path.clone();
        let r = self.stop();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(health_path(&path));
        r.map(|_| ())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for c in self.captures.drain(..) {
            c.stop();
        }
    }
}

/// `<name>.health.json` next to a recording.
pub fn health_path(recording: &Path) -> PathBuf {
    recording.with_extension("health.json")
}

fn write_all(mut file: OpusFile, rx: Receiver<Vec<f32>>, events: Events) -> Result<Finished, Error> {
    let mut reported = false;
    for chunk in rx {
        file.push(&chunk)?;
        if !reported {
            if let Some(e) = file.disk_error() {
                reported = true;
                events(RecorderEvent::DiskError { text: e.to_string() });
            }
        }
    }
    file.finish()
}

#[cfg(all(test, feature = "opus"))]
mod tests {
    use super::*;
    use crate::capture::fake;
    use std::sync::Mutex;

    #[test]
    fn records_both_channels_through_the_whole_pipeline() {
        let path = crate::webm::tests::temp_path("pipeline.webm");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let events: Events = Arc::new(move |e| s2.lock().unwrap().push(e));
        let cfg = RecorderConfig { path: path.clone(), mic_device: None, capture_other_side: true, writing_app: "test".into() };
        // Microphone: 440 Hz at 44.1 kHz; call: 1 kHz at 48 kHz, silent for the first second.
        let starters = fake::starters(fake::Tone { rate: 44_100, hz: 440.0, silent_until_ms: 0 }, Some(fake::Tone { rate: 48_000, hz: 1_000.0, silent_until_ms: 1_000 }));
        let rec = Recorder::start_with(cfg, events, starters).unwrap();
        assert_eq!(rec.layout(), ChannelLayout::MicRemote);
        thread::sleep(Duration::from_millis(3_000));
        let done = rec.stop().unwrap();

        assert!((2_900..=3_200).contains(&done.duration_ms), "{}", done.duration_ms);
        let levels = crate::file::tests::channel_levels(&done.path);
        assert!(levels[0] > -20.0 && levels[1] > -25.0, "{levels:?}");
        assert!(done.health_path.exists());
        assert_eq!(done.health.other.as_ref().unwrap().label, "fake call");
        let levels_events = seen.lock().unwrap().iter().filter(|e| matches!(e, RecorderEvent::Levels { .. })).count();
        assert!(levels_events >= 25, "{levels_events} level events");
    }

    #[test]
    fn falls_back_to_microphone_only() {
        let path = crate::webm::tests::temp_path("mono.webm");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let events: Events = Arc::new(move |e| s2.lock().unwrap().push(e));
        let cfg = RecorderConfig { path: path.clone(), mic_device: None, capture_other_side: true, writing_app: "test".into() };
        let rec = Recorder::start_with(cfg, events, fake::starters(fake::Tone { rate: 48_000, hz: 300.0, silent_until_ms: 0 }, None)).unwrap();
        assert_eq!(rec.layout(), ChannelLayout::Mono);
        thread::sleep(Duration::from_millis(500));
        let done = rec.stop().unwrap();
        assert_eq!(crate::webm::read_packets(&done.path).unwrap().0, 1);
        assert!(seen.lock().unwrap().iter().any(|e| matches!(e, RecorderEvent::Notice { .. })));
    }

    #[test]
    fn discard_removes_the_files() {
        let path = crate::webm::tests::temp_path("discard.webm");
        let events: Events = Arc::new(|_| {});
        let cfg = RecorderConfig { path: path.clone(), mic_device: None, capture_other_side: false, writing_app: "test".into() };
        let rec = Recorder::start_with(cfg, events, fake::starters(fake::Tone { rate: 48_000, hz: 300.0, silent_until_ms: 0 }, None)).unwrap();
        thread::sleep(Duration::from_millis(200));
        rec.discard().unwrap();
        assert!(!path.exists());
        assert!(!health_path(&path).exists());
    }
}
