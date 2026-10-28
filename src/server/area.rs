//! Changing the watched area while the map runs (lot 19): the monitor is
//! stopped and started again on the new area with the same database, and
//! every open page gets a new `hello`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{RwLock, watch};

use crate::directory::RelayDirectory;
use crate::error::{Error, Result};
use crate::monitor::{ListSources, Monitor, MonitorConfig, check_area};
use crate::store::Store;

/// How to start the monitor again on another area.
pub enum Relaunch {
    /// The relay lists from these sources, as at the start.
    Sources { sources: ListSources, db: String },
    /// These lists, fixed (tests, examples).
    Fixed {
        ios: Option<RelayDirectory>,
        android: Option<RelayDirectory>,
        db: String,
    },
}

impl Relaunch {
    fn db(&self) -> &str {
        match self {
            Relaunch::Sources { db, .. } | Relaunch::Fixed { db, .. } => db,
        }
    }

    async fn start(&self, config: MonitorConfig, store: Store) -> Result<Monitor> {
        match self {
            Relaunch::Sources { sources, .. } => {
                Monitor::start_from_sources(config, sources.clone(), store).await
            }
            Relaunch::Fixed { ios, android, .. } => {
                Monitor::start(config, ios.clone(), android.clone(), store).await
            }
        }
    }
}

/// The running monitor, which an area change replaces.
pub(crate) struct Holder {
    /// `None` while the area changes.
    monitor: RwLock<Option<Arc<Monitor>>>,
    config: Mutex<MonitorConfig>,
    relaunch: Option<Relaunch>,
    /// One change at a time.
    changing: tokio::sync::Mutex<()>,
    /// Bumped at each new monitor: the push loop follows the new one.
    pub(crate) generation: watch::Sender<u64>,
}

/// How long a change waits for the requests in progress to let go of the
/// old monitor.
const RELEASE: Duration = Duration::from_secs(10);

impl Holder {
    pub(crate) fn new(monitor: Monitor, config: MonitorConfig, relaunch: Option<Relaunch>) -> Self {
        Holder {
            monitor: RwLock::new(Some(Arc::new(monitor))),
            config: Mutex::new(config),
            relaunch,
            changing: tokio::sync::Mutex::new(()),
            generation: watch::channel(0).0,
        }
    }

    /// The monitor; `MonitorStopped` while the area changes.
    pub(crate) async fn get(&self) -> Result<Arc<Monitor>> {
        self.monitor
            .read()
            .await
            .clone()
            .ok_or(Error::MonitorStopped)
    }

    pub(crate) fn config(&self) -> MonitorConfig {
        self.config.lock().expect("not poisoned").clone()
    }

    /// Takes the monitor out, once nobody else holds it.
    pub(crate) async fn take(&self) -> Result<Monitor> {
        let shared = self
            .monitor
            .write()
            .await
            .take()
            .ok_or(Error::MonitorStopped)?;
        let mut shared = shared;
        let until = tokio::time::Instant::now() + RELEASE;
        loop {
            match Arc::try_unwrap(shared) {
                Ok(m) => return Ok(m),
                Err(back) if tokio::time::Instant::now() < until => {
                    shared = back;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(back) => {
                    *self.monitor.write().await = Some(back);
                    return Err(Error::MonitorStopped);
                }
            }
        }
    }

    /// Watches `geohash` instead of the current area. Returns the new
    /// configuration; unchanged when it is already the area.
    pub(crate) async fn change(&self, geohash: &str) -> Result<MonitorConfig> {
        let _one = self.changing.lock().await;
        let relaunch = self.relaunch.as_ref().ok_or(Error::AreaFixed)?;
        let old = self.config();
        let mut config = old.clone();
        config.start = geohash.to_owned();
        config.start = check_area(&config)?;
        if config.start == old.start {
            return Ok(old);
        }
        // Not for a second time: they were applied at the start.
        config.reinstate_all = false;
        config.forget_descents = false;
        let store = self.take().await?.shutdown().await?;
        tracing::info!(from = %old.start, to = %config.start, "watched area changes");
        let (monitor, config) = match relaunch.start(config.clone(), store).await {
            Ok(m) => (m, config),
            Err(e) => {
                // The store went with the failed start: open it again and
                // go back to the previous area.
                tracing::warn!(error = %e, area = %config.start, "cannot watch the new area, back to the previous one");
                let store = Store::open(relaunch.db())?;
                let mut back = old.clone();
                (back.reinstate_all, back.forget_descents) = (false, false);
                let m = relaunch.start(back, store).await?;
                *self.monitor.write().await = Some(Arc::new(m));
                self.generation.send_modify(|g| *g += 1);
                return Err(e);
            }
        };
        *self.monitor.write().await = Some(Arc::new(monitor));
        *self.config.lock().expect("not poisoned") = config.clone();
        self.generation.send_modify(|g| *g += 1);
        Ok(config)
    }
}
