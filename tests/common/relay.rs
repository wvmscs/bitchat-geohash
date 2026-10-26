//! A local NIP-01 relay for the network tests, and signing keys.
//!
//! The relay supports `EVENT` (signature checked, answered with `OK`), `REQ`
//! (stored events, `EOSE`, then live events; a REQ with an existing id
//! replaces it) and `CLOSE`. Like real relays, it forwards the ephemeral
//! kinds (20000–29999, so BitChat's 20000/20001) live without storing them.
//! No NIP-42, no limits.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bitchat_geohash::nostr::{Event, Filter};
use futures_util::{SinkExt, StreamExt};
use k256::schnorr::SigningKey;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

struct Sub {
    conn: usize,
    id: String,
    filters: Vec<Filter>,
    out: mpsc::UnboundedSender<String>,
}

#[derive(Default)]
struct Shared {
    stored: Mutex<Vec<Event>>,
    subs: Mutex<Vec<Sub>>,
}

impl Shared {
    fn publish(&self, e: Event) -> Result<(), &'static str> {
        if e.verify().is_err() {
            return Err("invalid: bad signature");
        }
        if !(20000..30000).contains(&e.kind) {
            self.stored.lock().unwrap().push(e.clone());
        }
        for s in self.subs.lock().unwrap().iter() {
            if s.filters.iter().any(|f| f.matches(&e)) {
                let _ = s.out.send(json!(["EVENT", s.id, e]).to_string());
            }
        }
        Ok(())
    }
}

pub struct TestRelay {
    shared: Arc<Shared>,
    addr: SocketAddr,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl TestRelay {
    /// Binds (`"127.0.0.1:0"` picks a free port) and serves in the background.
    pub async fn spawn(bind: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(bind).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared::default());
        let (stop, stop_rx) = watch::channel(false);
        let s = shared.clone();
        let task = tokio::spawn(async move {
            let mut stop_rx = stop_rx;
            let next_conn = AtomicUsize::new(0);
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        let conn = next_conn.fetch_add(1, Ordering::SeqCst);
                        tokio::spawn(serve(stream, conn, s.clone(), stop_rx.clone()));
                    }
                }
            }
        });
        Ok(TestRelay {
            shared,
            addr,
            stop,
            task,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Publishes as if a client had sent it.
    pub fn publish(&self, e: Event) {
        self.shared.publish(e).expect("test event must verify");
    }

    /// Stops accepting and closes every connection.
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

async fn serve(
    stream: TcpStream,
    conn: usize,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut source) = ws.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            msg = source.next() => {
                let Some(Ok(Message::Text(text))) = msg else {
                    match msg {
                        Some(Ok(_)) => continue,
                        _ => break,
                    }
                };
                let Ok(Value::Array(frame)) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                match frame.first().and_then(Value::as_str) {
                    Some("EVENT") => {
                        let reply = match frame.get(1).map(|v| serde_json::from_value::<Event>(v.clone())) {
                            Some(Ok(e)) => {
                                let id = e.id.clone();
                                match shared.publish(e) {
                                    Ok(()) => json!(["OK", id, true, ""]),
                                    Err(why) => json!(["OK", id, false, why]),
                                }
                            }
                            _ => json!(["NOTICE", "invalid: malformed EVENT"]),
                        };
                        let _ = out.send(reply.to_string());
                    }
                    Some("REQ") => {
                        let Some(id) = frame.get(1).and_then(Value::as_str).map(str::to_owned) else { continue };
                        let filters: Vec<Filter> = frame[2..].iter().filter_map(Filter::from_json).collect();
                        let mut subs = shared.subs.lock().unwrap();
                        subs.retain(|s| !(s.conn == conn && s.id == id));
                        for e in shared.stored.lock().unwrap().iter() {
                            if filters.iter().any(|f| f.matches(e)) {
                                let _ = out.send(json!(["EVENT", id, e]).to_string());
                            }
                        }
                        let _ = out.send(json!(["EOSE", id]).to_string());
                        subs.push(Sub { conn, id, filters, out: out.clone() });
                    }
                    Some("CLOSE") => {
                        if let Some(id) = frame.get(1).and_then(Value::as_str) {
                            shared.subs.lock().unwrap().retain(|s| !(s.conn == conn && s.id == id));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    shared.subs.lock().unwrap().retain(|s| s.conn != conn);
    drop(out);
    let _ = writer.await;
}

/// A signing key for test events. Keys are derived from a counter, so they
/// are distinct within a test binary and need no random source.
pub struct Identity {
    key: SigningKey,
}

impl Identity {
    pub fn generate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            let seed = Sha256::digest(format!("bitchat-geohash test key {n}").as_bytes());
            if let Ok(key) = SigningKey::from_bytes(&seed) {
                return Identity { key };
            }
        }
    }

    pub fn public_key_hex(&self) -> String {
        hex(&self.key.verifying_key().to_bytes())
    }
}

/// Signs test events (the library itself never signs).
pub trait Sign: Sized {
    fn sign(self, who: &Identity) -> Result<Event, String>;
}

impl Sign for Event {
    fn sign(mut self, who: &Identity) -> Result<Event, String> {
        if self.pubkey != who.public_key_hex() {
            return Err("signing key does not match pubkey".into());
        }
        self.id = self.compute_id();
        let id: Vec<u8> = (0..self.id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&self.id[i..i + 2], 16).unwrap())
            .collect();
        let sig = who
            .key
            .sign_raw(&id, &[0u8; 32])
            .map_err(|e| e.to_string())?;
        self.sig = Some(hex(&sig.to_bytes()));
        Ok(self)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
