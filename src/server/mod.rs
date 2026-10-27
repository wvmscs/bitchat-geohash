//! Internal web server of step 2: the map page, the live state pushed to
//! the browser (SSE) and the commands. See `PLAN_ET2.md`.
//!
//! Behind the `server` feature, so that the library can be used without it.
//!
//! [`serve`] starts it around a running [`Monitor`]. One task follows the
//! monitor's notifications and, at most once per `push_every`, computes the
//! deltas once for every browser ([`model::Published`]).

mod api;
mod assets;
mod guard;
pub mod model;
mod tls;

use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

pub use assets::Asset;
pub use tls::TlsFiles;

use crate::error::{Error, Result};
use crate::monitor::{Monitor, MonitorConfig};
use api::{AppState, Live};
use model::{CellView, Params, Published, RelayView, StatusView};

/// The routes of the page and its static files.
pub fn router() -> Router {
    Router::new()
        .route("/", get(|| async { serve_asset("index.html") }))
        .route("/assets/{*path}", get(asset))
}

async fn asset(Path(path): Path<String>) -> Response {
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    match Asset::get(path) {
        Some(a) => (
            [
                (header::CONTENT_TYPE, a.mime),
                (header::CACHE_CONTROL, "no-cache"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            a.bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Server settings.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Default `127.0.0.1:8080`.
    pub listen: SocketAddr,
    /// Required to listen on an address other than a loopback one (D13).
    pub allow_remote: bool,
    /// Certificate and key: HTTPS, HTTP/2 or HTTP/1.1 by ALPN (D17).
    pub tls: Option<TlsFiles>,
    /// Allows `allow_remote` without TLS, with a warning.
    pub insecure_remote: bool,
    /// Tile URL template and its attribution, given to the page.
    pub tiles: String,
    pub attribution: String,
    /// Deltas are computed and sent at most this often (default 1 s).
    pub push_every: Duration,
    /// SSE comment sent on an idle stream (default 15 s).
    pub keep_alive: Duration,
    /// SSE streams open at the same time (default 64).
    pub max_streams: usize,
    /// Request body size, in bytes (default 64 KiB).
    pub max_body: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            allow_remote: false,
            tls: None,
            insecure_remote: false,
            tiles: model::DEFAULT_TILES.to_owned(),
            attribution: model::DEFAULT_ATTRIBUTION.to_owned(),
            push_every: Duration::from_secs(1),
            keep_alive: Duration::from_secs(15),
            max_streams: 64,
            max_body: 64 * 1024,
        }
    }
}

/// A running server; [`Server::shutdown`] stops it and gives the monitor
/// back.
pub struct Server {
    addr: SocketAddr,
    https: bool,
    stop: watch::Sender<bool>,
    http: JoinHandle<std::io::Result<()>>,
    pusher: JoinHandle<()>,
    monitor: Arc<Monitor>,
}

impl Server {
    /// The address actually listened on (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://addr` or `https://addr`, without a trailing slash.
    pub fn url(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{scheme}://{}", self.addr)
    }

    /// Closes the SSE streams, waits for the requests in progress, and
    /// returns the monitor, still running.
    pub async fn shutdown(self) -> Result<Monitor> {
        let _ = self.stop.send(true);
        let _ = self.http.await;
        let _ = self.pusher.await;
        Arc::try_unwrap(self.monitor).map_err(|_| Error::MonitorStopped)
    }
}

/// Starts the server for `monitor`, started with `monitor_config`.
///
/// Refuses a non-loopback address without `allow_remote`, and without TLS
/// unless `insecure_remote`; warns with it: there is no authentication
/// (D13, D17). Unreadable or mismatched TLS files are an error.
pub async fn serve(
    monitor: Monitor,
    monitor_config: &MonitorConfig,
    config: ServerConfig,
) -> Result<Server> {
    if !config.listen.ip().is_loopback() {
        if !config.allow_remote {
            return Err(Error::RemoteListen(config.listen.to_string()));
        }
        if config.tls.is_none() {
            if !config.insecure_remote {
                return Err(Error::RemoteWithoutTls(config.listen.to_string()));
            }
            tracing::warn!(listen = %config.listen, "no TLS on a non-local address: the map and the commands travel in clear");
        }
        tracing::warn!(listen = %config.listen, "listening on a non-local address: anyone who can reach it can see the map and send commands");
    }
    let tls = config.tls.as_ref().map(tls::load).transpose()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    let monitor = Arc::new(monitor);
    let params = Params::new(monitor_config, &config.tiles, &config.attribution);
    let (tx, _) = broadcast::channel(64);
    let live = Arc::new(Live {
        published: Mutex::new(Published::new(params)),
        tx,
    });
    // The first `hello` already has the state.
    refresh(&monitor, &live).await?;

    let (stop, stop_rx) = watch::channel(false);
    let state = AppState {
        monitor: monitor.clone(),
        live: live.clone(),
        streams: Arc::new(AtomicUsize::new(0)),
        max_streams: config.max_streams,
        keep_alive: config.keep_alive,
        stop: stop_rx.clone(),
    };
    let guard = Arc::new(guard::Guard::new(
        addr,
        &config.tiles,
        tls.as_ref().map(|t| t.leaf.clone()),
    ));
    let app = Router::new()
        .route("/api/state", get(api::state))
        .route("/api/events", get(api::events))
        .route("/api/descend", post(api::descend))
        .route("/api/preview", post(api::preview))
        .route("/api/reinstate", post(api::reinstate))
        .with_state(state)
        .merge(router())
        .layer(DefaultBodyLimit::max(config.max_body))
        .layer(axum::middleware::from_fn_with_state(guard, guard::guard));

    let https = tls.is_some();
    let http = match tls {
        Some(tls) => tokio::spawn(tls::serve(listener, tls.acceptor, app, stop_rx.clone())),
        None => {
            let mut http_stop = stop_rx.clone();
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = http_stop.wait_for(|s| *s).await;
                    })
                    .await
            })
        }
    };
    let pusher = tokio::spawn(push_loop(monitor.clone(), live, config.push_every, stop_rx));
    let scheme = if https { "https" } else { "http" };
    tracing::info!("map on {scheme}://{addr}/");
    Ok(Server {
        addr,
        https,
        stop,
        http,
        pusher,
        monitor,
    })
}

/// Follows the monitor's notifications; at most once per `every`, when
/// something happened, publishes the deltas.
async fn push_loop(
    monitor: Arc<Monitor>,
    live: Arc<Live>,
    every: Duration,
    mut stop: watch::Receiver<bool>,
) {
    let mut events = monitor.subscribe();
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut dirty = false;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            event = events.recv() => match event {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => dirty = true,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = tick.tick() => {
                if dirty {
                    dirty = false;
                    if let Err(e) = refresh(&monitor, &live).await {
                        tracing::warn!(error = %e, "cannot read the monitor state, live updates stopped");
                        break;
                    }
                }
            }
        }
    }
}

/// Reads the monitor state and sends what changed.
async fn refresh(monitor: &Monitor, live: &Live) -> Result<()> {
    let cells: Vec<CellView> = monitor
        .snapshot()
        .await?
        .iter()
        .map(CellView::from)
        .collect();
    let relays: Vec<RelayView> = monitor
        .relays()
        .await?
        .iter()
        .map(RelayView::from)
        .collect();
    let status = StatusView::from(&monitor.status().await?);
    let mut published = live.published.lock().expect("not poisoned");
    for push in published.update(cells, relays, status) {
        // No stream open: nobody to send to.
        let _ = live.tx.send(Arc::new(push));
    }
    Ok(())
}
