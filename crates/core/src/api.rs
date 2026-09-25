//! The Notizli server API: pairing and upload (contract fixed by the server).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

/// Base URL of the Notizli server; override at build time with
/// `NOTIZLI_BASE_URL=https://staging.example cargo build`.
pub fn base_url() -> String {
    option_env!("NOTIZLI_BASE_URL").unwrap_or("https://notizli.ch").trim_end_matches('/').to_string()
}

pub fn pair_page_url() -> String {
    format!("{}/pair", base_url())
}

pub fn meeting_url(meeting_id: &str) -> String {
    format!("{}/meetings/{meeting_id}", base_url())
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ApiError {
    #[error("Can't reach Notizli. Check the internet connection.")]
    Network(String),
    #[error("This recorder is no longer paired with your Notizli account.")]
    Unauthorized,
    #[error("The recording is larger than the 200 MB Notizli accepts.")]
    TooLarge,
    #[error("This pairing link has expired or was already used. Start pairing again.")]
    PairingExpired,
    #[error("{0}")]
    PairingRejected(String),
    #[error("Notizli didn't accept the request ({0}).")]
    BadRequest(String),
    #[error("Notizli had a problem ({0}). The recording is kept and will be sent again.")]
    Server(String),
}

impl ApiError {
    /// Worth trying again later with the same file and token.
    pub fn retryable(&self) -> bool {
        matches!(self, ApiError::Network(_) | ApiError::Server(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairPreview {
    pub email: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Paired {
    pub device_token: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub upload_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Uploaded {
    pub meeting_id: String,
}

/// Form fields sent with a recording.
#[derive(Debug, Clone)]
pub struct UploadMeta {
    pub title: String,
    /// ISO-8601 time the recording started.
    pub started_at: String,
    pub channel_layout: String,
}

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    base: String,
}

impl Default for Api {
    fn default() -> Self {
        Self::new(&base_url())
    }
}

/// A short, plain message from a response body; never raw HTML (a proxy's
/// error page must not end up on screen).
fn plain(body: &str, content_type: &str) -> String {
    let t = body.trim();
    if content_type.contains("html") || t.starts_with('<') || t.is_empty() {
        return String::new();
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            return e.chars().take(200).collect();
        }
    }
    t.chars().take(200).collect()
}

async fn read_error(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = resp.text().await.unwrap_or_default();
    (status, plain(&body, &ct))
}

fn network(e: reqwest::Error) -> ApiError {
    ApiError::Network(e.to_string())
}

fn server_detail(status: u16, msg: &str) -> String {
    if msg.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {msg}")
    }
}

impl Api {
    pub fn new(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(120))
            .user_agent(concat!("NotizliRecorder/", env!("CARGO_PKG_VERSION")))
            .https_only(!base.starts_with("http://127.0.0.1") && !base.starts_with("http://localhost"))
            .build()
            .expect("HTTP client");
        Api { http, base: base.trim_end_matches('/').to_string() }
    }

    /// Who a pairing token belongs to, without using it up.
    pub async fn pair_preview(&self, pairing_token: &str) -> Result<PairPreview, ApiError> {
        let resp = self
            .http
            .post(format!("{}/api/public/recorder/pair-preview", self.base))
            .json(&serde_json::json!({ "pairing_token": pairing_token }))
            .send()
            .await
            .map_err(network)?;
        if resp.status().is_success() {
            return resp.json::<PairPreview>().await.map_err(|e| ApiError::Server(e.to_string()));
        }
        let (status, msg) = read_error(resp).await;
        Err(match status {
            404 => ApiError::PairingExpired,
            400 => ApiError::BadRequest(server_detail(status, &msg)),
            _ => ApiError::Server(server_detail(status, &msg)),
        })
    }

    /// Exchange a pairing token for this recorder's device token.
    pub async fn pair(&self, pairing_token: &str, device_label: &str) -> Result<Paired, ApiError> {
        let label: String = device_label.chars().take(80).collect();
        let resp = self
            .http
            .post(format!("{}/api/public/recorder/pair", self.base))
            .json(&serde_json::json!({ "pairing_token": pairing_token, "device_label": label }))
            .send()
            .await
            .map_err(network)?;
        if resp.status().is_success() {
            return resp.json::<Paired>().await.map_err(|e| ApiError::Server(e.to_string()));
        }
        let (status, msg) = read_error(resp).await;
        Err(match status {
            401 => {
                let lower = msg.to_ascii_lowercase();
                if lower.contains("used") || lower.contains("expired") {
                    ApiError::PairingExpired
                } else {
                    ApiError::PairingRejected("This pairing token isn't valid. Start pairing again from notizli.ch/pair.".into())
                }
            }
            400 => ApiError::BadRequest(server_detail(status, &msg)),
            _ => ApiError::Server(server_detail(status, &msg)),
        })
    }

    /// Upload a recording from disk, streaming it (never all in memory).
    /// `sent` is updated with the bytes sent so far.
    pub async fn upload(
        &self,
        upload_url: Option<&str>,
        device_token: &str,
        file: &Path,
        mime: &str,
        meta: &UploadMeta,
        sent: Arc<AtomicU64>,
    ) -> Result<Uploaded, ApiError> {
        let url = upload_url.map(str::to_string).unwrap_or_else(|| format!("{}/api/public/recorder/upload", self.base));
        let f = tokio::fs::File::open(file).await.map_err(|e| ApiError::Network(format!("can't read the recording: {e}")))?;
        let len = f.metadata().await.map_err(|e| ApiError::Network(e.to_string()))?.len();
        sent.store(0, Ordering::Relaxed);
        let counter = sent.clone();
        let stream = ReaderStream::with_capacity(f, 256 * 1024).map(move |chunk| {
            if let Ok(c) = &chunk {
                counter.fetch_add(c.len() as u64, Ordering::Relaxed);
            }
            chunk
        });
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("webm");
        let part = reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), len)
            .file_name(format!("recording.{ext}"))
            .mime_str(mime)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let title: String = meta.title.chars().take(200).collect();
        let form = reqwest::multipart::Form::new()
            .text("title", title)
            .text("started_at", meta.started_at.clone())
            .text("channel_layout", meta.channel_layout.clone())
            .part("audio", part);
        let resp = self.http.post(url).bearer_auth(device_token).multipart(form).send().await.map_err(network)?;
        if resp.status().is_success() {
            return resp.json::<Uploaded>().await.map_err(|e| ApiError::Server(format!("unexpected answer: {e}")));
        }
        let (status, msg) = read_error(resp).await;
        Err(match status {
            401 => ApiError::Unauthorized,
            413 => ApiError::TooLarge,
            400 => ApiError::BadRequest(server_detail(status, &msg)),
            _ => ApiError::Server(server_detail(status, &msg)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, TestServer};

    #[tokio::test]
    async fn preview_and_pair() {
        let srv = TestServer::start(vec![
            Reply::json(200, r#"{"email":"a@example.com","expires_at":"2026-09-26T10:00:00Z"}"#),
            Reply::json(404, r#"{"error":"This pairing link has expired or was already used. Start pairing again."}"#),
            Reply::json(200, r#"{"device_token":"ccp_abc","label":"Desktop (windows)","upload_url":"https://notizli.ch/api/public/recorder/upload"}"#),
            Reply::text(401, "Pairing token already used"),
        ])
        .await;
        let api = Api::new(&srv.url());
        let p = api.pair_preview("ccp_pair_x").await.unwrap();
        assert_eq!(p.email, "a@example.com");
        assert_eq!(api.pair_preview("ccp_pair_x").await, Err(ApiError::PairingExpired));
        let paired = api.pair("ccp_pair_x", "Desktop (windows)").await.unwrap();
        assert_eq!(paired.device_token, "ccp_abc");
        assert_eq!(api.pair("ccp_pair_x", "x").await, Err(ApiError::PairingExpired));
        let reqs = srv.requests();
        assert_eq!(reqs[0].path, "/api/public/recorder/pair-preview");
        assert!(reqs[0].body_text().contains("\"pairing_token\":\"ccp_pair_x\""));
        assert!(reqs[2].body_text().contains("\"device_label\":\"Desktop (windows)\""));
    }

    #[tokio::test]
    async fn upload_streams_the_file_with_fields() {
        let srv = TestServer::start(vec![
            Reply::json(202, r#"{"meeting_id":"m-1"}"#),
            Reply::text(401, "Invalid token"),
            Reply::html(413, "<html><body>413 Request Entity Too Large</body></html>"),
            Reply::html(502, "<html>Bad gateway</html>"),
        ])
        .await;
        let dir = std::env::temp_dir().join(format!("notizli-core-upload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rec.webm");
        std::fs::write(&file, vec![7u8; 300_000]).unwrap();
        let api = Api::new(&srv.url());
        let meta = UploadMeta { title: "Weekly".into(), started_at: "2026-09-26T08:00:00Z".into(), channel_layout: "mic_remote".into() };
        let sent = Arc::new(AtomicU64::new(0));
        let ok = api.upload(None, "tok", &file, "audio/webm", &meta, sent.clone()).await.unwrap();
        assert_eq!(ok.meeting_id, "m-1");
        assert_eq!(sent.load(Ordering::Relaxed), 300_000);
        assert_eq!(api.upload(None, "tok", &file, "audio/webm", &meta, sent.clone()).await, Err(ApiError::Unauthorized));
        assert_eq!(api.upload(None, "tok", &file, "audio/webm", &meta, sent.clone()).await, Err(ApiError::TooLarge));
        let e = api.upload(None, "tok", &file, "audio/webm", &meta, sent.clone()).await.unwrap_err();
        assert_eq!(e, ApiError::Server("HTTP 502".into()));
        assert!(e.retryable());
        assert!(!e.to_string().contains('<'));

        let r = &srv.requests()[0];
        assert_eq!(r.path, "/api/public/recorder/upload");
        assert_eq!(r.header("authorization").as_deref(), Some("Bearer tok"));
        let body = r.body_text();
        for needle in [
            "name=\"title\"\r\n\r\nWeekly",
            "name=\"started_at\"\r\n\r\n2026-09-26T08:00:00Z",
            "name=\"channel_layout\"\r\n\r\nmic_remote",
            "name=\"audio\"; filename=\"recording.webm\"",
            "Content-Type: audio/webm",
        ] {
            assert!(body.contains(needle), "missing {needle:?}");
        }
        assert!(r.body.len() > 300_000);
    }

    #[tokio::test]
    async fn network_errors_are_retryable() {
        let api = Api::new("http://127.0.0.1:9");
        let e = api.pair_preview("ccp_pair_x").await.unwrap_err();
        assert!(matches!(e, ApiError::Network(_)));
        assert!(e.retryable());
    }
}
