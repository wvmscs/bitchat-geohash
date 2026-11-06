// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Internal web server of step 2: the map page, the live state pushed to
//! the browser (SSE) and the commands. See `PLAN_ET2.md`.
//!
//! Behind the `server` feature, so that the library can be used without it.
//!
//! [`serve`] starts it around a running [`Monitor`]. One task follows the
//! monitor's notifications and, at most once per `push_every`, computes the
//! deltas once for every browser ([`model::Published`]). The views, the
//! commands and that task are `crate::service`'s, shared with the browser
//! build; here, HTTP, TLS, the proxy and the guard.

mod api;
mod assets;
mod guard;
mod proxy;
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

pub use crate::service::Relaunch;
/// The JSON views sent to the page (in `service`, shared with the browser
/// build).
pub use crate::service::model;
pub use assets::Asset;
pub use proxy::{Net, ProxyConfig};
pub use tls::TlsFiles;

use crate::error::{Error, Result};
use crate::monitor::{Monitor, MonitorConfig};
use crate::service::api::Live;
use crate::service::area::Holder;
use crate::service::{push_loop, refresh};
use api::AppState;
use model::{Params, Published};

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
    /// Behind a TLS proxy (D25): clear HTTP from the proxy only; no `tls`.
    pub proxy: Option<ProxyConfig>,
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
    /// Places for the area field and the name of the area (lot 23): none,
    /// geohashes only.
    #[cfg(feature = "geocode")]
    pub places: Option<Arc<crate::geocode::Places>>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            allow_remote: false,
            tls: None,
            insecure_remote: false,
            proxy: None,
            tiles: model::DEFAULT_TILES.to_owned(),
            attribution: model::DEFAULT_ATTRIBUTION.to_owned(),
            push_every: Duration::from_secs(1),
            keep_alive: Duration::from_secs(15),
            max_streams: 64,
            max_body: 64 * 1024,
            #[cfg(feature = "geocode")]
            places: None,
        }
    }
}

/// A running server; [`Server::shutdown`] stops it and gives the monitor
/// back.
pub struct Server {
    addr: SocketAddr,
    https: bool,
    public: Option<String>,
    stop: watch::Sender<bool>,
    http: JoinHandle<std::io::Result<()>>,
    pusher: JoinHandle<()>,
    holder: Arc<Holder>,
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

    /// Behind a proxy, the public origin (`https://name`); the browser
    /// goes there, not to [`Server::url`].
    pub fn public_url(&self) -> Option<&str> {
        self.public.as_deref()
    }

    /// Closes the SSE streams, waits for the requests in progress, and
    /// returns the monitor, still running.
    pub async fn shutdown(self) -> Result<Monitor> {
        let _ = self.stop.send(true);
        let _ = self.http.await;
        let _ = self.pusher.await;
        self.holder.take().await
    }
}

/// Starts the server for `monitor`, started with `monitor_config`; the
/// watched area cannot change ([`serve_with`] for that).
pub async fn serve(
    monitor: Monitor,
    monitor_config: &MonitorConfig,
    config: ServerConfig,
) -> Result<Server> {
    serve_with(monitor, monitor_config, None, config).await
}

/// Starts the server for `monitor`, started with `monitor_config`. With
/// `relaunch`, `POST /api/area` changes the watched area (lot 19).
///
/// Refuses a non-loopback address without `allow_remote`, and without TLS
/// unless `insecure_remote` or behind a proxy; warns with it: there is no
/// authentication (D13, D17). Unreadable or mismatched TLS files are an
/// error. Behind a proxy (D25), the public name must lead here, unless
/// `ProxyConfig::check` is off.
pub async fn serve_with(
    monitor: Monitor,
    monitor_config: &MonitorConfig,
    relaunch: Option<Relaunch>,
    config: ServerConfig,
) -> Result<Server> {
    if config.proxy.is_some() && config.tls.is_some() {
        return Err(Error::Proxy(
            "TLS is the proxy's: no certificate here".into(),
        ));
    }
    if !config.listen.ip().is_loopback() {
        if !config.allow_remote {
            return Err(Error::RemoteListen(config.listen.to_string()));
        }
        if config.tls.is_none() && config.proxy.is_none() {
            if !config.insecure_remote {
                return Err(Error::RemoteWithoutTls(config.listen.to_string()));
            }
            tracing::warn!(listen = %config.listen, "no TLS on a non-local address: the map and the commands travel in clear");
        }
        tracing::warn!(listen = %config.listen, "listening on a non-local address: anyone who can reach it can see the map and send commands");
    }
    let behind = config
        .proxy
        .as_ref()
        .map(|p| {
            Ok::<_, Error>(guard::Behind {
                public: proxy::Public::parse(&p.public)?,
                from: p.from.clone(),
                token: proxy::token(),
            })
        })
        .transpose()?;
    let probe = config
        .proxy
        .as_ref()
        .filter(|p| p.check)
        .zip(behind.as_ref())
        .map(|(_, b)| (b.public.origin.clone(), b.token.clone()));
    let public = behind.as_ref().map(|b| b.public.origin.clone());
    let tls = config.tls.as_ref().map(tls::load).transpose()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    let holder = Arc::new(Holder::new(monitor, monitor_config.clone(), relaunch));
    // A public map keeps its strategy: one visitor would change it for all.
    let strategy_change = config.proxy.is_none() && config.listen.ip().is_loopback();
    #[allow(unused_mut)]
    let mut params = Params::new(monitor_config, &config.tiles, &config.attribution);
    params.strategy_change = strategy_change;
    #[cfg(feature = "geocode")]
    {
        params.geocode = config.places.is_some();
    }
    let (tx, _) = broadcast::channel(64);
    let live = Arc::new(Live {
        published: Mutex::new(Published::new(params)),
        tx,
    });
    // The first `hello` already has the state.
    refresh(&*holder.get().await?, &live).await?;

    let (stop, stop_rx) = watch::channel(false);
    let state = AppState {
        holder: holder.clone(),
        live: live.clone(),
        streams: Arc::new(AtomicUsize::new(0)),
        max_streams: config.max_streams,
        keep_alive: config.keep_alive,
        stop: stop_rx.clone(),
        #[cfg(feature = "geocode")]
        places: config.places.clone(),
        strategy_change,
    };
    let guard = Arc::new(guard::Guard::new(
        addr,
        &config.tiles,
        tls.as_ref().map(|t| t.leaf.clone()),
        behind,
    ));
    let app = Router::new()
        .route("/api/state", get(api::state))
        .route("/api/events", get(api::events))
        .route("/api/descend", post(api::descend))
        .route("/api/preview", post(api::preview))
        .route("/api/reinstate", post(api::reinstate))
        .route("/api/area", post(api::area))
        .route("/api/strategy", post(api::strategy));
    #[cfg(feature = "geocode")]
    let app = app.route("/api/place", get(api::place));
    let app = app
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
                // The peer address: behind a proxy, only the proxy's (D25).
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    let _ = http_stop.wait_for(|s| *s).await;
                })
                .await
            })
        }
    };
    let pusher = tokio::spawn(push_loop(holder.clone(), live, config.push_every, stop_rx));
    let scheme = if https { "https" } else { "http" };
    tracing::info!("map on {scheme}://{addr}/");
    let server = Server {
        addr,
        https,
        public,
        stop,
        http,
        pusher,
        holder,
    };
    if let Some((origin, token)) = probe {
        if let Err(e) = proxy::probe(&origin, &token).await {
            let _ = server.shutdown().await;
            return Err(e);
        }
        tracing::info!("map behind the proxy on {origin}/ (checked)");
    }
    Ok(server)
}
