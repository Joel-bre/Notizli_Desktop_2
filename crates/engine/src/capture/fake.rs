//! Test sources: a tone pushed in 10 ms chunks at real-time pace.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{Capture, Starters, ThreadCapture};
use crate::error::Error;
use crate::recorder::{Events, RecorderEvent};
use crate::source::Source;

#[derive(Clone, Copy)]
pub struct Tone {
    pub rate: u32,
    pub hz: f32,
    pub silent_until_ms: u64,
}

fn start(tone: Tone, label: &'static str, source: Arc<Source>) -> Result<Box<dyn Capture>, String> {
    let cap = ThreadCapture::spawn("fake", Duration::from_secs(1), move |stop, ready| {
        source.opened(label);
        ready(Ok(()));
        let started = Instant::now();
        let chunk = (tone.rate / 100) as usize;
        let mut n: u64 = 0;
        let mut buf = vec![0f32; chunk];
        while !stop.load(Ordering::SeqCst) {
            let silent = started.elapsed() < Duration::from_millis(tone.silent_until_ms);
            for s in buf.iter_mut() {
                *s = if silent { 0.0 } else { 0.3 * (2.0 * std::f32::consts::PI * tone.hz * n as f32 / tone.rate as f32).sin() };
                n += 1;
            }
            source.push(&buf, tone.rate);
            // Keep real-time pace from the start, not per chunk, so no drift builds up.
            let due = Duration::from_secs_f64(n as f64 / f64::from(tone.rate));
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
    })?;
    Ok(Box::new(cap))
}

pub fn starters(mic: Tone, other: Option<Tone>) -> Starters {
    Starters {
        mic: Box::new(move |_, source, events: Events| {
            events(RecorderEvent::Microphone { label: "fake mic".into() });
            start(mic, "fake mic", source).map_err(Error::Microphone)
        }),
        other: Box::new(move |source, _| match other {
            Some(t) => start(t, "fake call", source).map_err(Error::OtherSide),
            None => Err(Error::OtherSide("not available in this test".into())),
        }),
    }
}
