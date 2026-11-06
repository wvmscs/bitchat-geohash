// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The map's service without its transport: what the browser receives
//! (`model`), the `/api/*` answers and commands (`api`), the area change
//! (`area`), and the task that publishes the deltas. The web server
//! (feature `server`) carries it over HTTP and SSE; the browser build hands
//! it the page's requests directly.

// The browser build has one thread: its monitor need not be `Send`.
#![cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]

pub(crate) mod api;
pub(crate) mod area;
pub mod model;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, watch};

pub use area::Relaunch;

use crate::error::Result;
use crate::monitor::{Monitor, MonitorConfig};
use crate::rt::{self, JoinHandle};
use api::Live;
use area::Holder;
use model::{CellView, MESH_LEVELS, MeshView, Params, Published, Push, RelayView, StatusView};

/// Settings of a [`Service`].
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Tile URL template and its attribution, given to the page.
    pub tiles: String,
    pub attribution: String,
    /// Deltas are computed and sent at most this often (default 1 s).
    pub push_every: Duration,
    /// The page may change the relay strategy (default yes: one viewer).
    pub strategy_change: bool,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        ServiceConfig {
            tiles: model::DEFAULT_TILES.to_owned(),
            attribution: model::DEFAULT_ATTRIBUTION.to_owned(),
            push_every: Duration::from_secs(1),
            strategy_change: true,
        }
    }
}

/// The map's service without a web server: the page's requests go to
/// [`Service::handle`], its event stream comes from [`Service::events`]
/// (the browser build).
pub struct Service {
    inner: api::Service,
    stop: watch::Sender<bool>,
    pusher: JoinHandle<()>,
}

/// One message of the event stream, as SSE would carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// `hello`, `delta`, ...
    pub event: String,
    pub id: u64,
    /// JSON.
    pub data: String,
}

impl From<&Push> for Message {
    fn from(push: &Push) -> Self {
        Message {
            event: push.event().to_owned(),
            id: push.seq(),
            data: push.to_json(),
        }
    }
}

/// One page's event stream: `hello`, then the deltas.
pub struct Events {
    hello: Option<Message>,
    rx: broadcast::Receiver<Arc<Push>>,
}

impl Events {
    /// The next message; `None` once the stream is closed (it fell behind,
    /// or the service stopped): the page opens a new one and gets a new
    /// `hello`.
    pub async fn next(&mut self) -> Option<Message> {
        if let Some(hello) = self.hello.take() {
            return Some(hello);
        }
        match self.rx.recv().await {
            Ok(push) => Some(Message::from(&*push)),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(missed = n, "event stream behind, closed");
                None
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

impl Service {
    /// Starts publishing the state of `monitor`, started with
    /// `monitor_config`. With `relaunch`, `POST /api/area` and
    /// `/api/strategy` start it again with another area or strategy.
    pub async fn start(
        monitor: Monitor,
        monitor_config: &MonitorConfig,
        relaunch: Option<Relaunch>,
        config: ServiceConfig,
    ) -> Result<Service> {
        let holder = Arc::new(Holder::new(monitor, monitor_config.clone(), relaunch));
        let mut params = Params::new(monitor_config, &config.tiles, &config.attribution);
        params.strategy_change = config.strategy_change;
        let (tx, _) = broadcast::channel(64);
        let live = Arc::new(Live {
            published: Mutex::new(Published::new(params)),
            tx,
        });
        // The first `hello` already has the state.
        refresh(&*holder.get().await?, &live).await?;
        let (stop, stop_rx) = watch::channel(false);
        let pusher = rt::spawn(push_loop(
            holder.clone(),
            live.clone(),
            config.push_every,
            stop_rx,
        ));
        Ok(Service {
            inner: api::Service {
                holder,
                live,
                strategy_change: config.strategy_change,
            },
            stop,
            pusher,
        })
    }

    /// Answers a request of the page (`GET /api/state`, `POST
    /// /api/descend`, ...): `(HTTP status, JSON body)`, as the web server
    /// would.
    pub async fn handle(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        let (status, body) = api::handle(&self.inner, method, path, body).await;
        (status, body.to_string())
    }

    /// A new event stream (`GET /api/events`).
    pub fn events(&self) -> Events {
        let (hello, rx) = self.inner.live.subscribe();
        Events {
            hello: Some(Message::from(&hello)),
            rx,
        }
    }

    /// Stops publishing and returns the monitor, still running.
    pub async fn shutdown(self) -> Result<Monitor> {
        let _ = self.stop.send(true);
        let _ = self.pusher.await;
        self.inner.holder.take().await
    }
}

/// Follows the monitor's notifications; at most once per `every`, when
/// something happened, publishes the deltas. After an area change, follows
/// the new monitor. It never keeps the monitor between two uses, so that an
/// area change can take it.
pub(crate) async fn push_loop(
    holder: Arc<Holder>,
    live: Arc<Live>,
    every: Duration,
    mut stop: watch::Receiver<bool>,
) {
    let mut generation = holder.generation.subscribe();
    let mut tick = rt::interval(every);
    'monitor: loop {
        generation.mark_unchanged();
        let mut events = match holder.get().await {
            Ok(m) => m.subscribe(),
            // Changing: wait for the new one, and start the pages over
            // from it (otherwise they would only get its deltas).
            Err(_) => {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = generation.changed() => {
                        if let Ok(m) = holder.get().await
                            && let Err(e) = reset(&m, &holder.config(), &live).await
                        {
                            tracing::warn!(error = %e, "cannot read the new monitor state");
                            api::announce_area(&live, None);
                        }
                        continue 'monitor;
                    }
                }
            }
        };
        let mut dirty = false;
        loop {
            tokio::select! {
                _ = stop.changed() => break 'monitor,
                _ = generation.changed() => {
                    // A new area: every page starts over from a `hello`.
                    if let Ok(m) = holder.get().await
                        && let Err(e) = reset(&m, &holder.config(), &live).await
                    {
                        tracing::warn!(error = %e, "cannot read the new monitor state");
                    }
                    continue 'monitor;
                }
                event = events.recv() => match event {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => dirty = true,
                    // The old monitor of an area change: the new one comes.
                    Err(broadcast::error::RecvError::Closed) => {
                        tokio::select! {
                            _ = stop.changed() => break 'monitor,
                            _ = generation.changed() => {
                                if let Ok(m) = holder.get().await
                                    && let Err(e) = reset(&m, &holder.config(), &live).await
                                {
                                    tracing::warn!(error = %e, "cannot read the new monitor state");
                                    api::announce_area(&live, None);
                                }
                                continue 'monitor;
                            }
                        }
                    }
                },
                _ = tick.tick() => {
                    if dirty {
                        dirty = false;
                        let done = match holder.get().await {
                            Ok(m) => refresh(&m, &live).await,
                            Err(_) => Ok(()),
                        };
                        if let Err(e) = done {
                            tracing::warn!(error = %e, "cannot read the monitor state");
                        }
                    }
                }
            }
        }
    }
}

/// What the browsers are sent of a monitor.
async fn read(
    monitor: &Monitor,
) -> Result<(Vec<CellView>, Vec<RelayView>, Vec<MeshView>, StatusView)> {
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
    let mut meshes = Vec::new();
    for precision in MESH_LEVELS {
        meshes.extend(monitor.mesh(precision).await?.iter().map(MeshView::from));
    }
    let status = StatusView::from(&monitor.status().await?);
    Ok((cells, relays, meshes, status))
}

/// After an area change: a new published state, sent whole (`hello`) to
/// the open streams.
async fn reset(monitor: &Monitor, config: &MonitorConfig, live: &Live) -> Result<()> {
    let (cells, relays, meshes, status) = read(monitor).await?;
    let mut published = live.published.lock().expect("not poisoned");
    let old = published.params();
    let mut params = Params::new(config, &old.tiles, &old.attribution);
    params.geocode = old.geocode;
    params.strategy_change = old.strategy_change;
    // The sequence goes on: a page that missed the `hello` sees a gap and
    // reconnects.
    let seq = published.seq();
    // Another change may have started since: the new `hello` says so.
    let changing = published
        .changing()
        .filter(|g| *g != config.start)
        .map(str::to_owned);
    *published = Published::starting_at(params, seq);
    published.set_changing(changing);
    published.update(cells, relays, status);
    published.update_meshes(meshes);
    let _ = live.tx.send(Arc::new(published.hello()));
    Ok(())
}

/// Reads the monitor state and sends what changed.
pub(crate) async fn refresh(monitor: &Monitor, live: &Live) -> Result<()> {
    let (cells, relays, meshes, status) = read(monitor).await?;
    let mut published = live.published.lock().expect("not poisoned");
    let mut pushes = published.update(cells, relays, status);
    pushes.extend(published.update_meshes(meshes));
    for push in pushes {
        // No stream open: nobody to send to.
        let _ = live.tx.send(Arc::new(push));
    }
    Ok(())
}
