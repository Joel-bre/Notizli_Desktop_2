//! Windows capture (WASAPI).
//!
//! - Microphone: shared-mode capture of the chosen device, or of the Windows
//!   default microphone, reopened when the default changes or the device fails.
//! - The call: endpoint loopback of the speaker the meeting app is playing on,
//!   found from the active audio sessions on every output device (meeting apps
//!   first, then browsers, then anything else, else the default speaker).
//!   Re-checked every second; a new speaker is taken once it is chosen twice
//!   in a row. Per-app capture (process loopback) is deliberately not used: on
//!   a real laptop it returned only digital silence, for every app, while
//!   loopback of the speaker heard the call clearly.
//!
//! Both ask Windows for 48 kHz float stereo and let it convert.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};
use wasapi::{
    initialize_mta, AudioCaptureClient, AudioClient, Device, DeviceEnumerator, Direction, Handle, Role, SampleType, SessionState,
    StreamMode, WaveFormat,
};

use super::{downmix, sleep_unless, Capture, InputDevice, ThreadCapture};
use crate::error::Error;
use crate::recorder::{Events, RecorderEvent};
use crate::source::Source;

const RATE: usize = 48_000;
const CHANNELS: usize = 2;
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Call apps, matched on the executable of the process playing the sound or
/// any of its ancestors (Teams plays from a child ms-teams.exe, browsers from
/// helper processes).
const MEETING_APPS: &[(&str, &str)] = &[
    ("ms-teams.exe", "Microsoft Teams"),
    ("teams.exe", "Microsoft Teams"),
    ("ms-teams_modulehost.exe", "Microsoft Teams"),
    ("zoom.exe", "Zoom"),
    ("ciscocollabhost.exe", "Webex"),
    ("webex.exe", "Webex"),
    ("webexmta.exe", "Webex"),
    ("atmgr.exe", "Webex"),
    ("slack.exe", "Slack"),
    ("whatsapp.exe", "WhatsApp"),
    ("whatsapp.root.exe", "WhatsApp"),
    ("discord.exe", "Discord"),
    ("skype.exe", "Skype"),
];

const BROWSERS: &[(&str, &str)] = &[
    ("chrome.exe", "Google Chrome"),
    ("msedge.exe", "Microsoft Edge"),
    ("firefox.exe", "Firefox"),
    ("brave.exe", "Brave"),
    ("opera.exe", "Opera"),
    ("vivaldi.exe", "Vivaldi"),
    ("arc.exe", "Arc"),
    ("comet.exe", "Comet"),
];

fn com() {
    // Already initialized (possibly as STA on a UI thread) is fine: the device
    // APIs used here work in either apartment.
    let _ = initialize_mta();
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub fn input_devices() -> Result<Vec<InputDevice>, Error> {
    com();
    let e = |e: wasapi::WasapiError| Error::Microphone(e.to_string());
    let enumerator = DeviceEnumerator::new().map_err(e)?;
    let default_id = enumerator.get_default_device_for_role(&Direction::Capture, &Role::Console).and_then(|d| d.get_id()).ok();
    let mut out = Vec::new();
    for device in &enumerator.get_device_collection(&Direction::Capture).map_err(e)? {
        let Ok(device) = device else { continue };
        let Ok(id) = device.get_id() else { continue };
        let name = device.get_friendlyname().unwrap_or_else(|_| "Microphone".into());
        out.push(InputDevice { is_default: default_id.as_deref() == Some(id.as_str()), id, name });
    }
    Ok(out)
}

/// A running shared-mode capture stream, delivered as mono 48 kHz.
struct Stream {
    client: AudioClient,
    capture: AudioCaptureClient,
    event: Option<Handle>,
    raw: VecDeque<u8>,
    frame: Vec<f32>,
}

impl Stream {
    /// Microphones are event-driven; loopback is polled, because loopback
    /// events are not signalled on every Windows 10 build.
    fn open(device: &Device, loopback: bool) -> Result<Stream, String> {
        let mut client = device.get_iaudioclient().map_err(s)?;
        let format = WaveFormat::new(32, 32, &SampleType::Float, RATE, CHANNELS, None);
        let mode = if loopback {
            StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 2_000_000 }
        } else {
            StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }
        };
        client.initialize_client(&format, &Direction::Capture, &mode).map_err(s)?;
        let event = if loopback { None } else { Some(client.set_get_eventhandle().map_err(s)?) };
        let capture = client.get_audiocaptureclient().map_err(s)?;
        client.start_stream().map_err(s)?;
        Ok(Stream { client, capture, event, raw: VecDeque::new(), frame: Vec::new() })
    }

    /// Wait briefly for audio and put it in `mono` (cleared first).
    fn read(&mut self, mono: &mut Vec<f32>) -> Result<(), String> {
        match &self.event {
            Some(h) => {
                let _ = h.wait_for_event(100);
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
        loop {
            match self.capture.get_next_packet_size() {
                Ok(Some(n)) if n > 0 => {
                    let before = self.raw.len();
                    let info = self.capture.read_from_device_to_deque(&mut self.raw).map_err(s)?;
                    // A packet flagged silent must be read as zeros, whatever its bytes hold.
                    if info.flags.silent {
                        self.raw.iter_mut().skip(before).for_each(|b| *b = 0);
                    }
                }
                Ok(_) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        let bytes = self.raw.len() - self.raw.len() % (4 * CHANNELS);
        self.frame.clear();
        let mut sample = [0u8; 4];
        for (i, b) in self.raw.drain(..bytes).enumerate() {
            sample[i % 4] = b;
            if i % 4 == 3 {
                self.frame.push(f32::from_le_bytes(sample));
            }
        }
        downmix(&self.frame, CHANNELS, mono);
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}

// ---- microphone ------------------------------------------------------------

pub fn start_mic(device: Option<String>, source: Arc<Source>, events: Events) -> Result<Box<dyn Capture>, Error> {
    let cap = ThreadCapture::spawn("notizli-mic", Duration::from_secs(5), move |stop, ready| {
        mic_thread(device, &source, &events, stop, ready)
    })
    .map_err(Error::Microphone)?;
    Ok(Box::new(cap))
}

/// The chosen device if it is still there, else the default microphone.
/// The flag says whether the default is being followed.
fn pick_mic(enumerator: &DeviceEnumerator, chosen: Option<&str>) -> Result<(Device, bool), String> {
    if let Some(id) = chosen {
        if let Ok(d) = enumerator.get_device(id) {
            if d.get_state().map(|st| st == wasapi::DeviceState::Active).unwrap_or(false) {
                return Ok((d, false));
            }
        }
    }
    let d = enumerator
        .get_default_device_for_role(&Direction::Capture, &Role::Console)
        .map_err(|_| "no microphone is connected".to_string())?;
    Ok((d, true))
}

fn mic_thread(chosen: Option<String>, source: &Source, events: &Events, stop: &AtomicBool, ready: &dyn Fn(Result<(), String>)) {
    com();
    let mut started = false;
    let mut mono = Vec::with_capacity(RATE / 10);
    while !stop.load(Ordering::SeqCst) {
        let opened = DeviceEnumerator::new().map_err(s).and_then(|en| {
            let (device, follows_default) = pick_mic(&en, chosen.as_deref())?;
            let id = device.get_id().map_err(s)?;
            let name = device.get_friendlyname().unwrap_or_else(|_| "Microphone".into());
            let stream = Stream::open(&device, false)?;
            Ok((en, stream, id, name, follows_default))
        });
        let (enumerator, mut stream, id, name, follows_default) = match opened {
            Ok(x) => x,
            Err(e) => {
                if !started {
                    ready(Err(e));
                    return;
                }
                sleep_unless(stop, Duration::from_secs(1));
                continue;
            }
        };
        source.opened(&name);
        events(RecorderEvent::Microphone { label: name.clone() });
        if started {
            events(RecorderEvent::Notice { text: format!("Microphone: {name}") });
        } else {
            started = true;
            ready(Ok(()));
        }

        let mut last_check = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            if let Err(e) = stream.read(&mut mono) {
                log::warn!("microphone read failed: {e}");
                events(RecorderEvent::Notice { text: format!("Microphone \"{name}\" stopped; reconnecting…") });
                sleep_unless(stop, Duration::from_millis(300));
                break;
            }
            if !mono.is_empty() {
                source.push(&mono, RATE as u32);
            }
            if last_check.elapsed() >= CHECK_EVERY {
                last_check = Instant::now();
                let default_now = enumerator.get_default_device_for_role(&Direction::Capture, &Role::Console).and_then(|d| d.get_id()).ok();
                let moved_default = follows_default && default_now.is_some() && default_now.as_deref() != Some(id.as_str());
                // The chosen device came back after we fell back to the default.
                let chosen_back =
                    follows_default && chosen.is_some() && pick_mic(&enumerator, chosen.as_deref()).map(|(_, f)| !f).unwrap_or(false);
                if moved_default || chosen_back {
                    break;
                }
            }
        }
    }
}

// ---- the call --------------------------------------------------------------

pub fn start_other_side(source: Arc<Source>, events: Events) -> Result<Box<dyn Capture>, Error> {
    let cap = ThreadCapture::spawn("notizli-call", Duration::from_secs(5), move |stop, ready| call_thread(&source, &events, stop, ready))
        .map_err(Error::OtherSide)?;
    Ok(Box::new(cap))
}

#[derive(Clone, Debug, PartialEq)]
struct Choice {
    device_id: String,
    device_name: String,
    app: Option<String>,
}

impl Choice {
    fn label(&self) -> String {
        match &self.app {
            Some(app) => format!("{app} — {}", self.device_name),
            None => self.device_name.clone(),
        }
    }
}

fn call_thread(source: &Source, events: &Events, stop: &AtomicBool, ready: &dyn Fn(Result<(), String>)) {
    com();
    let mut system = System::new();
    let own_pid = std::process::id();
    let mut started = false;
    let mut current: Option<Choice> = None;
    let mut stream: Option<Stream> = None;
    let mut pending: Option<(String, u8)> = None;
    let mut last_check: Option<Instant> = None;
    let mut mono = Vec::with_capacity(RATE / 10);

    while !stop.load(Ordering::SeqCst) {
        if stream.is_none() || last_check.is_none_or(|t| t.elapsed() >= CHECK_EVERY) {
            last_check = Some(Instant::now());
            match choose_speaker(&mut system, own_pid) {
                Ok(choice) => {
                    let same_device = current.as_ref().is_some_and(|c| c.device_id == choice.device_id);
                    let switch = if stream.is_none() {
                        true
                    } else if same_device {
                        pending = None;
                        false
                    } else {
                        // Take a new speaker only once it is chosen twice in a row.
                        match &mut pending {
                            Some((id, n)) if *id == choice.device_id => {
                                *n += 1;
                                *n >= 2
                            }
                            _ => {
                                pending = Some((choice.device_id.clone(), 1));
                                false
                            }
                        }
                    };
                    if switch {
                        pending = None;
                        stream = None;
                        match open_speaker(&choice.device_id) {
                            Ok(st) => {
                                source.opened(&choice.label());
                                events(RecorderEvent::Hearing { label: choice.label() });
                                if started {
                                    events(RecorderEvent::Notice { text: format!("Now recording the call from \"{}\"", choice.device_name) });
                                } else {
                                    started = true;
                                    ready(Ok(()));
                                }
                                stream = Some(st);
                                current = Some(choice);
                            }
                            Err(e) => {
                                if !started {
                                    ready(Err(e));
                                    return;
                                }
                                log::warn!("could not open \"{}\": {e}", choice.device_name);
                                current = None;
                            }
                        }
                    } else if let Some(cur) = &mut current {
                        // Same speaker, maybe a different app (the call started in a browser).
                        if same_device && cur.app != choice.app && choice.app.is_some() {
                            cur.app = choice.app;
                            source.set_label(&cur.label());
                            events(RecorderEvent::Hearing { label: cur.label() });
                        }
                    }
                }
                Err(e) => {
                    if !started {
                        ready(Err(e));
                        return;
                    }
                }
            }
        }

        match &mut stream {
            Some(st) => match st.read(&mut mono) {
                Ok(()) => {
                    if !mono.is_empty() {
                        source.push(&mono, RATE as u32);
                    }
                }
                Err(e) => {
                    log::warn!("speaker capture stopped: {e}");
                    stream = None;
                    current = None;
                }
            },
            None => sleep_unless(stop, Duration::from_millis(250)),
        }
    }
}

fn open_speaker(device_id: &str) -> Result<Stream, String> {
    let enumerator = DeviceEnumerator::new().map_err(s)?;
    let device = enumerator.get_device(device_id).map_err(s)?;
    Stream::open(&device, true)
}

/// The speaker to record: where the most relevant app is playing, else the
/// default speaker.
fn choose_speaker(system: &mut System, own_pid: u32) -> Result<Choice, String> {
    let enumerator = DeviceEnumerator::new().map_err(s)?;
    let mut sessions: Vec<(String, String, u32, f32)> = Vec::new(); // device id, device name, pid, peak
    if let Ok(devices) = enumerator.get_device_collection(&Direction::Render) {
        for device in &devices {
            let Ok(device) = device else { continue };
            let Ok(id) = device.get_id() else { continue };
            let name = device.get_friendlyname().unwrap_or_else(|_| "Speaker".into());
            let Ok(manager) = device.get_iaudiosessionmanager() else { continue };
            let Ok(list) = manager.get_audiosessionenumerator() else { continue };
            for i in 0..list.get_count().unwrap_or(0) {
                let Ok(control) = list.get_session(i) else { continue };
                if control.get_state().ok() != Some(SessionState::Active) {
                    continue;
                }
                let pid = control.get_process_id().unwrap_or(0);
                if pid == 0 || pid == own_pid {
                    continue; // system sounds, or ourselves
                }
                let peak = control.get_audiometerinformation().and_then(|m| m.get_peak_value()).unwrap_or(0.0);
                sessions.push((id.clone(), name.clone(), pid, peak));
            }
        }
    }

    let names = process_names(system, sessions.iter().map(|x| x.2));
    let mut best: Option<(u8, f32, usize, Option<String>)> = None;
    for (i, (_, _, pid, peak)) in sessions.iter().enumerate() {
        let (priority, app) = classify(&names, *pid, own_pid);
        if priority == u8::MAX {
            continue;
        }
        let better = best.as_ref().is_none_or(|(p, pk, _, _)| priority < *p || (priority == *p && *peak > *pk));
        if better {
            best = Some((priority, *peak, i, app));
        }
    }
    if let Some((_, _, i, app)) = best {
        let (id, name, _, _) = &sessions[i];
        return Ok(Choice { device_id: id.clone(), device_name: name.clone(), app });
    }
    let device = enumerator
        .get_default_device_for_role(&Direction::Render, &Role::Console)
        .map_err(|_| "no speaker or headphones are connected".to_string())?;
    Ok(Choice {
        device_id: device.get_id().map_err(s)?,
        device_name: device.get_friendlyname().unwrap_or_else(|_| "Speaker".into()),
        app: None,
    })
}

/// Executable name and parent of each process and its ancestors.
fn process_names(system: &mut System, pids: impl Iterator<Item = u32>) -> HashMap<u32, (String, Option<u32>)> {
    let mut known: HashMap<u32, (String, Option<u32>)> = HashMap::new();
    let mut todo: Vec<Pid> = pids.map(Pid::from_u32).collect();
    for _ in 0..16 {
        if todo.is_empty() {
            break;
        }
        system.refresh_processes(ProcessesToUpdate::Some(&todo), true);
        let mut next: Vec<Pid> = Vec::new();
        for pid in &todo {
            if let Some(p) = system.process(*pid) {
                let parent = p.parent().map(|pp| pp.as_u32());
                known.insert(pid.as_u32(), (p.name().to_string_lossy().to_ascii_lowercase(), parent));
                if let Some(pp) = parent {
                    if !known.contains_key(&pp) && !next.iter().any(|x| x.as_u32() == pp) {
                        next.push(Pid::from_u32(pp));
                    }
                }
            }
        }
        todo = next;
    }
    known
}

/// (priority, app name): 0 = call app, 1 = browser, 2 = anything else,
/// MAX = skip (Notizli itself).
fn classify(names: &HashMap<u32, (String, Option<u32>)>, pid: u32, own_pid: u32) -> (u8, Option<String>) {
    let mut found: (u8, Option<String>) = (2, names.get(&pid).map(|(n, _)| n.trim_end_matches(".exe").to_string()));
    let mut cur = Some(pid);
    for _ in 0..16 {
        let Some(p) = cur else { break };
        if p == own_pid {
            return (u8::MAX, None);
        }
        let Some((exe, parent)) = names.get(&p) else { break };
        if exe.starts_with("notizli") {
            return (u8::MAX, None);
        }
        if let Some((_, app)) = MEETING_APPS.iter().find(|(e, _)| e == exe) {
            found = (0, Some((*app).to_string()));
        } else if let Some((_, app)) = BROWSERS.iter().find(|(e, _)| e == exe) {
            if found.0 > 1 {
                found = (1, Some((*app).to_string()));
            }
        }
        cur = *parent;
    }
    found
}
