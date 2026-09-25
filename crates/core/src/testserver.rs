//! A tiny HTTP server for tests: answers requests with canned replies, in order,
//! and records what it received.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Reply { status, content_type: "application/json", body: body.into() }
    }
    pub fn text(status: u16, body: &str) -> Self {
        Reply { status, content_type: "text/plain", body: body.into() }
    }
    pub fn html(status: u16, body: &str) -> Self {
        Reply { status, content_type: "text/html", body: body.into() }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub struct TestServer {
    port: u16,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl TestServer {
    pub async fn start(replies: Vec<Reply>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            for reply in replies {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                let header_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(p + 4);
                    }
                };
                let Some(header_end) = header_end else { continue };
                let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
                let mut lines = head.split("\r\n");
                let path = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
                let headers: Vec<(String, String)> = lines
                    .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
                    .collect();
                let find = |name: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
                let mut body = buf[header_end..].to_vec();
                if let Some(len) = find("content-length").and_then(|v| v.parse::<usize>().ok()) {
                    while body.len() < len {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&tmp[..n]);
                    }
                } else if find("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
                    while !body.ends_with(b"0\r\n\r\n") {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&tmp[..n]);
                    }
                    body = dechunk(&body);
                }
                seen2.lock().unwrap().push(Request { path, headers, body });
                let resp = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    reply.content_type,
                    reply.body.len(),
                    reply.body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        TestServer { port, seen }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(p) = data.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(std::str::from_utf8(&data[..p]).unwrap_or("0").trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = p + 2;
        out.extend_from_slice(&data[start..(start + size).min(data.len())]);
        data = &data[(start + size + 2).min(data.len())..];
    }
    out
}
