//! Platform capture: opens the devices and pushes mono audio into [`Source`]s.
//! Each source runs on its own thread, which reopens the device when it
//! changes or fails.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;

use crate::error::Error;
use crate::recorder::Events;
use crate::source::Source;

#[cfg(any(windows, test))]
mod choose;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(all(test, feature = "opus"))]
pub(crate) mod fake;

/// A microphone the user can choose.
#[derive(Clone, Debug, Serialize)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Microphones currently available.
pub fn input_devices() -> Result<Vec<InputDevice>, Error> {
    #[cfg(windows)]
    return windows::input_devices();
    #[cfg(target_os = "macos")]
    return macos::input_devices();
    #[cfg(not(any(windows, target_os = "macos")))]
    Err(Error::Unsupported("recording is only available on Windows and macOS"))
}

pub(crate) trait Capture: Send {
    fn stop(self: Box<Self>);
}

pub(crate) type MicStarter = Box<dyn FnOnce(Option<String>, Arc<Source>, Events) -> Result<Box<dyn Capture>, Error> + Send>;
pub(crate) type OtherStarter = Box<dyn FnOnce(Arc<Source>, Events) -> Result<Box<dyn Capture>, Error> + Send>;

pub(crate) struct Starters {
    pub mic: MicStarter,
    pub other: OtherStarter,
}

pub(crate) fn platform() -> Starters {
    #[cfg(windows)]
    return Starters { mic: Box::new(windows::start_mic), other: Box::new(windows::start_other_side) };
    #[cfg(target_os = "macos")]
    return Starters { mic: Box::new(macos::start_mic), other: Box::new(macos::start_other_side) };
    #[cfg(not(any(windows, target_os = "macos")))]
    Starters {
        mic: Box::new(|_, _, _| Err(Error::Unsupported("recording is only available on Windows and macOS"))),
        other: Box::new(|_, _| Err(Error::Unsupported("recording is only available on Windows and macOS"))),
    }
}

/// A capture running on its own thread until stopped.
pub(crate) struct ThreadCapture {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ThreadCapture {
    /// Run `body` on a new thread. It must call `ready` once the device is
    /// open (or failed to open) and return when `stop` becomes true.
    /// `start` waits up to `timeout` for that first result.
    pub(crate) fn spawn<F>(name: &str, timeout: Duration, body: F) -> Result<Self, String>
    where
        F: FnOnce(&AtomicBool, &dyn Fn(Result<(), String>)) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let thread_stop = stop.clone();
        let handle = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let ready = move |r: Result<(), String>| {
                    let _ = tx.send(r);
                };
                body(&thread_stop, &ready);
            })
            .map_err(|e| e.to_string())?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(())) => Ok(ThreadCapture { stop, handle: Some(handle) }),
            Ok(Err(e)) => {
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("the device did not start in time".into())
            }
        }
    }
}

impl Capture for ThreadCapture {
    fn stop(mut self: Box<Self>) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Sleep in small steps so `stop` stays responsive.
#[allow(dead_code)]
pub(crate) fn sleep_unless(stop: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(50);
    let mut slept = Duration::ZERO;
    while slept < total && !stop.load(Ordering::SeqCst) {
        thread::sleep(step);
        slept += step;
    }
}

/// Average interleaved frames of `channels` into mono.
#[allow(dead_code)]
pub(crate) fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    for frame in interleaved.chunks_exact(channels) {
        out.push(frame.iter().sum::<f32>() * scale);
    }
}
