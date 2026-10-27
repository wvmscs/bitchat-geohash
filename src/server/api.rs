//! `/api/*`: state, SSE stream and commands.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use super::model::{Published, Push};
use crate::error::Error;
use crate::monitor::Monitor;

/// The state sent to the browsers: the last one published, and the channel
/// its deltas go out on.
pub(crate) struct Live {
    pub(crate) published: Mutex<Published>,
    pub(crate) tx: broadcast::Sender<Arc<Push>>,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) monitor: Arc<Monitor>,
    pub(crate) live: Arc<Live>,
    pub(crate) streams: Arc<AtomicUsize>,
    pub(crate) max_streams: usize,
    pub(crate) keep_alive: Duration,
    pub(crate) stop: watch::Receiver<bool>,
}

/// An error answer: `{"error": code, "message": ..., ...details}`.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    body: Value,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            body: json!({ "error": code, "message": message.into() }),
        }
    }

    fn with(mut self, key: &str, value: Value) -> Self {
        self.body[key] = value;
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let message = e.to_string();
        match e {
            Error::InvalidGeohash(g) => {
                ApiError::new(StatusCode::BAD_REQUEST, "invalid_geohash", message)
                    .with("geohash", json!(g))
            }
            Error::TooManyChannels { count, max } => {
                ApiError::new(StatusCode::CONFLICT, "too_many_channels", message)
                    .with("count", json!(count))
                    .with("max", json!(max))
            }
            Error::OutsideArea { geohash, start } => {
                ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "outside_area", message)
                    .with("geohash", json!(geohash))
                    .with("start", json!(start))
            }
            Error::NoFinerLevel(g) => {
                ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "no_finer_level", message)
                    .with("geohash", json!(g))
            }
            Error::MonitorStopped => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "monitor_stopped", message)
            }
            _ => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        let status = r.status();
        let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
            "too_large"
        } else {
            "bad_request"
        };
        ApiError::new(status, code, r.body_text())
    }
}

/// `GET /api/state`: the full state, as in `hello`.
pub(crate) async fn state(State(s): State<AppState>) -> Response {
    let hello = s.live.published.lock().expect("not poisoned").hello();
    Json(hello).into_response()
}

/// Frees a stream slot when the stream ends.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn sse_event(push: &Push) -> Result<SseEvent, Infallible> {
    Ok(SseEvent::default()
        .event(push.event())
        .id(push.seq().to_string())
        .data(push.to_json()))
}

/// `GET /api/events`: `hello`, then the deltas. A stream that falls behind
/// is closed: the browser reconnects and gets a new `hello`.
pub(crate) async fn events(State(s): State<AppState>) -> Response {
    if s.streams.fetch_add(1, Ordering::SeqCst) >= s.max_streams {
        s.streams.fetch_sub(1, Ordering::SeqCst);
        tracing::warn!(max = s.max_streams, "SSE stream refused: too many");
        return ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "too_many_streams",
            "too many open streams (several tabs?)",
        )
        .into_response();
    }
    let slot = Slot(s.streams.clone());
    // Under the lock, so that no delta falls between `hello` and the
    // subscription.
    let (hello, rx) = {
        let published = s.live.published.lock().expect("not poisoned");
        (published.hello(), s.live.tx.subscribe())
    };
    let first = futures_util::stream::once(std::future::ready(sse_event(&hello)));
    let rest = futures_util::stream::unfold(
        (rx, s.stop.clone(), slot),
        |(mut rx, mut stop, slot)| async move {
            if *stop.borrow() {
                return None;
            }
            tokio::select! {
                push = rx.recv() => match push {
                    Ok(push) => Some((sse_event(&push), (rx, stop, slot))),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(missed = n, "SSE stream behind, closed");
                        None
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                },
                _ = stop.changed() => None,
            }
        },
    );
    Sse::new(first.chain(rest))
        .keep_alive(KeepAlive::new().interval(s.keep_alive))
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct DescendBody {
    geohash: String,
}

/// `POST /api/descend {"geohash": "u09tv"}`.
pub(crate) async fn descend(
    State(s): State<AppState>,
    body: Result<Json<DescendBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    tracing::info!(geohash = %body.geohash, "command: descend");
    let added = s.monitor.descend(&body.geohash).await.inspect_err(|e| {
        tracing::info!(geohash = %body.geohash, error = %e, "descend refused");
    })?;
    Ok(Json(json!({ "added": added })))
}

/// `POST /api/preview {"geohash": "u09tv"}`: listens for a while to the
/// channels below (D21).
pub(crate) async fn preview(
    State(s): State<AppState>,
    body: Result<Json<DescendBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    tracing::debug!(geohash = %body.geohash, "command: preview");
    let added = s.monitor.preview(&body.geohash).await.inspect_err(|e| {
        tracing::info!(geohash = %body.geohash, error = %e, "preview refused");
    })?;
    Ok(Json(json!({ "added": added })))
}

#[derive(Deserialize)]
pub(crate) struct ReinstateBody {
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    all: bool,
}

/// `POST /api/reinstate {"channels": [...]}` or `{"all": true}`.
pub(crate) async fn reinstate(
    State(s): State<AppState>,
    body: Result<Json<ReinstateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    let back = if body.all {
        tracing::info!("command: reinstate all");
        s.monitor.reinstate_all().await?
    } else if !body.channels.is_empty() {
        tracing::info!(channels = body.channels.len(), "command: reinstate");
        s.monitor.reinstate(body.channels).await?
    } else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "expected {\"channels\": [...]} or {\"all\": true}",
        ));
    };
    Ok(Json(json!({ "reinstated": back })))
}
