// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! One task per relay connection: subscriptions from the listen plan,
//! reconnection with backoff (lot 7).
//!
//! [`Listener::sync`] brings the open connections in line with a plan: it
//! opens the missing ones, sends the new or changed subscriptions (same id,
//! so the relay drops the old filter), closes the emptied ones and
//! disconnects relays left without channel. Read-only: nothing is ever
//! published, and AUTH challenges are ignored, as the app does.

use std::collections::BTreeMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::aggregate::Observation;
use crate::nostr::{Event, unix_now};
use crate::plan::{ListenPlan, Subscription};

/// Unix seconds; injectable for tests.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(unix_now)
}

#[derive(Clone)]
pub struct ListenerConfig {
    /// First reconnection delay, doubled at each failure (with jitter).
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    pub connect_timeout: Duration,
    /// Most connections open at once, all relays together.
    pub max_connections: usize,
    /// How far back a REQ asks (`since = now - lookback`).
    pub lookback_secs: i64,
    pub clock: Clock,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        ListenerConfig {
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(300),
            connect_timeout: Duration::from_secs(10),
            max_connections: 256,
            lookback_secs: 3600,
            clock: system_clock(),
        }
    }
}

/// State of one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    /// Disconnected; next attempt after this delay.
    Waiting {
        retry_in: Duration,
    },
    /// Given up for good: the relay won't recover by itself, or requires
    /// AUTH (which the app does not do).
    Abandoned {
        auth: bool,
        reason: String,
    },
}

/// What the connections report.
#[derive(Debug, Clone)]
pub enum ListenerEvent {
    Observation(Observation),
    State {
        relay: String,
        connection: usize,
        state: ConnState,
    },
    /// The relay closed a subscription (other than `auth-required:`).
    Closed {
        relay: String,
        subscription: String,
        reason: String,
    },
    Notice {
        relay: String,
        text: String,
    },
}

enum Cmd {
    Send(Vec<Subscription>),
    Close(Vec<String>),
    Shutdown,
}

struct Conn {
    tx: mpsc::UnboundedSender<Cmd>,
    task: JoinHandle<()>,
    /// Mirror of what the connection is subscribed to.
    subs: Vec<Subscription>,
}

/// The open connections. Events go to the channel given to [`Listener::new`].
pub struct Listener {
    config: ListenerConfig,
    events: mpsc::UnboundedSender<ListenerEvent>,
    conns: BTreeMap<(String, usize), Conn>,
}

impl Listener {
    pub fn new(config: ListenerConfig, events: mpsc::UnboundedSender<ListenerEvent>) -> Self {
        Listener {
            config,
            events,
            conns: BTreeMap::new(),
        }
    }

    /// Open connections (abandoned ones included until the plan drops their
    /// relay).
    pub fn connection_count(&self) -> usize {
        self.conns.len()
    }

    /// Brings the connections in line with `plan`. Must run inside a tokio
    /// runtime.
    pub fn sync(&mut self, plan: &ListenPlan) {
        let mut desired: BTreeMap<(String, usize), Vec<Subscription>> = BTreeMap::new();
        for relay in plan.relays() {
            for (conn, subs) in plan.connections(&relay) {
                desired.insert((relay.clone(), conn), subs);
            }
        }
        let gone: Vec<(String, usize)> = self
            .conns
            .keys()
            .filter(|k| !desired.contains_key(*k))
            .cloned()
            .collect();
        for key in gone {
            if let Some(conn) = self.conns.remove(&key) {
                let _ = conn.tx.send(Cmd::Shutdown);
                tracing::debug!(relay = %key.0, connection = key.1, "disconnecting");
            }
        }
        for (key, subs) in desired {
            let open = self.conns.len();
            match self.conns.get_mut(&key) {
                Some(conn) => {
                    let send: Vec<Subscription> = subs
                        .iter()
                        .filter(|s| !conn.subs.contains(s))
                        .cloned()
                        .collect();
                    let close: Vec<String> = conn
                        .subs
                        .iter()
                        .filter(|old| !subs.iter().any(|s| s.id == old.id))
                        .map(|old| old.id.clone())
                        .collect();
                    if !close.is_empty() {
                        let _ = conn.tx.send(Cmd::Close(close));
                    }
                    if !send.is_empty() {
                        let _ = conn.tx.send(Cmd::Send(send));
                    }
                    conn.subs = subs;
                }
                None if open >= self.config.max_connections => {
                    tracing::warn!(relay = %key.0, max = self.config.max_connections, "connection cap reached");
                }
                None => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    let task = tokio::spawn(run_connection(
                        key.0.clone(),
                        key.1,
                        subs.clone(),
                        rx,
                        self.events.clone(),
                        self.config.clone(),
                    ));
                    self.conns.insert(key, Conn { tx, task, subs });
                }
            }
        }
    }

    /// Closes every connection and waits for the tasks to end.
    pub async fn shutdown(self) {
        for conn in self.conns.values() {
            let _ = conn.tx.send(Cmd::Shutdown);
        }
        for (_, conn) in self.conns {
            let _ = conn.task.await;
        }
    }
}

/// Why a session ended.
enum End {
    Shutdown,
    /// Reconnect after backoff.
    Lost(String),
    /// Don't reconnect.
    Abandon {
        auth: bool,
        reason: String,
    },
}

async fn run_connection(
    relay: String,
    connection: usize,
    mut subs: Vec<Subscription>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    events: mpsc::UnboundedSender<ListenerEvent>,
    config: ListenerConfig,
) {
    let state = |state: ConnState| {
        let _ = events.send(ListenerEvent::State {
            relay: relay.clone(),
            connection,
            state,
        });
    };
    let mut backoff = config.backoff_min;
    loop {
        state(ConnState::Connecting);
        let connect = tokio::time::timeout(
            config.connect_timeout,
            tokio_tungstenite::connect_async(relay.as_str()),
        );
        let end = tokio::select! {
            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => End::Shutdown,
                // Applied below; the session sends the up-to-date list.
                Some(other) => { apply(&mut subs, other); continue; }
            },
            result = connect => match result {
                Err(_) => End::Lost("connection timeout".into()),
                Ok(Err(e)) if crate::probe::is_permanent_ws(&e) => End::Abandon { auth: false, reason: e.to_string() },
                Ok(Err(e)) => End::Lost(e.to_string()),
                Ok(Ok((ws, _))) => {
                    state(ConnState::Connected);
                    backoff = config.backoff_min;
                    session(ws, &relay, &mut subs, &mut rx, &events, &config).await
                }
            },
        };
        match end {
            End::Shutdown => return,
            End::Abandon { auth, reason } => {
                tracing::warn!(%relay, connection, %reason, "relay abandoned");
                state(ConnState::Abandoned { auth, reason });
                return;
            }
            End::Lost(reason) => {
                let retry_in = jitter(backoff);
                tracing::debug!(%relay, connection, %reason, ?retry_in, "connection lost");
                state(ConnState::Waiting { retry_in });
                backoff = (backoff * 2).min(config.backoff_max);
                let sleep = tokio::time::sleep(retry_in);
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        _ = &mut sleep => break,
                        cmd = rx.recv() => match cmd {
                            None | Some(Cmd::Shutdown) => return,
                            Some(other) => apply(&mut subs, other),
                        },
                    }
                }
            }
        }
    }
}

fn apply(subs: &mut Vec<Subscription>, cmd: Cmd) {
    match cmd {
        Cmd::Send(new) => {
            for s in new {
                subs.retain(|old| old.id != s.id);
                subs.push(s);
            }
        }
        Cmd::Close(ids) => subs.retain(|s| !ids.contains(&s.id)),
        Cmd::Shutdown => {}
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn session(
    mut ws: Ws,
    relay: &str,
    subs: &mut Vec<Subscription>,
    rx: &mut mpsc::UnboundedReceiver<Cmd>,
    events: &mpsc::UnboundedSender<ListenerEvent>,
    config: &ListenerConfig,
) -> End {
    let since = |config: &ListenerConfig| Some((config.clock)() - config.lookback_secs);
    for s in subs.iter() {
        if let Err(e) = ws.send(req(s, since(config))).await {
            return End::Lost(e.to_string());
        }
    }
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let frames: Vec<Message> = match &cmd {
                    None | Some(Cmd::Shutdown) => {
                        let _ = ws.close(None).await;
                        return End::Shutdown;
                    }
                    Some(Cmd::Send(new)) => new.iter().map(|s| req(s, since(config))).collect(),
                    Some(Cmd::Close(ids)) => ids.iter().map(|id| Message::text(json!(["CLOSE", id]).to_string())).collect(),
                };
                apply(subs, cmd.expect("handled above"));
                for f in frames {
                    if let Err(e) = ws.send(f).await {
                        return End::Lost(e.to_string());
                    }
                }
            }
            msg = ws.next() => {
                let text = match msg {
                    None => return End::Lost("closed by the relay".into()),
                    Some(Err(e)) => return End::Lost(e.to_string()),
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) => return End::Lost("closed by the relay".into()),
                    Some(Ok(_)) => continue,
                };
                let Ok(Value::Array(v)) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                let str_at = |i: usize| v.get(i).and_then(Value::as_str).unwrap_or("").to_owned();
                match v.first().and_then(Value::as_str) {
                    Some("EVENT") => {
                        let Some(Ok(event)) = v.get(2).map(|e| serde_json::from_value::<Event>(e.clone())) else { continue };
                        if !event.within_tag_limits() {
                            continue;
                        }
                        let _ = events.send(ListenerEvent::Observation(Observation { relay: relay.to_owned(), event }));
                    }
                    Some("CLOSED") => {
                        let reason = str_at(2);
                        if reason.contains("auth-required:") {
                            let _ = ws.close(None).await;
                            return End::Abandon { auth: true, reason };
                        }
                        let _ = events.send(ListenerEvent::Closed { relay: relay.to_owned(), subscription: str_at(1), reason });
                    }
                    Some("NOTICE") => {
                        let _ = events.send(ListenerEvent::Notice { relay: relay.to_owned(), text: str_at(1) });
                    }
                    // AUTH: ignored, as the app does. EOSE: nothing to do.
                    _ => {}
                }
            }
        }
    }
}

fn req(sub: &Subscription, since: Option<i64>) -> Message {
    Message::text(json!(["REQ", sub.id, sub.filter(since).to_json()]).to_string())
}

/// `delay` scaled by a random factor in [0.5, 1.5).
fn jitter(delay: Duration) -> Duration {
    let r = RandomState::new().hash_one(std::time::Instant::now()) % 1000;
    delay.mul_f64(0.5 + r as f64 / 1000.0)
}
