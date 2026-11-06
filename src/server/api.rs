// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! `/api/*` over HTTP: state, SSE stream and commands. The answers and the
//! commands are `crate::service::api`'s.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::{broadcast, watch};

pub(crate) use crate::service::api::ApiError;
use crate::service::api::{self as commands, DescendBody, Live, ReinstateBody, StrategyBody};
use crate::service::area::Holder;
use crate::service::model::Push;

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
    /// The page may change the relay strategy (lot 27): not on a public map.
    pub(crate) strategy_change: bool,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.body)).into_response()
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
    Json(s.live.hello()).into_response()
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
    let (hello, rx) = s.live.subscribe();
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

/// `POST /api/descend {"geohash": "u09tv"}`.
pub(crate) async fn descend(
    State(s): State<AppState>,
    body: Result<Json<DescendBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    commands::descend(&s.holder, body).await.map(Json)
}

/// `POST /api/preview {"geohash": "u09tv"}`: listens for a while to the
/// channels below (D21).
pub(crate) async fn preview(
    State(s): State<AppState>,
    body: Result<Json<DescendBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    commands::preview(&s.holder, body).await.map(Json)
}

/// `POST /api/reinstate {"channels": [...]}` or `{"all": true}`.
pub(crate) async fn reinstate(
    State(s): State<AppState>,
    body: Result<Json<ReinstateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    commands::reinstate(&s.holder, body).await.map(Json)
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
            (r.geohash, serde_json::json!(r.place))
        }
        None => (body.geohash, Value::Null),
    };
    #[cfg(not(feature = "geocode"))]
    let (geohash, place) = (body.geohash, Value::Null);
    commands::area(&s.holder, &s.live, geohash, place)
        .await
        .map(Json)
}

#[cfg(feature = "geocode")]
#[derive(serde::Deserialize)]
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
    Ok(Json(
        serde_json::json!({ "geohash": q.geohash, "place": place }),
    ))
}

/// `POST /api/strategy {"strategy": "a1"}`: listens with that relay strategy
/// instead (lot 27, D28). Answers once the monitor runs with it; the pages
/// get a new `hello`. Refused on a public map: one visitor would change
/// what every other one sees.
pub(crate) async fn strategy(
    State(s): State<AppState>,
    body: Result<Json<StrategyBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body?;
    commands::strategy(&s.holder, s.strategy_change, body)
        .await
        .map(Json)
}
