// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Relay probe: read a location channel the way the app does (lot 5).
//!
//! Connects, sends a REQ for kinds 20000/20001 with the `g` tag, and waits
//! for EOSE or CLOSED. Writing is never probed (it would post to the
//! channel), and NIP-42 AUTH is never answered (the app does not).

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::io::ErrorKind;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt, stream};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::nostr::unix_now;
use crate::plan::{ListenPlan, Subscription};
use crate::store::Write;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// Probes running at the same time.
pub const PROBE_CONCURRENCY: usize = 32;

/// Probe outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Answers the channel REQ.
    Ok,
    /// Requires NIP-42 AUTH, which the app does not do: unusable by the app.
    Auth,
    /// Closed the subscription for another reason (may be transient).
    Refused,
    /// May recover: timeout, 5xx, reset.
    Down,
    /// Won't recover by itself: bad certificate, unknown host, connection
    /// refused, HTTP 4xx.
    Dead,
    Unprobed,
}

impl Status {
    /// As stored in the database (`probe_result.status`).
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Auth => "auth",
            Status::Refused => "refused",
            Status::Down => "down",
            Status::Dead => "dead",
            Status::Unprobed => "unprobed",
        }
    }

    /// From the database (`probe_result.status`).
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "ok" => Status::Ok,
            "auth" => Status::Auth,
            "refused" => Status::Refused,
            "down" => Status::Down,
            "dead" => Status::Dead,
            "unprobed" => Status::Unprobed,
            _ => return None,
        })
    }

    /// Whether listening to the relay is worth it. Dead relays won't come
    /// back, and AUTH relays never deliver to a client that doesn't
    /// authenticate; both are set aside, not replaced. Down and refused ones
    /// are kept: the listener retries with backoff.
    pub fn worth_listening(self) -> bool {
        !matches!(self, Status::Dead | Status::Auth)
    }
}

/// Probe result; `text` is for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub status: Status,
    pub text: String,
}

impl Verdict {
    pub fn unprobed() -> Self {
        Verdict {
            status: Status::Unprobed,
            text: String::new(),
        }
    }

    /// The database record of this verdict.
    pub fn to_write(&self, relay: &str, probed_at: i64) -> Write {
        Write::Probe {
            relay: relay.to_owned(),
            probed_at,
            status: self.status.as_str().to_owned(),
            detail: self.text.clone(),
        }
    }
}

/// Probes a relay on one channel, with [`PROBE_TIMEOUT`].
pub async fn probe(url: &str, geohash: &str) -> Verdict {
    probe_with_timeout(url, geohash, PROBE_TIMEOUT).await
}

pub async fn probe_with_timeout(url: &str, geohash: &str, timeout: Duration) -> Verdict {
    let start = Instant::now();
    let (status, text) = match tokio::time::timeout(timeout, read_channel(url, geohash)).await {
        Err(_) => (
            Status::Down,
            format!("no answer within {} s", timeout.as_secs_f32()),
        ),
        Ok(Err(e)) if is_permanent(&*e) => (Status::Dead, format!("dead: {e}")),
        Ok(Err(e)) => (Status::Down, format!("unreachable: {e}")),
        Ok(Ok(outcome)) => {
            let ms = start.elapsed().as_millis();
            match outcome {
                Outcome::Eose { events, challenged } => {
                    let auth = if challenged {
                        ", sends AUTH challenges"
                    } else {
                        ""
                    };
                    (
                        Status::Ok,
                        format!("usable ({ms} ms, {events} recent event(s){auth})"),
                    )
                }
                Outcome::Closed(reason) if reason.contains("auth-required:") => (
                    Status::Auth,
                    format!("AUTH required: unusable by the app ({reason})"),
                ),
                Outcome::Closed(reason) => (Status::Refused, format!("refused: {reason}")),
            }
        }
    };
    Verdict { status, text }
}

/// Probes every relay of the plan that has a watched channel (dead ones
/// excluded), once each, on its first channel; at most `concurrency` at a
/// time. Keyed by relay URL.
pub async fn probe_relays(
    plan: &ListenPlan,
    concurrency: usize,
    timeout: Duration,
) -> BTreeMap<String, Verdict> {
    probe_new_relays(plan, &BTreeMap::new(), concurrency, timeout).await
}

/// Like [`probe_relays`], for the relays without a verdict in `known` only
/// (after a relay list reload).
pub async fn probe_new_relays(
    plan: &ListenPlan,
    known: &BTreeMap<String, Verdict>,
    concurrency: usize,
    timeout: Duration,
) -> BTreeMap<String, Verdict> {
    probe_where(
        plan,
        |relay| !known.contains_key(relay),
        concurrency,
        timeout,
    )
    .await
}

/// Probes the relays of the plan that `wanted` keeps (lot 20: the area's
/// own channel first, the others in the background).
pub async fn probe_where(
    plan: &ListenPlan,
    wanted: impl Fn(&str) -> bool,
    concurrency: usize,
    timeout: Duration,
) -> BTreeMap<String, Verdict> {
    let targets: Vec<(String, String)> = plan
        .relays()
        .into_iter()
        .filter(|relay| wanted(relay))
        .filter_map(|relay| {
            let channel = plan
                .subscriptions(&relay)
                .into_iter()
                .next()
                .and_then(|s: Subscription| s.channels.into_iter().next())?;
            Some((relay, channel))
        })
        .collect();
    stream::iter(targets)
        .map(|(relay, channel)| async move {
            let verdict = probe_with_timeout(&relay, &channel, timeout).await;
            (relay, verdict)
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await
}

/// Failures a retry won't fix. rustls certificate and handshake errors reach
/// us as `Io(InvalidData)`; DNS failures as an uncategorised I/O error.
fn is_permanent(e: &(dyn StdError + 'static)) -> bool {
    e.downcast_ref::<WsError>().is_some_and(is_permanent_ws)
}

pub(crate) fn is_permanent_ws(e: &WsError) -> bool {
    match e {
        WsError::Tls(_) | WsError::Url(_) => true,
        WsError::Http(response) => response.status().is_client_error(),
        WsError::Io(io) => {
            matches!(
                io.kind(),
                ErrorKind::ConnectionRefused | ErrorKind::InvalidData
            ) || io.to_string().contains("failed to lookup address")
        }
        _ => false,
    }
}

enum Outcome {
    Eose { events: usize, challenged: bool },
    Closed(String),
}

const PROBE_LOOKBACK_SECS: i64 = 3600;

async fn read_channel(url: &str, geohash: &str) -> Result<Outcome, Box<dyn StdError>> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await?;
    let filter = json!({
        "kinds": [crate::KIND_GEO_MESSAGE, crate::KIND_GEO_PRESENCE],
        "#g": [geohash],
        "since": unix_now() - PROBE_LOOKBACK_SECS,
        "limit": 200,
    });
    ws.send(Message::text(json!(["REQ", "probe", filter]).to_string()))
        .await?;
    let (mut events, mut challenged) = (0, false);
    while let Some(msg) = ws.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(Value::Array(v)) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let why = |i: usize| v.get(i).and_then(Value::as_str).unwrap_or("").to_owned();
        match v.first().and_then(Value::as_str) {
            Some("EVENT") => events += 1,
            Some("AUTH") => challenged = true,
            Some("EOSE") => {
                let _ = ws.close(None).await;
                return Ok(Outcome::Eose { events, challenged });
            }
            Some("CLOSED") => return Ok(Outcome::Closed(why(2))),
            _ => {}
        }
    }
    Err("connection closed before EOSE".into())
}
