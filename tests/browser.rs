// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The map in the browser, without a server: the site of `web/build.sh` in
//! a headless Google Chrome, driven over the DevTools protocol. Ignored by
//! default (it needs the built site and Chrome):
//!
//! ```sh
//! web/build.sh && cargo test --test browser -- --ignored
//! ```
//!
//! On a local relay; on `$BITCHAT_TEST_RELAY` when set (closed loop: a
//! message published there with `nak` and a throwaway key); on the real
//! network, read only, with `BITCHAT_REAL_NETWORK=1` (a screenshot in
//! `target/web-real-network.png`). `BITCHAT_CHROME` overrides the path of
//! Chrome.
#![cfg(feature = "server")]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use bitchat_geohash::KIND_GEO_MESSAGE;
use bitchat_geohash::nostr::{Event, unix_now};
use common::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/// Serves the site, and `/relays.csv`: one relay, in Paris.
async fn serve_site(root: PathBuf, relay: Option<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (root, relay) = (root.clone(), relay.clone());
            tokio::spawn(async move {
                let mut buf = vec![0; 8192];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request.split(' ').nth(1).unwrap_or("/");
                let path = path.split('?').next().unwrap_or("/");
                let (status, kind, body) =
                    if let Some(relay) = relay.filter(|_| path == "/relays.csv") {
                        let csv = format!("Relay URL,Latitude,Longitude\n{relay},48.8566,2.3522\n");
                        ("200 OK", "text/csv", csv.into_bytes())
                    } else {
                        let file = if path == "/" {
                            "index.html"
                        } else {
                            &path[1..]
                        };
                        match std::fs::read(root.join(file))
                            .ok()
                            .filter(|_| !file.contains(".."))
                        {
                            Some(bytes) => ("200 OK", mime(file), bytes),
                            None => ("404 Not Found", "text/plain", b"not found".to_vec()),
                        }
                    };
                let head = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    base
}

fn mime(file: &str) -> &'static str {
    match Path::new(file).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

/// Headless Chrome with its own profile, killed and removed on drop.
struct Chrome {
    child: Child,
    profile: PathBuf,
    port: u16,
}

impl Chrome {
    fn launch(path: &str) -> Chrome {
        let profile = std::env::temp_dir().join(format!(
            "bitchat-geohash-chrome-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&profile).unwrap();
        let child = Command::new(path)
            .args([
                "--headless=new",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                &format!("--user-data-dir={}", profile.display()),
                "about:blank",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cannot start Chrome");
        let file = profile.join("DevToolsActivePort");
        let since = Instant::now();
        let port = loop {
            if let Some(port) = std::fs::read_to_string(&file)
                .ok()
                .and_then(|s| s.lines().next()?.parse().ok())
            {
                break port;
            }
            assert!(
                since.elapsed() < Duration::from_secs(15),
                "Chrome did not start"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        Chrome {
            child,
            profile,
            port,
        }
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The DevTools protocol of one page.
struct Page {
    ws: Socket,
    next: u64,
    /// Exceptions thrown in the page or its worker, and console errors.
    errors: Vec<String>,
}

impl Page {
    async fn open(chrome: &Chrome) -> Page {
        let targets: Value = reqwest::get(format!("http://127.0.0.1:{}/json/list", chrome.port))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let url = targets
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["type"] == "page")
            .and_then(|t| t["webSocketDebuggerUrl"].as_str())
            .expect("a page")
            .to_owned();
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let mut page = Page {
            ws,
            next: 1,
            errors: Vec::new(),
        };
        page.call("Runtime.enable", json!({})).await;
        page
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        self.ws
            .send(Message::text(request.to_string()))
            .await
            .unwrap();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(30), self.ws.next())
                .await
                .expect("DevTools silent")
                .expect("DevTools closed")
                .unwrap();
            let Message::Text(text) = message else {
                continue;
            };
            let v: Value = serde_json::from_str(text.as_str()).unwrap();
            if v["id"] == id {
                assert!(v["error"].is_null(), "{method}: {}", v["error"]);
                return v["result"].clone();
            }
            self.note(&v);
        }
    }

    fn note(&mut self, v: &Value) {
        match v["method"].as_str() {
            Some("Runtime.exceptionThrown") => self
                .errors
                .push(v["params"]["exceptionDetails"].to_string()),
            Some("Runtime.consoleAPICalled") if v["params"]["type"] == "error" => {
                self.errors.push(v["params"]["args"].to_string())
            }
            _ => {}
        }
    }

    /// Evaluates `expression` in the page, promises awaited; its JSON value.
    async fn eval(&mut self, expression: &str) -> Value {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
            )
            .await;
        assert!(
            r["exceptionDetails"].is_null(),
            "{expression}: {}",
            r["exceptionDetails"]
        );
        r["result"]["value"].clone()
    }

    /// Waits until `expression` is true.
    async fn until(&mut self, what: &str, expression: &str) {
        let since = Instant::now();
        while self.eval(expression).await != json!(true) {
            if since.elapsed() > Duration::from_secs(30) {
                let panel = self.eval(PANEL).await;
                panic!(
                    "{what}: still not, after 30 s; errors: {:?}; panel: {panel}",
                    self.errors
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// `/api/state` as the page sees it (through shim.js and the worker).
    async fn state(&mut self) -> Value {
        self.eval("fetch('/api/state').then(r => r.json())").await
    }
}

const PANEL: &str = "(document.getElementById('panel')?.innerText ?? '')";

fn message(channel: &str) -> Event {
    let who = Identity::generate();
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        unix_now(),
        KIND_GEO_MESSAGE,
        tags,
        "hi".into(),
    )
    .sign(&who)
    .unwrap()
}

fn site() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("target/web")
}

fn chrome_path() -> String {
    std::env::var("BITCHAT_CHROME").unwrap_or_else(|_| CHROME.to_owned())
}

/// The built site and Chrome are there; otherwise the test says so and
/// does nothing (`cargo test -- --ignored` also runs the timing tests).
fn ready() -> bool {
    let built = site().join("assets/pkg/bitchat_geohash_bg.wasm").exists();
    let chrome = Path::new(&chrome_path()).exists();
    if !built || !chrome {
        eprintln!("browser test skipped: site built: {built} (web/build.sh), Chrome: {chrome}");
    }
    built && chrome
}

/// The built site, served, and Chrome on its page with `query`.
async fn open_map(relay: Option<String>, query: &str) -> (Chrome, Page, String) {
    bitchat_geohash::install_crypto_provider();
    let base = serve_site(site(), relay).await;
    let chrome = Chrome::launch(&chrome_path());
    let mut page = Page::open(&chrome).await;
    let url = format!("{base}/?{}", query.replace("{base}", &base));
    page.call("Page.navigate", json!({ "url": url })).await;
    (chrome, page, base)
}

#[tokio::test]
#[ignore = "needs web/build.sh and Google Chrome"]
async fn the_map_runs_in_the_browser_without_a_server() {
    if !ready() {
        return;
    }
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let (chrome, mut page, _) = open_map(
        Some(relay.url()),
        "area=u09t&relays-csv={base}/relays.csv&no-android",
    )
    .await;

    // The page's own panel, fed by the event stream from the worker.
    page.until(
        "connected",
        &format!("{PANEL}.includes('connections: 1 / 1 open')"),
    )
    .await;
    page.until("live", &format!("{PANEL}.includes('live')"))
        .await;
    let state = page.state().await;
    assert_eq!(state["cells"].as_array().unwrap().len(), 33);
    assert_eq!(state["params"]["start"], "u09t");
    page.until(
        "cells drawn",
        "document.querySelectorAll('.leaflet-overlay-pane path').length >= 32",
    )
    .await;

    // A message on the relay reaches the map.
    relay.publish(message("u09tv"));
    page.until(
        "message counted",
        "fetch('/api/state').then(r => r.json()).then(s => \
         s.cells.some(c => c.geohash === 'u09tv' && c.messages === 1))",
    )
    .await;

    // A command, as the page sends it.
    let added = page
        .eval(
            "fetch('/api/descend', {method: 'POST', headers: {'content-type': 'application/json'}, \
             body: JSON.stringify({geohash: 'u09tv'})}).then(r => r.json())",
        )
        .await;
    assert_eq!(added["added"].as_array().unwrap().len(), 32, "{added}");

    // The state is kept in the browser's database: after a reload, the
    // descent is still there (saved within 5 s).
    tokio::time::sleep(Duration::from_secs(6)).await;
    page.call("Page.reload", json!({})).await;
    page.until(
        "connected again",
        &format!("{PANEL}.includes('connections: 1 / 1 open')"),
    )
    .await;
    let state = page.state().await;
    assert_eq!(
        state["cells"].as_array().unwrap().len(),
        65,
        "descent restored"
    );
    assert!(page.errors.is_empty(), "{:?}", page.errors);
    drop(chrome);
    relay.shutdown().await;
}

const COUNTED_IN_U09TV: &str = "fetch('/api/state').then(r => r.json()).then(s => \
     s.cells.some(c => c.geohash === 'u09tv' && c.messages >= 1))";

/// Closed loop on the user's own relay: the map listens to it alone, and a
/// message published there with a throwaway key (`nak`) is counted.
#[tokio::test]
#[ignore = "needs web/build.sh, Google Chrome, nak and $BITCHAT_TEST_RELAY"]
async fn the_map_on_the_test_relay() {
    let Ok(relay) = std::env::var("BITCHAT_TEST_RELAY") else {
        eprintln!("BITCHAT_TEST_RELAY not set: nothing done");
        return;
    };
    if !ready() {
        return;
    }
    let (chrome, mut page, _) = open_map(
        Some(relay.clone()),
        "area=u09t&relays-csv={base}/relays.csv&no-android",
    )
    .await;
    page.until(
        "connected",
        &format!("{PANEL}.includes('connections: 1 / 1 open')"),
    )
    .await;
    let key = Command::new("nak")
        .args(["key", "generate"])
        .output()
        .expect("nak");
    let key = String::from_utf8(key.stdout).unwrap();
    let published = Command::new("nak")
        .args([
            "event",
            "-k",
            "20000",
            "-t",
            "g=u09tv",
            "-c",
            "bitchat-geohash browser build: qualification",
            "--sec",
            key.trim(),
            &relay,
        ])
        .output()
        .expect("nak");
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    page.until("message counted", COUNTED_IN_U09TV).await;
    assert!(page.errors.is_empty(), "{:?}", page.errors);
    drop(chrome);
}

/// The real network, read only: the official lists downloaded by the page,
/// relays probed and listened to from the browser.
#[tokio::test]
#[ignore = "needs web/build.sh, Google Chrome and BITCHAT_REAL_NETWORK=1"]
async fn the_map_on_the_real_network() {
    if std::env::var("BITCHAT_REAL_NETWORK").as_deref() != Ok("1") {
        eprintln!("BITCHAT_REAL_NETWORK not 1: nothing done");
        return;
    }
    if !ready() {
        return;
    }
    let (chrome, mut page, _) = open_map(None, "area=u09").await;
    page.until(
        "relays listened",
        &format!(
            "{PANEL}.includes('live') && \
             /connections: [1-9][0-9]* \\/ /.test({PANEL})"
        ),
    )
    .await;
    // Some time for the presences of the area to come.
    tokio::time::sleep(Duration::from_secs(60)).await;
    let state = page.state().await;
    let status = &state["status"];
    eprintln!(
        "real network: {} cells, {} watched, relays {} listened, {} set aside, connections {} / {}",
        state["cells"].as_array().map_or(0, Vec::len),
        status["watched"],
        status["relays"],
        status["dropped_relays"].as_array().map_or(0, Vec::len),
        status["connections_up"],
        status["connections"],
    );
    let active: Vec<String> = state["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["messages"].as_u64().unwrap_or(0) + c["presences"].as_u64().unwrap_or(0) > 0)
        .map(|c| {
            format!(
                "{} ({} msg, {} presences)",
                c["geohash"], c["messages"], c["presences"]
            )
        })
        .collect();
    eprintln!("active cells: {active:?}");
    let shot = page
        .call("Page.captureScreenshot", json!({ "format": "png" }))
        .await;
    let png = base64_decode(shot["data"].as_str().unwrap());
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/web-real-network.png");
    std::fs::write(&path, png).unwrap();
    eprintln!("screenshot: {}", path.display());
    assert!(status["connections_up"].as_u64().unwrap_or(0) > 0);
    assert!(page.errors.is_empty(), "{:?}", page.errors);
    drop(chrome);
}

/// Standard base64, as DevTools sends screenshots.
fn base64_decode(s: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').map(value).collect();
    bytes
        .chunks(4)
        .flat_map(|q| {
            let n = q.iter().fold(0u32, |n, &v| n << 6 | v as u32) << (6 * (4 - q.len()));
            let b = n.to_be_bytes();
            b[1..q.len()].to_vec()
        })
        .collect()
}
