//! The wall-clock mixer: every 10 ms of real time it takes 10 ms from each
//! source (silence if a source has nothing) and hands interleaved frames to the
//! writer. Neither device's clock drives the recording, so one device vanishing
//! (a headset unplugged) never stalls the other channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::analyzer::Analyzer;
use crate::source::Source;
use crate::SAMPLE_RATE;

/// 10 ms.
const STEP: usize = (SAMPLE_RATE / 100) as usize;
/// Frames handed to the writer at once (100 ms).
const CHUNK_STEPS: usize = 10;

pub fn run(
    mic: Arc<Source>,
    other: Option<Arc<Source>>,
    mut analyzer: Analyzer,
    out: Sender<Vec<f32>>,
    stop: Arc<AtomicBool>,
) -> Analyzer {
    let channels = if other.is_some() { 2 } else { 1 };
    let start = Instant::now();
    let mut emitted: u64 = 0;
    let mut mic_buf = vec![0f32; STEP];
    let mut other_buf = vec![0f32; STEP];
    let mut chunk: Vec<f32> = Vec::with_capacity(STEP * channels * CHUNK_STEPS);
    loop {
        let stopping = stop.load(Ordering::SeqCst);
        let due = (start.elapsed().as_nanos() * u128::from(SAMPLE_RATE) / 1_000_000_000) as u64;
        while due >= emitted + STEP as u64 {
            let now = Instant::now();
            mic.take(&mut mic_buf, now);
            match &other {
                Some(o) => {
                    o.take(&mut other_buf, now);
                    analyzer.block(&mic_buf, Some(&other_buf));
                    for (m, o) in mic_buf.iter().zip(&other_buf) {
                        chunk.push(*m);
                        chunk.push(*o);
                    }
                }
                None => {
                    analyzer.block(&mic_buf, None);
                    chunk.extend_from_slice(&mic_buf);
                }
            }
            emitted += STEP as u64;
            if chunk.len() >= STEP * channels * CHUNK_STEPS {
                // The writer only goes away if encoding failed; keep metering.
                let _ = out.send(std::mem::replace(&mut chunk, Vec::with_capacity(STEP * channels * CHUNK_STEPS)));
            }
        }
        mic.trim();
        if let Some(o) = &other {
            o.trim();
        }
        if stopping {
            if !chunk.is_empty() {
                let _ = out.send(chunk);
            }
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    analyzer
}
