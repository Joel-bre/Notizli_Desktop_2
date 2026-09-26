//! A plain log file next to the health reports (`<data>/logs/notizli.log`),
//! so a problem during a meeting leaves a trace to send to support.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 1_000_000;

struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = time::OffsetDateTime::now_utc();
        let line = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}Z {:<5} {}: {}\n",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
            record.level(),
            record.target(),
            record.args()
        );
        let mut guard = self.file.lock().unwrap();
        if guard.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() > MAX_BYTES) {
            *guard = None;
            let _ = fs::rename(&self.path, self.path.with_extension("old.log"));
        }
        if guard.is_none() {
            *guard = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        if let Some(f) = guard.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.flush();
        }
    }
}

/// Start logging to `<logs_dir>/notizli.log`. Only the first call has an effect.
pub fn init(logs_dir: &Path) {
    let _ = fs::create_dir_all(logs_dir);
    let logger = FileLogger { path: logs_dir.join("notizli.log"), file: Mutex::new(None) };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
