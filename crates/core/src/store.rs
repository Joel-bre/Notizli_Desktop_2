//! Recordings on disk until the server has accepted them.
//!
//! `<data>/unsent/<id>.webm` is the audio, `<id>.json` its metadata and
//! `<id>.health.json` the engine's health report. A recording is deleted only
//! after the server answered 202; its health report then moves to `logs/`
//! (the last 30 are kept).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use notizli_engine::webm;

const KEEP_LOGS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Being recorded (or the app stopped while recording).
    Recording,
    /// Complete, waiting to be uploaded.
    Ready,
    /// Rejected as over the server's size limit: kept, not retried.
    TooLarge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub title: String,
    /// ISO-8601 (UTC) time the recording started.
    pub started_at: String,
    pub channel_layout: String,
    pub mime: String,
    pub status: Status,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Unix time before which no automatic retry happens.
    #[serde(default)]
    pub retry_after: u64,
    /// Recovered after the app stopped mid-recording.
    #[serde(default)]
    pub recovered: bool,
}

pub struct Store {
    unsent: PathBuf,
    logs: PathBuf,
}

pub fn now_unix() -> u64 {
    OffsetDateTime::now_utc().unix_timestamp().max(0) as u64
}

/// Seconds to wait before the next automatic try, after `attempts` failures.
pub fn backoff_secs(attempts: u32) -> u64 {
    const STEPS: [u64; 7] = [0, 60, 120, 300, 600, 1_800, 3_600];
    STEPS[(attempts as usize).min(STEPS.len() - 1)]
}

impl Store {
    pub fn open(data_dir: &Path) -> io::Result<Store> {
        let s = Store { unsent: data_dir.join("unsent"), logs: data_dir.join("logs") };
        fs::create_dir_all(&s.unsent)?;
        fs::create_dir_all(&s.logs)?;
        Ok(s)
    }

    pub fn folder(&self) -> &Path {
        &self.unsent
    }

    pub fn audio_path(&self, id: &str) -> PathBuf {
        self.unsent.join(format!("{id}.webm"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.unsent.join(format!("{id}.json"))
    }

    pub fn health_path(&self, id: &str) -> PathBuf {
        self.unsent.join(format!("{id}.health.json"))
    }

    /// Reserve a new recording (status Recording) and return its metadata.
    pub fn create(&self, title: &str, started: OffsetDateTime) -> io::Result<Meta> {
        let stamp = started.to_offset(time::UtcOffset::UTC);
        let base = format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            stamp.year(),
            stamp.month() as u8,
            stamp.day(),
            stamp.hour(),
            stamp.minute(),
            stamp.second()
        );
        let mut id = base.clone();
        let mut n = 1;
        while self.meta_path(&id).exists() || self.audio_path(&id).exists() {
            n += 1;
            id = format!("{base}-{n}");
        }
        let meta = Meta {
            id,
            title: title.to_string(),
            started_at: stamp.format(&Rfc3339).unwrap_or_default(),
            channel_layout: "mono".into(),
            mime: webm::MIME.into(),
            status: Status::Recording,
            duration_ms: 0,
            size: 0,
            attempts: 0,
            last_error: None,
            retry_after: 0,
            recovered: false,
        };
        self.save(&meta)?;
        Ok(meta)
    }

    /// Write metadata atomically (temp file + rename).
    pub fn save(&self, meta: &Meta) -> io::Result<()> {
        let tmp = self.unsent.join(format!("{}.json.tmp", meta.id));
        fs::write(&tmp, serde_json::to_vec_pretty(meta)?)?;
        fs::rename(&tmp, self.meta_path(&meta.id))
    }

    pub fn get(&self, id: &str) -> Option<Meta> {
        let data = fs::read(self.meta_path(id)).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Everything not yet accepted by the server, oldest first.
    pub fn list(&self) -> Vec<Meta> {
        let mut out: Vec<Meta> = fs::read_dir(&self.unsent)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?;
                if id.ends_with(".health") {
                    return None;
                }
                self.get(id)
            })
            .collect();
        out.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        out
    }

    /// The server accepted the recording: delete it, keep its health report.
    pub fn accepted(&self, id: &str) -> io::Result<()> {
        let health = self.health_path(id);
        if health.exists() {
            let _ = fs::rename(&health, self.logs.join(format!("{id}.health.json")));
            self.prune_logs();
        }
        fs::remove_file(self.audio_path(id))?;
        fs::remove_file(self.meta_path(id))
    }

    /// Delete a recording on the user's request.
    pub fn discard(&self, id: &str) -> io::Result<()> {
        for p in [self.audio_path(id), self.health_path(id), self.meta_path(id)] {
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn prune_logs(&self) {
        let mut logs: Vec<PathBuf> = fs::read_dir(&self.logs).into_iter().flatten().flatten().map(|e| e.path()).collect();
        logs.sort();
        while logs.len() > KEEP_LOGS {
            let _ = fs::remove_file(logs.remove(0));
        }
    }

    /// At startup: recordings still marked Recording were interrupted (crash,
    /// power loss). Make their files complete and queue them for upload.
    /// Returns the recovered ones.
    pub fn recover(&self) -> Vec<Meta> {
        let mut recovered = Vec::new();
        for mut meta in self.list().into_iter().filter(|m| m.status == Status::Recording) {
            let audio = self.audio_path(&meta.id);
            match webm::repair(&audio, 20) {
                Ok(r) if r.duration_ms > 0 => {
                    meta.status = Status::Ready;
                    meta.duration_ms = r.duration_ms;
                    meta.size = fs::metadata(&audio).map(|m| m.len()).unwrap_or(0);
                    meta.recovered = true;
                    if self.save(&meta).is_ok() {
                        recovered.push(meta);
                    }
                }
                // Nothing usable was written (stopped within the first seconds).
                _ => {
                    let _ = self.discard(&meta.id);
                }
            }
        }
        recovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("notizli-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (Store::open(&dir).unwrap(), dir)
    }

    #[test]
    fn lifecycle() {
        let (s, _) = store("life");
        let t = OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap();
        let mut a = s.create("Weekly", t).unwrap();
        let b = s.create("Second", t).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.started_at, "2026-09-21T14:13:20Z");
        fs::write(s.audio_path(&a.id), b"x").unwrap();
        fs::write(s.health_path(&a.id), b"{}").unwrap();
        a.status = Status::Ready;
        s.save(&a).unwrap();
        assert_eq!(s.list().len(), 2);
        assert_eq!(s.get(&a.id).unwrap().status, Status::Ready);
        s.accepted(&a.id).unwrap();
        assert_eq!(s.list().len(), 1);
        s.discard(&b.id).unwrap();
        assert!(s.list().is_empty());
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(0), 0);
        assert_eq!(backoff_secs(1), 60);
        assert_eq!(backoff_secs(50), 3_600);
    }

    #[test]
    fn recovers_an_interrupted_recording() {
        let (s, _) = store("recover");
        let meta = s.create("Crashed", OffsetDateTime::now_utc()).unwrap();
        let mut w = webm::WebmWriter::create(&s.audio_path(&meta.id), 1, 312, 20, "test").unwrap();
        for i in 0..250u64 {
            w.write_packet(i * 20, &[0xF8, 0xFF, 0xFE]);
        }
        std::mem::forget(w); // the app died here
        let empty = s.create("Nothing", OffsetDateTime::now_utc()).unwrap();

        let rec = s.recover();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].id, meta.id);
        assert_eq!(rec[0].duration_ms, 4_000);
        assert!(rec[0].recovered);
        assert_eq!(s.get(&meta.id).unwrap().status, Status::Ready);
        assert!(s.get(&empty.id).is_none(), "an empty interrupted recording is dropped");
    }
}
