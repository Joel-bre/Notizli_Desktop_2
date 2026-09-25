//! `notizli-rec`: record the room microphone and the call to a file, show the
//! levels live, and write a plain-language result. Used to check a computer
//! before trusting it with a real meeting.
//!
//!   notizli-rec record [--minutes N] [--out DIR] [--mic DEVICE_ID] [--mic-only]
//!   notizli-rec devices

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notizli_engine::{input_devices, Recorder, RecorderConfig, RecorderEvent, Recording};

const RESULT_FILE: &str = "notizli-test-result.txt";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("devices") => devices(),
        Some("record") | None => record(&args[args.len().min(1)..]),
        _ => {
            eprintln!("usage: notizli-rec record [--minutes N] [--out DIR] [--mic DEVICE_ID] [--mic-only]\n       notizli-rec devices");
            2
        }
    };
    std::process::exit(code);
}

fn devices() -> i32 {
    match input_devices() {
        Ok(list) => {
            for d in list {
                println!("{} {}\n    id: {}", if d.is_default { "*" } else { " " }, d.name, d.id);
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[derive(Default)]
struct Live {
    mic_db: f32,
    other_db: Option<f32>,
    hearing: String,
    microphone: String,
}

fn bar(db: f32) -> String {
    let n = ((db + 60.0) / 5.0).clamp(0.0, 12.0) as usize;
    format!("{:<12}", "#".repeat(n))
}

fn record(args: &[String]) -> i32 {
    let mut minutes = 3.0f64;
    let mut out_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut mic = None;
    let mut other_side = true;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--minutes" => minutes = it.next().and_then(|v| v.parse().ok()).unwrap_or(minutes),
            "--out" => out_dir = it.next().map(PathBuf::from).unwrap_or(out_dir),
            "--mic" => mic = it.next().cloned(),
            "--mic-only" => other_side = false,
            _ => {}
        }
    }
    let stamp = now_stamp();
    let path = out_dir.join(format!("notizli-test-{stamp}.webm"));

    let live = Arc::new(Mutex::new(Live::default()));
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let (l2, n2) = (live.clone(), notices.clone());
    let events = Arc::new(move |e: RecorderEvent| match e {
        RecorderEvent::Levels { mic_db, other_db, .. } => {
            let mut l = l2.lock().unwrap();
            l.mic_db = mic_db;
            l.other_db = other_db;
        }
        RecorderEvent::Hearing { label } => l2.lock().unwrap().hearing = label,
        RecorderEvent::Microphone { label } => l2.lock().unwrap().microphone = label,
        RecorderEvent::OtherSideSilent { silent: true, .. } => {
            n2.lock().unwrap().push("WARNING: the other side has been silent for 2 minutes while you talk".into())
        }
        RecorderEvent::OtherSideSilent { silent: false, .. } => n2.lock().unwrap().push("the other side is heard again".into()),
        RecorderEvent::Notice { text } => n2.lock().unwrap().push(text),
        RecorderEvent::DiskError { text } => n2.lock().unwrap().push(format!("DISK ERROR: {text} (the recording continues in memory)")),
    });

    let cfg = RecorderConfig {
        path: path.clone(),
        mic_device: mic,
        capture_other_side: other_side,
        writing_app: format!("notizli-rec {}", env!("CARGO_PKG_VERSION")),
    };
    let recorder = match Recorder::start(cfg, events) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Could not start recording: {e}");
            eprintln!("{msg}");
            let _ = std::fs::write(out_dir.join(RESULT_FILE), msg);
            return 1;
        }
    };

    println!("Recording to {}", path.display());
    println!("Press Enter to stop (stops by itself after {minutes} min).\n");
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
            stop.store(true, Ordering::SeqCst);
        });
    }
    let limit = Duration::from_secs_f64(minutes * 60.0);
    let started = Instant::now();
    while !stop.load(Ordering::SeqCst) && started.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(250));
        for n in notices.lock().unwrap().drain(..) {
            println!("\r{:<100}", format!("  [{}] {n}", clock(started.elapsed())));
        }
        let l = live.lock().unwrap();
        let other = match l.other_db {
            Some(db) => format!("call {} {:>4.0} dB", bar(db), db),
            None => "call (not recorded)".into(),
        };
        print!("\r{} you {} {:>4.0} dB | {} | {}   ", clock(started.elapsed()), bar(l.mic_db), l.mic_db, other, l.hearing);
        let _ = std::io::stdout().flush();
    }
    println!("\n\nFinishing…");
    let microphone = live.lock().unwrap().microphone.clone();
    match recorder.stop() {
        Ok(rec) => {
            let text = result_text(&rec, &microphone, &stamp);
            println!("\n{text}");
            let _ = std::fs::write(out_dir.join(RESULT_FILE), &text);
            0
        }
        Err(e) => {
            let msg = format!("The recording could not be finished: {e}");
            eprintln!("{msg}");
            let _ = std::fs::write(out_dir.join(RESULT_FILE), msg);
            1
        }
    }
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn now_stamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let f = time::macros::format_description!("[year]-[month]-[day]-[hour][minute][second]");
    now.format(&f).unwrap_or_else(|_| "recording".into())
}

fn result_text(rec: &Recording, microphone: &str, stamp: &str) -> String {
    let h = &rec.health;
    let mut t = String::new();
    t.push_str(&format!("Notizli recording test {stamp} (notizli-rec {}, {})\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS));
    t.push_str(&format!("File: {} ({} s, {} KB, {})\n\n", rec.path.display(), rec.duration_ms / 1000, rec.size / 1024, rec.layout.as_str()));
    t.push_str(&format!("RESULT: {}\n\n", h.verdict));
    t.push_str(&format!(
        "  You (microphone \"{}\"): someone talking for {} s, loudest {:.0} dB, reopened {}x, gaps {}, dropped {} ms\n",
        if microphone.is_empty() { &h.mic.label } else { microphone },
        h.mic.active_s,
        h.mic.loudest_db,
        h.mic.stats.opens.saturating_sub(1),
        h.mic.stats.underruns,
        h.mic.stats.dropped_ms
    ));
    match &h.other {
        Some(o) => t.push_str(&format!(
            "  The call (\"{}\"): heard for {} s, loudest {:.0} dB, switched {}x, gaps {}, dropped {} ms\n",
            o.label,
            o.active_s,
            o.loudest_db,
            o.stats.opens.saturating_sub(1),
            o.stats.underruns,
            o.stats.dropped_ms
        )),
        None => t.push_str("  The call: not recorded\n"),
    }
    if let Some(e) = &rec.disk_error {
        t.push_str(&format!("  DISK ERROR during the recording: {e}\n"));
    }
    t.push_str("\nEvery 10 s (you dB / call dB / seconds talking / seconds heard / what the call channel records):\n");
    for p in &h.timeline {
        t.push_str(&format!(
            "  {}  you {:>5.0}  call {:>5}  {:>2}s {:>2}s  {}\n",
            clock(Duration::from_secs(u64::from(p.t))),
            p.mic_db,
            p.other_db.map(|d| format!("{d:.0}")).unwrap_or_else(|| "-".into()),
            p.mic_talking_s,
            p.other_heard_s,
            p.hearing
        ));
    }
    if !h.events.is_empty() {
        t.push_str("\nEvents:\n");
        for e in &h.events {
            t.push_str(&format!("  {e}\n"));
        }
    }
    t.push_str(&format!("\nDetails: {}\n", rec.health_path.display()));
    t.push_str("Send this whole text to the Notizli team.\n");
    t
}
