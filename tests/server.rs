//! Web server: page and built-in files (lot 10); live state, commands and
//! protections around a monitor on a local relay (lot 13).
#![cfg(feature = "server")]

mod common;

use std::time::Duration;

use bitchat_geohash::monitor::{Monitor, MonitorConfig};
use bitchat_geohash::nostr::Event;
use bitchat_geohash::server::{Asset, Server, ServerConfig, router, serve};
use bitchat_geohash::store::Store;
use bitchat_geohash::{Error, KIND_GEO_MESSAGE};
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Leaflet 1.9.4 from the npm package, unmodified (D10).
const LEAFLET_JS_SHA256: &str = "db49d009c841f5ca34a888c96511ae936fd9f5533e90d8b2c4d57596f4e5641a";
const LEAFLET_CSS_SHA256: &str = "a7837102824184820dfa198d1ebcd109ff6d0ff9a2672a074b9a1b4d147d04c6";

async fn serve_static() -> String {
    bitchat_geohash::install_crypto_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router()).await });
    format!("http://{addr}")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn page_and_assets_are_served_with_their_type() {
    let base = serve_static().await;
    let page = reqwest::get(format!("{base}/")).await.unwrap();
    assert_eq!(page.status(), 200);
    assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(page.headers()["x-content-type-options"], "nosniff");
    let html = page.text().await.unwrap();

    // Everything the page references is built in, and nothing is external.
    let refs: Vec<&str> = html
        .split('"')
        .filter(|s| s.starts_with("/assets/"))
        .collect();
    assert_eq!(refs.len(), 4, "{refs:?}");
    for r in refs {
        let path = r.strip_prefix("/assets/").unwrap();
        let a = Asset::get(path).unwrap_or_else(|| panic!("{path} not built in"));
        let resp = reqwest::get(format!("{base}{r}")).await.unwrap();
        assert_eq!(resp.status(), 200, "{r}");
        assert_eq!(resp.headers()["content-type"], a.mime, "{r}");
        assert_eq!(resp.bytes().await.unwrap().as_ref(), a.bytes, "{r}");
    }
    assert!(
        !html.contains("https://"),
        "no external resource in the page"
    );
}

#[tokio::test]
async fn unknown_paths_are_not_found() {
    let base = serve_static().await;
    for path in ["/assets/nope.js", "/assets/../Cargo.toml", "/nope"] {
        let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(resp.status(), 404, "{path}");
    }
}

#[test]
fn leaflet_is_the_unmodified_release() {
    let js = Asset::get("leaflet/leaflet.js").unwrap();
    let css = Asset::get("leaflet/leaflet.css").unwrap();
    assert_eq!(sha256(js.bytes), LEAFLET_JS_SHA256);
    assert_eq!(sha256(css.bytes), LEAFLET_CSS_SHA256);
    let license = Asset::get("leaflet/LICENSE").unwrap();
    assert!(license.bytes.starts_with(b"BSD 2-Clause License"));
}

#[test]
fn every_asset_has_content_and_a_type() {
    for a in Asset::all() {
        assert!(!a.bytes.is_empty(), "{}", a.path);
        assert!(!a.mime.is_empty(), "{}", a.path);
    }
}

// --- Lot 13: live state, commands, protections ---

struct Running {
    relay: TestRelay,
    clock: TestClock,
    server: Server,
    http: reqwest::Client,
}

impl Running {
    async fn start(tune: impl FnOnce(&mut MonitorConfig, &mut ServerConfig)) -> Self {
        bitchat_geohash::install_crypto_provider();
        let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
        let clock = TestClock::new();
        let mut config = config("u09t", &clock);
        let mut server = ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            push_every: Duration::from_millis(50),
            ..ServerConfig::default()
        };
        tune(&mut config, &mut server);
        let monitor = Monitor::start(
            config.clone(),
            Some(directory_of(&[relay.url()])),
            None,
            Store::open_in_memory().unwrap(),
        )
        .await
        .unwrap();
        status_until(&monitor, "connected", connected).await;
        let server = serve(monitor, &config, server).await.unwrap();
        Running {
            relay,
            clock,
            server,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server.url())
    }

    /// A command as the page sends it.
    async fn post(&self, path: &str, body: &str) -> (u16, Value) {
        let resp = self
            .http
            .post(self.url(path))
            .header("content-type", "application/json")
            .header("origin", self.server.url())
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        (
            status,
            serde_json::from_str(&resp.text().await.unwrap()).unwrap(),
        )
    }

    async fn state(&self) -> Value {
        let text = reqwest::get(self.url("/api/state"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    async fn stop(self) {
        let monitor = self.server.shutdown().await.unwrap();
        drop(monitor.shutdown().await.unwrap());
        self.relay.shutdown().await;
    }
}

/// Reads an SSE stream: `(event, id, data)`, keep-alive comments skipped.
struct SseReader {
    resp: reqwest::Response,
    buf: String,
}

impl SseReader {
    async fn open(url: &str) -> Self {
        let resp = reqwest::get(url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        SseReader {
            resp,
            buf: String::new(),
        }
    }

    /// The next message, `None` when the stream ends.
    async fn next(&mut self) -> Option<(String, u64, Value)> {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let (mut event, mut id, mut data) = (String::new(), 0, String::new());
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_owned();
                    } else if let Some(v) = line.strip_prefix("id:") {
                        id = v.trim().parse().unwrap();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim_start());
                    }
                }
                if event.is_empty() {
                    continue; // keep-alive comment
                }
                return Some((event, id, serde_json::from_str(&data).unwrap()));
            }
            let chunk = tokio::time::timeout(Duration::from_secs(3), self.resp.chunk())
                .await
                .expect("SSE stream silent")
                .unwrap()?;
            self.buf.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    }

    /// Skips messages until one of `event` matching `want`.
    async fn until(&mut self, event: &str, want: impl Fn(&Value) -> bool) -> (u64, Value) {
        loop {
            let (e, id, data) = self.next().await.expect("stream ended");
            if e == event && want(&data) {
                return (id, data);
            }
        }
    }
}

fn message(channel: &str, at: i64) -> Event {
    let who = Identity::generate();
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        at,
        KIND_GEO_MESSAGE,
        tags,
        "hi".into(),
    )
    .sign(&who)
    .unwrap()
}

fn cell<'a>(cells: &'a Value, geohash: &str) -> Option<&'a Value> {
    cells.as_array()?.iter().find(|c| c["geohash"] == geohash)
}

#[tokio::test]
async fn hello_then_a_delta_after_an_event() {
    let r = Running::start(|_, _| {}).await;
    let mut sse = SseReader::open(&r.url("/api/events")).await;
    let (event, hello_seq, hello) = sse.next().await.unwrap();
    assert_eq!(
        (event.as_str(), hello["type"].as_str()),
        ("hello", Some("hello"))
    );
    assert_eq!(hello["cells"].as_array().unwrap().len(), 33);
    assert_eq!(hello["params"]["start"], "u09t");
    assert_eq!(cell(&hello["cells"], "u09tv").unwrap()["messages"], 0);

    let sent = std::time::Instant::now();
    r.relay.publish(message("u09tv", r.clock.get()));
    let (seq, delta) = sse
        .until("cells", |d| {
            cell(&d["cells"], "u09tv").is_some_and(|c| c["messages"] == 1)
        })
        .await;
    assert!(
        sent.elapsed() < Duration::from_secs(2),
        "{:?}",
        sent.elapsed()
    );
    assert!(seq > hello_seq);
    assert_eq!(delta["seq"], seq);
    // Only what changed.
    assert!(cell(&delta["cells"], "u09tw").is_none());
    r.stop().await;
}

#[tokio::test]
async fn descent_by_post_reaches_the_stream() {
    let r = Running::start(|_, _| {}).await;
    let mut sse = SseReader::open(&r.url("/api/events")).await;
    sse.next().await.unwrap();
    let (status, body) = r.post("/api/descend", r#"{"geohash":"u09tv"}"#).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["added"].as_array().unwrap().len(), 32);
    let (_, delta) = sse
        .until("cells", |d| cell(&d["cells"], "u09tvw").is_some())
        .await;
    assert_eq!(cell(&delta["cells"], "u09tvw").unwrap()["level"], 6);
    assert_eq!(r.state().await["cells"].as_array().unwrap().len(), 65);
    r.stop().await;
}

#[tokio::test]
async fn preview_by_post_reaches_the_stream() {
    let r = Running::start(|_, _| {}).await;
    let mut sse = SseReader::open(&r.url("/api/events")).await;
    sse.next().await.unwrap();
    let (status, body) = r.post("/api/preview", r#"{"geohash":"u09tv"}"#).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["added"].as_array().unwrap().len(), 32);
    let (_, delta) = sse
        .until("cells", |d| cell(&d["cells"], "u09tvw").is_some())
        .await;
    assert_eq!(cell(&delta["cells"], "u09tvw").unwrap()["preview"], true);
    let (status, body) = r.post("/api/preview", r#"{"geohash":"u0b"}"#).await;
    assert_eq!((status, &body["error"]), (422, &json!("outside_area")));
    // The preview ends: the cells are removed from the page.
    r.clock.set(r.clock.get() + 300);
    let (_, removed) = sse
        .until("removed", |d| {
            d["cells"].as_array().is_some_and(|c| c.len() == 32)
        })
        .await;
    assert!(
        removed["cells"]
            .as_array()
            .unwrap()
            .contains(&json!("u09tvw"))
    );
    r.stop().await;
}

#[tokio::test]
async fn refused_commands_say_why() {
    let r = Running::start(|c, _| c.max_channels = 40).await;
    let (status, body) = r.post("/api/descend", r#"{"geohash":"u09tv"}"#).await;
    assert_eq!(status, 409);
    assert_eq!(
        (&body["error"], &body["count"], &body["max"]),
        (&json!("too_many_channels"), &json!(65), &json!(40))
    );
    let (status, body) = r.post("/api/descend", r#"{"geohash":"u0a"}"#).await;
    assert_eq!((status, &body["error"]), (400, &json!("invalid_geohash")));
    let (status, body) = r.post("/api/descend", r#"{"geohash":"u0b"}"#).await;
    assert_eq!((status, &body["error"]), (422, &json!("outside_area")));
    let (status, body) = r.post("/api/descend", r#"{"geo":"u09tv"}"#).await;
    assert_eq!((status, &body["error"]), (422, &json!("bad_request")));
    let (status, body) = r.post("/api/reinstate", "{}").await;
    assert_eq!((status, &body["error"]), (400, &json!("bad_request")));
    r.stop().await;
}

#[tokio::test]
async fn reinstate_by_post() {
    let r = Running::start(|_, _| {}).await;
    r.clock.set(r.clock.get() + inactivity());
    for _ in 0..150 {
        if r.state().await["status"]["retired"] == 33 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (status, body) = r.post("/api/reinstate", r#"{"channels":["u09tv"]}"#).await;
    assert_eq!((status, &body["reinstated"]), (200, &json!(["u09tv"])));
    let (status, body) = r.post("/api/reinstate", r#"{"all":true}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["reinstated"].as_array().unwrap().len(), 32);
    r.stop().await;
}

#[tokio::test]
async fn commands_from_elsewhere_are_refused() {
    let r = Running::start(|_, _| {}).await;
    let descend = || {
        r.http
            .post(r.url("/api/descend"))
            .header("content-type", "application/json")
            .body(r#"{"geohash":"u09tv"}"#)
    };
    // No Origin, another site's Origin.
    assert_eq!(descend().send().await.unwrap().status(), 403);
    let other = descend()
        .header("origin", "http://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 403);
    // Not JSON: a plain HTML form could send it.
    let form = r
        .http
        .post(r.url("/api/descend"))
        .header("content-type", "text/plain")
        .header("origin", r.server.url())
        .body(r#"{"geohash":"u09tv"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), 415);
    // DNS rebinding: a name that is not the server's.
    let rebound = r
        .http
        .get(r.url("/api/state"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), 421);
    let localhost = r
        .http
        .get(r.url("/api/state"))
        .header(
            "host",
            format!("localhost:{}", r.server.local_addr().port()),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(localhost.status(), 200);
    // Too big.
    let big = format!(r#"{{"channels":["{}"]}}"#, "u".repeat(70 * 1024));
    let (status, body) = r.post("/api/reinstate", &big).await;
    assert_eq!((status, &body["error"]), (413, &json!("too_large")));
    // Nothing was done.
    assert_eq!(r.state().await["status"]["watched"], 33);
    r.stop().await;
}

#[tokio::test]
async fn security_headers() {
    let r = Running::start(|_, _| {}).await;
    let page = reqwest::get(r.url("/")).await.unwrap();
    let h = page.headers();
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(
        csp.contains("img-src 'self' data: https://tile.openstreetmap.org"),
        "{csp}"
    );
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["referrer-policy"], "no-referrer");
    r.stop().await;
}

#[tokio::test]
async fn streams_are_bounded_and_closed_at_shutdown() {
    let r = Running::start(|_, s| s.max_streams = 1).await;
    let mut sse = SseReader::open(&r.url("/api/events")).await;
    sse.next().await.unwrap();
    let second = reqwest::get(r.url("/api/events")).await.unwrap();
    assert_eq!(second.status(), 503);
    let monitor = r.server.shutdown().await.unwrap();
    assert!(sse.next().await.is_none(), "stream closed");
    // The monitor is still running and stops cleanly.
    assert_eq!(monitor.status().await.unwrap().watched.len(), 33);
    drop(monitor.shutdown().await.unwrap());
    r.relay.shutdown().await;
}

#[tokio::test]
async fn remote_address_needs_allow_remote() {
    let clock = TestClock::new();
    let config = config("u09t", &clock);
    let monitor = Monitor::start(
        config.clone(),
        Some(directory_of(&["ws://127.0.0.1:1".into()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    let server = ServerConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        ..ServerConfig::default()
    };
    assert!(matches!(
        serve(monitor, &config, server).await,
        Err(Error::RemoteListen(_))
    ));
}
