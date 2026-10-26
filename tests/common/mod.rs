//! Local relays and servers for the network tests. Each test binary uses a
//! different subset.
#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use bitchat_geohash::directory::RelayDirectory;

/// A relay list with these URLs, all in the centre of Paris (so they are
/// the closest relays to every `u09` channel).
pub fn directory_of(urls: &[String]) -> RelayDirectory {
    let mut csv = String::from("Relay URL,Latitude,Longitude\n");
    for url in urls {
        csv.push_str(&format!("{url},48.8566,2.3522\n"));
    }
    RelayDirectory::parse(&csv)
}

async fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    (l, format!("ws://{addr}"))
}

/// A WebSocket server that answers each REQ with `replies`, where `{sub}`
/// stands for the subscription id.
pub async fn scripted_relay(replies: &[&str]) -> String {
    let (l, url) = listener().await;
    let replies: Vec<String> = replies.iter().map(|r| r.to_string()).collect();
    tokio::spawn(async move {
        while let Ok((stream, _)) = l.accept().await {
            let replies = replies.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(Value::Array(v)) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
                    };
                    if v.first().and_then(Value::as_str) != Some("REQ") {
                        continue;
                    }
                    let sub = v.get(1).and_then(Value::as_str).unwrap_or("");
                    for r in &replies {
                        let _ = ws.send(Message::text(r.replace("{sub}", sub))).await;
                    }
                }
            });
        }
    });
    url
}

/// Accepts TCP connections and never answers: the handshake hangs.
pub async fn silent_server() -> String {
    let (l, url) = listener().await;
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = l.accept().await {
            held.push(stream);
        }
    });
    url
}

/// Answers every HTTP request with this status line, e.g. `404 Not Found`.
pub async fn http_server(status: &'static str) -> String {
    let (l, url) = listener().await;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let reply =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    url
}

/// A local port nobody listens on.
pub async fn closed_port() -> String {
    let (l, url) = listener().await;
    drop(l);
    url
}

/// What a [`recording_relay`] saw.
#[derive(Clone, Default)]
pub struct Recording {
    /// Client frames, with the number of the connection that sent them.
    pub frames: std::sync::Arc<std::sync::Mutex<Vec<(usize, Value)>>>,
    /// Connections open now.
    pub open: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Connections accepted since the start.
    pub accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Recording {
    /// Frames of this type (`"REQ"`, `"CLOSE"`, ...).
    pub fn frames_of(&self, kind: &str) -> Vec<(usize, Value)> {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, v)| v[0] == kind)
            .cloned()
            .collect()
    }

    pub fn open(&self) -> usize {
        self.open.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn accepted(&self) -> usize {
        self.accepted.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A relay that records every client frame and answers each REQ with EOSE.
/// With `challenge`, it sends an AUTH challenge on connect.
pub async fn recording_relay(challenge: bool) -> (String, Recording) {
    use std::sync::atomic::Ordering::SeqCst;
    let (l, url) = listener().await;
    let rec = Recording::default();
    let r = rec.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = l.accept().await {
            let r = r.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let n = r.accepted.fetch_add(1, SeqCst);
                r.open.fetch_add(1, SeqCst);
                if challenge {
                    let _ = ws.send(Message::text(r#"["AUTH","challenge"]"#)).await;
                }
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
                    };
                    if v[0] == "REQ" {
                        let eose = serde_json::json!(["EOSE", v[1]]).to_string();
                        let _ = ws.send(Message::text(eose)).await;
                    }
                    r.frames.lock().unwrap().push((n, v));
                }
                r.open.fetch_sub(1, SeqCst);
            });
        }
    });
    (url, rec)
}

/// Polls `check` every 20 ms for up to 3 s.
pub async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..150 {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// Captured log output (current-thread runtime: the monitor task logs on
/// the test thread).
#[derive(Clone, Default)]
pub struct Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    pub fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    pub fn info_lines(&self, needle: &str) -> usize {
        self.text()
            .lines()
            .filter(|l| l.contains(" INFO ") && l.contains(needle))
            .count()
    }
}

pub struct TestClock(pub std::sync::Arc<std::sync::atomic::AtomicI64>);

impl TestClock {
    pub fn new() -> Self {
        TestClock(std::sync::Arc::new(std::sync::atomic::AtomicI64::new(
            bitchat_envelope::envelope::unix_now(),
        )))
    }
    pub fn set(&self, t: i64) {
        self.0.store(t, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn get(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub struct TempDb(pub std::path::PathBuf);

impl TempDb {
    pub fn new(name: &str) -> Self {
        TempDb(std::env::temp_dir().join(format!(
            "bitchat-geohash-test-{name}-{}.sqlite",
            std::process::id()
        )))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

pub fn config(start: &str, clock: &TestClock) -> bitchat_geohash::monitor::MonitorConfig {
    let c = clock.0.clone();
    bitchat_geohash::monitor::MonitorConfig {
        depth: 1,
        probe: false,
        tick: std::time::Duration::from_millis(30),
        listener: bitchat_geohash::listener::ListenerConfig {
            backoff_min: std::time::Duration::from_millis(50),
            backoff_max: std::time::Duration::from_millis(200),
            clock: std::sync::Arc::new(move || c.load(std::sync::atomic::Ordering::SeqCst)),
            ..bitchat_geohash::listener::ListenerConfig::default()
        },
        ..bitchat_geohash::monitor::MonitorConfig::new(start)
    }
}

pub async fn status_until(
    monitor: &bitchat_geohash::monitor::Monitor,
    what: &str,
    check: impl Fn(&bitchat_geohash::monitor::MonitorStatus) -> bool,
) -> bitchat_geohash::monitor::MonitorStatus {
    for _ in 0..150 {
        let s = monitor.status().await.unwrap();
        if check(&s) {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "timed out waiting for: {what}: {:?}",
        monitor.status().await.unwrap()
    );
}

pub fn connected(s: &bitchat_geohash::monitor::MonitorStatus) -> bool {
    !s.connections.is_empty()
        && s.connections
            .iter()
            .all(|c| c.state == bitchat_geohash::listener::ConnState::Connected)
}

/// What a [`list_server`] answers: `Some(body)` with 200, `None` with 500.
pub type ListBody = std::sync::Arc<std::sync::Mutex<Option<String>>>;

/// An HTTP server for a relay list, whose answer the test changes.
pub async fn list_server(body: Option<String>) -> (String, ListBody) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/relays.csv", l.local_addr().unwrap());
    let shared: ListBody = std::sync::Arc::new(std::sync::Mutex::new(body));
    let b = shared.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = l.accept().await {
            let b = b.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let current = b.lock().unwrap().clone();
                let reply = match current {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/csv\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                };
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (url, shared)
}

/// A relay list CSV with these URLs, in the centre of Paris.
pub fn list_csv(urls: &[&str]) -> String {
    let mut csv = String::from("Relay URL,Latitude,Longitude\n");
    for url in urls {
        csv.push_str(&format!("{url},48.8566,2.3522\n"));
    }
    csv
}

/// Default inactivity delay, in seconds.
pub fn inactivity() -> i64 {
    bitchat_geohash::aggregate::AggregatorConfig::default().inactivity_secs
}
