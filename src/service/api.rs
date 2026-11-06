// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! `/api/*` without HTTP: the answers and the commands, for the web server
//! (`server::api` wraps them in axum) and for the browser build, whose page
//! sends the same requests to [`handle`].

use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::area::Holder;
use super::model::{Published, Push};
use crate::error::Error;

/// HTTP status codes of the answers.
pub(crate) mod status {
    pub const OK: u16 = 200;
    pub const BAD_REQUEST: u16 = 400;
    pub const FORBIDDEN: u16 = 403;
    pub const NOT_FOUND: u16 = 404;
    pub const METHOD_NOT_ALLOWED: u16 = 405;
    pub const CONFLICT: u16 = 409;
    pub const UNPROCESSABLE_ENTITY: u16 = 422;
    pub const INTERNAL_SERVER_ERROR: u16 = 500;
    pub const BAD_GATEWAY: u16 = 502;
    pub const SERVICE_UNAVAILABLE: u16 = 503;
}

/// The state sent to the browsers: the last one published, and the channel
/// its deltas go out on.
pub(crate) struct Live {
    pub(crate) published: Mutex<Published>,
    pub(crate) tx: broadcast::Sender<Arc<Push>>,
}

impl Live {
    /// The full state, as in `hello`.
    pub(crate) fn hello(&self) -> Push {
        self.published.lock().expect("not poisoned").hello()
    }

    /// `hello`, and the deltas after it: under the lock, so that no delta
    /// falls between them.
    pub(crate) fn subscribe(&self) -> (Push, broadcast::Receiver<Arc<Push>>) {
        let published = self.published.lock().expect("not poisoned");
        (published.hello(), self.tx.subscribe())
    }
}

/// An error answer: `{"error": code, "message": ..., ...details}`.
#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl ApiError {
    pub(crate) fn new(status: impl Into<u16>, code: &str, message: impl Into<String>) -> Self {
        ApiError {
            status: status.into(),
            body: json!({ "error": code, "message": message.into() }),
        }
    }

    fn with(mut self, key: &str, value: Value) -> Self {
        self.body[key] = value;
        self
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        use status::*;
        let message = e.to_string();
        match e {
            Error::InvalidGeohash(g) => {
                ApiError::new(BAD_REQUEST, "invalid_geohash", message).with("geohash", json!(g))
            }
            Error::TooManyChannels { count, max } => {
                ApiError::new(CONFLICT, "too_many_channels", message)
                    .with("count", json!(count))
                    .with("max", json!(max))
            }
            Error::OutsideArea { geohash, start } => {
                ApiError::new(UNPROCESSABLE_ENTITY, "outside_area", message)
                    .with("geohash", json!(geohash))
                    .with("start", json!(start))
            }
            Error::NoFinerLevel(g) => {
                ApiError::new(UNPROCESSABLE_ENTITY, "no_finer_level", message)
                    .with("geohash", json!(g))
            }
            Error::MonitorStopped => ApiError::new(SERVICE_UNAVAILABLE, "monitor_stopped", message),
            Error::AreaFixed => ApiError::new(CONFLICT, "area_fixed", message),
            Error::StrategyFixed => ApiError::new(CONFLICT, "strategy_fixed", message),
            Error::UnknownStrategy(name) => ApiError::new(BAD_REQUEST, "unknown_strategy", message)
                .with("strategy", json!(name)),
            Error::PlaceNotFound => ApiError::new(NOT_FOUND, "place_not_found", message),
            Error::Geocode(_) => ApiError::new(BAD_GATEWAY, "geocode", message),
            _ => ApiError::new(INTERNAL_SERVER_ERROR, "internal", message),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct DescendBody {
    pub(crate) geohash: String,
}

#[derive(Deserialize)]
pub(crate) struct ReinstateBody {
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
pub(crate) struct StrategyBody {
    strategy: String,
}

/// `POST /api/descend {"geohash": "u09tv"}`.
pub(crate) async fn descend(holder: &Holder, body: DescendBody) -> Result<Value, ApiError> {
    tracing::info!(geohash = %body.geohash, "command: descend");
    let added = holder
        .get()
        .await?
        .descend(&body.geohash)
        .await
        .inspect_err(|e| {
            tracing::info!(geohash = %body.geohash, error = %e, "descend refused");
        })?;
    Ok(json!({ "added": added }))
}

/// `POST /api/preview {"geohash": "u09tv"}`: listens for a while to the
/// channels below (D21).
pub(crate) async fn preview(holder: &Holder, body: DescendBody) -> Result<Value, ApiError> {
    tracing::debug!(geohash = %body.geohash, "command: preview");
    let added = holder
        .get()
        .await?
        .preview(&body.geohash)
        .await
        .inspect_err(|e| {
            tracing::info!(geohash = %body.geohash, error = %e, "preview refused");
        })?;
    Ok(json!({ "added": added }))
}

/// `POST /api/reinstate {"channels": [...]}` or `{"all": true}`.
pub(crate) async fn reinstate(holder: &Holder, body: ReinstateBody) -> Result<Value, ApiError> {
    let back = if body.all {
        tracing::info!("command: reinstate all");
        holder.get().await?.reinstate_all().await?
    } else if !body.channels.is_empty() {
        tracing::info!(channels = body.channels.len(), "command: reinstate");
        holder.get().await?.reinstate(body.channels).await?
    } else {
        return Err(ApiError::new(
            status::BAD_REQUEST,
            "bad_request",
            "expected {\"channels\": [...]} or {\"all\": true}",
        ));
    };
    Ok(json!({ "reinstated": back }))
}

/// Tells the pages which area is being changed to (`None`: no more).
pub(crate) fn announce_area(live: &Live, changing: Option<String>) {
    let mut published = live.published.lock().expect("not poisoned");
    if let Some(push) = published.set_changing(changing) {
        let _ = live.tx.send(Arc::new(push));
    }
}

/// `POST /api/area`, once the text typed is a geohash: watches that area
/// instead (lot 19). Answers once the new area is watched; the pages get a
/// new `hello`. `place`: what the text typed named, if anything.
pub(crate) async fn area(
    holder: &Holder,
    live: &Live,
    geohash: String,
    place: Value,
) -> Result<Value, ApiError> {
    tracing::info!(%geohash, "command: area");
    // Every page shows the change at once, whoever asked (the console over
    // the map, another tab), not only when the new area is there (lot 25).
    let config = holder
        .change(&geohash, |area| announce_area(live, Some(area.to_owned())))
        .await
        .inspect_err(|e| {
            tracing::info!(%geohash, error = %e, "area change refused");
            announce_area(live, None);
        })?;
    Ok(json!({ "start": config.start, "depth": config.depth, "place": place }))
}

/// `POST /api/strategy {"strategy": "a1"}`: listens with that relay strategy
/// instead (lot 27, D28). Answers once the monitor runs with it; the pages
/// get a new `hello`. Refused when `allowed` is false (a public map: one
/// visitor would change what every other one sees).
pub(crate) async fn strategy(
    holder: &Holder,
    allowed: bool,
    body: StrategyBody,
) -> Result<Value, ApiError> {
    if !allowed {
        return Err(ApiError::new(
            status::FORBIDDEN,
            "strategy_fixed",
            "this map is public: its relay strategy does not change from the page",
        ));
    }
    let kind = crate::strategy::StrategyKind::parse(&body.strategy)?;
    tracing::info!(strategy = %kind, "command: strategy");
    let config = holder.change_strategy(kind).await?;
    Ok(json!({ "strategy": config.strategy.name() }))
}

/// What [`handle`] needs: the monitor and the published state.
pub(crate) struct Service {
    pub(crate) holder: Arc<Holder>,
    pub(crate) live: Arc<Live>,
    pub(crate) strategy_change: bool,
}

/// The body as axum's `Json` reads it, with its answers: a body of the wrong
/// shape is 422, one that is not JSON 400.
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|e| {
        if e.classify() == serde_json::error::Category::Data {
            ApiError::new(
                status::UNPROCESSABLE_ENTITY,
                "bad_request",
                format!("Failed to deserialize the JSON body into the target type: {e}"),
            )
        } else {
            ApiError::new(
                status::BAD_REQUEST,
                "bad_request",
                format!("Failed to parse the request body as JSON: {e}"),
            )
        }
    })
}

/// Answers a request of the page, without HTTP: `(status, JSON body)`.
/// `GET /api/events` is not here: the stream is [`Live::subscribe`].
pub(crate) async fn handle(
    service: &Service,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, Value) {
    let path = path.split('?').next().unwrap_or_default();
    let holder = &*service.holder;
    let answer = match (method, path) {
        ("GET", "/api/state") => {
            Ok(serde_json::to_value(service.live.hello()).expect("plain data serialises"))
        }
        ("POST", "/api/descend") => match parse(body) {
            Ok(b) => descend(holder, b).await,
            Err(e) => Err(e),
        },
        ("POST", "/api/preview") => match parse(body) {
            Ok(b) => preview(holder, b).await,
            Err(e) => Err(e),
        },
        ("POST", "/api/reinstate") => match parse(body) {
            Ok(b) => reinstate(holder, b).await,
            Err(e) => Err(e),
        },
        ("POST", "/api/area") => match parse::<DescendBody>(body) {
            Ok(b) => area(holder, &service.live, b.geohash, Value::Null).await,
            Err(e) => Err(e),
        },
        ("POST", "/api/strategy") => match parse(body) {
            Ok(b) => strategy(holder, service.strategy_change, b).await,
            Err(e) => Err(e),
        },
        (
            _,
            "/api/state" | "/api/descend" | "/api/preview" | "/api/reinstate" | "/api/area"
            | "/api/strategy",
        ) => Err(ApiError::new(
            status::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "method not allowed",
        )),
        _ => Err(ApiError::new(
            status::NOT_FOUND,
            "not_found",
            "no such route",
        )),
    };
    match answer {
        Ok(body) => (status::OK, body),
        Err(e) => (e.status, e.body),
    }
}
