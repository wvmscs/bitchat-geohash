// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

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

use super::area::Holder;
use super::model::{Published, Push};
use crate::error::Error;

/// The state sent to the browsers: the last one published, and the channel
/// its deltas go out on.
pub(crate) struct Live {
    pub(crate) published: Mutex<Published>,
    pub(crate) tx: broadcast::Sender<Arc<Push>>,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) holder: Arc<Holder>,
    pub(crate) live: Arc<Live>,
    pub(crate) streams: Arc<AtomicUsize>,
    pub(crate) max_streams: usize,
    pub(crate) keep_alive: Duration,
    pub(crate) stop: watch::Receiver<bool>,
    #[cfg(feature = "geocode")]
    pub(crate) places: Option<Arc<crate::geocode::Places>>,
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
            Error::AreaFixed => ApiError::new(StatusCode::CONFLICT, "area_fixed", message),
            Error::PlaceNotFound => {
                ApiError::new(StatusCode::NOT_FOUND, "place_not_found", message)
            }
            Error::Geocode(_) => ApiError::new(StatusCode::BAD_GATEWAY, "geocode", message),
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
    let added = s
        .holder
        .get()
        .await?
        .descend(&body.geohash)
        .await
        .inspect_err(|e| {
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
    let added = s
        .holder
        .get()
        .await?
        .preview(&body.geohash)
        .await
        .inspect_err(|e| {
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
        s.holder.get().await?.reinstate_all().await?
    } else if !body.channels.is_empty() {
        tracing::info!(channels = body.channels.len(), "command: reinstate");
        s.holder.get().await?.reinstate(body.channels).await?
    } else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "expected {\"channels\": [...]} or {\"all\": true}",
        ));
    };
    Ok(Json(json!({ "reinstated": back })))
}

/// `POST /api/area {"geohash": "u09t"}`: watches that area instead (lot 19).
/// Answers once the new area is watched; the pages get a new `hello`.
/// With places (lot 23), `"geohash"` may also be `#u09t`, a place or an IP
/// address: the answer then has the `place` found. A typed text is not
/// logged, only the cell it gives.
pub(crate) async fn area(
    State(s): State<AppState>,
    body: Result<Json<DescendBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    #[cfg(feature = "geocode")]
    let (geohash, place) = match &s.places {
        Some(places) => {
            let (min, max) = crate::geocode::area_bounds(&s.holder.config().levels);
            let r = places
                .resolve(&body.geohash, min, max)
                .await
                .inspect_err(|e| {
                    tracing::info!(error = %e, "area: nothing found for the text typed");
                })?;
            (r.geohash, r.place)
        }
        None => (body.geohash, None),
    };
    #[cfg(not(feature = "geocode"))]
    let (geohash, place) = (body.geohash, None::<()>);
    tracing::info!(%geohash, "command: area");
    let config = s.holder.change(&geohash).await.inspect_err(|e| {
        tracing::info!(%geohash, error = %e, "area change refused");
    })?;
    Ok(Json(
        json!({ "start": config.start, "depth": config.depth, "place": place }),
    ))
}

#[cfg(feature = "geocode")]
#[derive(Deserialize)]
pub(crate) struct PlaceQuery {
    geohash: String,
}

/// `GET /api/place?geohash=u09t` (lot 23): the approximate name of a cell,
/// `{"label", "source", "attribution"}`, or `{"place": null}` when nothing
/// is named there (open sea). Asked once per cell.
#[cfg(feature = "geocode")]
pub(crate) async fn place(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<PlaceQuery>,
) -> Result<Json<Value>, ApiError> {
    let places = s
        .places
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no_places", "place names are off"))?;
    let place = places.name(&q.geohash).await?;
    Ok(Json(json!({ "geohash": q.geohash, "place": place })))
}
