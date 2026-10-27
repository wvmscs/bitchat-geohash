//! Wiring of the parts: control loop, retirement and reinstatement of
//! channels, 24 h relay list reload (lots 7 and 8).
//!
//! [`Monitor::start`] builds the plan, restores the retired channels from the
//! store (D6), probes the relays, then runs one task that:
//! * feeds what the relays deliver to the aggregator;
//! * every tick, retires the channels inactive for the delay (logged at
//!   INFO) and hands the complete minutes to the store;
//! * answers the handle's requests (reinstatement, snapshots, status);
//! * with [`Monitor::start_from_sources`], reloads the relay lists every
//!   `reload_every` (24 h): unchanged lists change nothing; changed ones give
//!   a new plan, new relays are probed, and the connections follow. A failed
//!   download keeps the current list (logged at WARN) until the next cycle.
//!
//! Channels can be added while running ([`Monitor::descend`],
//! [`Monitor::watch_channels`], lot 11 of step 2): they are listened to at
//! once, their new relays are probed in the background, and they are
//! restored at the next start (D12). [`Monitor::subscribe`] tells what
//! changed, for the web server.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::aggregate::{Aggregator, AggregatorConfig, CellStats, MeshStats, Rejection};
use crate::channels::{DEFAULT_MAX_CHANNELS, Level, enumerate, normalize};
use crate::directory::{ListSource, Platform, RelayDirectory, fetch};
use crate::error::{Error, Result};
use crate::listener::{ConnState, Listener, ListenerConfig, ListenerEvent};
use crate::plan::{ListenPlan, PlanConfig};
use crate::probe::{
    PROBE_CONCURRENCY, PROBE_TIMEOUT, Status, Verdict, probe_new_relays, probe_relays,
};
use crate::store::{ChannelState, RelayListLoad, Store, StoreWriter, Write, sha256_hex};

#[derive(Clone)]
pub struct MonitorConfig {
    /// Geohash to watch, and how many channel levels below it.
    pub start: String,
    pub depth: usize,
    pub levels: Vec<Level>,
    pub max_channels: usize,
    pub plan: PlanConfig,
    pub aggregator: AggregatorConfig,
    pub listener: ListenerConfig,
    /// Probe the relays before listening.
    pub probe: bool,
    pub probe_timeout: Duration,
    pub probe_concurrency: usize,
    /// Control loop period: inactivity check and store hand-over.
    pub tick: Duration,
    /// Watch the channels retired in a previous run again.
    pub reinstate_all: bool,
    /// Do not restore the channels added by descents in previous runs, and
    /// forget them.
    pub forget_descents: bool,
    /// Relay list reload period, with [`Monitor::start_from_sources`].
    pub reload_every: Duration,
}

impl MonitorConfig {
    pub fn new(start: &str) -> Self {
        MonitorConfig {
            start: start.to_owned(),
            depth: 1,
            levels: Level::DEFAULT.to_vec(),
            max_channels: DEFAULT_MAX_CHANNELS,
            plan: PlanConfig::default(),
            aggregator: AggregatorConfig::default(),
            listener: ListenerConfig::default(),
            probe: true,
            probe_timeout: PROBE_TIMEOUT,
            probe_concurrency: PROBE_CONCURRENCY,
            tick: Duration::from_secs(5),
            reinstate_all: false,
            forget_descents: false,
            reload_every: Duration::from_secs(24 * 3600),
        }
    }
}

/// Where the relay lists come from; a missing one is not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSources {
    pub ios: Option<ListSource>,
    pub android: Option<ListSource>,
}

impl ListSources {
    /// The official lists of both apps (D5).
    pub fn official() -> Self {
        ListSources {
            ios: Some(ListSource::official(Platform::Ios)),
            android: Some(ListSource::official(Platform::Android)),
        }
    }

    fn each(&self) -> Vec<(Platform, ListSource)> {
        [
            (Platform::Ios, &self.ios),
            (Platform::Android, &self.android),
        ]
        .into_iter()
        .filter_map(|(p, s)| s.clone().map(|s| (p, s)))
        .collect()
    }
}

/// Where the lists in use come from, for the reload.
struct Lists {
    sources: ListSources,
    sha256: BTreeMap<Platform, String>,
}

/// Result of a reload, computed off the control loop.
struct Reloaded {
    loads: Vec<RelayListLoad>,
    failures: Vec<(Platform, String, String)>,
    /// New lists, plan (before retirements and dead relays) and verdicts of
    /// its new relays; `None` when no list changed.
    changed: Option<NewLists>,
}

struct NewLists {
    ios: Option<RelayDirectory>,
    android: Option<RelayDirectory>,
    plan: ListenPlan,
    verdicts: BTreeMap<String, Verdict>,
}

fn load_record(
    platform: Platform,
    source: &ListSource,
    csv: String,
    dir: &RelayDirectory,
    at: i64,
) -> RelayListLoad {
    RelayListLoad {
        platform,
        source: source.describe(),
        loaded_at: at,
        sha256: sha256_hex(&csv),
        relays: dir.len() as u32,
        rejected: dir.rejected() as u32,
        csv,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionStatus {
    pub relay: String,
    pub connection: usize,
    pub state: ConnState,
}

/// What the monitor is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorStatus {
    pub watched: Vec<String>,
    pub retired: Vec<String>,
    /// Relays listened to.
    pub relays: Vec<String>,
    /// Relays set aside (probe or connection: dead or AUTH).
    pub dropped_relays: Vec<String>,
    pub connections: Vec<ConnectionStatus>,
    pub rejected: BTreeMap<Rejection, u64>,
}

/// A relay of the plan, with where it is and how it is doing.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayInfo {
    pub url: String,
    /// Coordinates from the relay lists (IP geolocation).
    pub lat: f64,
    pub lon: f64,
    /// Chosen by each app for at least one channel.
    pub ios: bool,
    pub android: bool,
    /// Probe or connection verdict; `None` when not probed.
    pub status: Option<Status>,
    /// Listened to: alive, with at least one watched channel.
    pub listened: bool,
    /// Watched channels it is chosen for.
    pub channels: u32,
    pub connections: Vec<ConnState>,
}

/// What changed in the monitor, for [`Monitor::subscribe`]. Counters
/// change at every tick; snapshots tell by how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorEvent {
    /// One control loop turn: activity may have changed.
    Tick {
        at: i64,
    },
    /// Channels added by a descent or on request.
    Added(Vec<String>),
    Retired(Vec<String>),
    Reinstated(Vec<String>),
    /// A relay verdict or connection state changed.
    RelaysChanged,
    /// New relay lists gave a new plan.
    PlanChanged,
}

/// Events kept for a slow subscriber before it lags.
const EVENT_CAPACITY: usize = 256;

enum Command {
    Reinstate(Vec<String>, oneshot::Sender<Vec<String>>),
    Watch(Vec<String>, oneshot::Sender<Result<Vec<String>>>),
    Descend(String, oneshot::Sender<Result<Vec<String>>>),
    Relays(oneshot::Sender<Vec<RelayInfo>>),
    ReinstateAll(oneshot::Sender<Vec<String>>),
    Snapshot(oneshot::Sender<Vec<CellStats>>),
    Mesh(usize, oneshot::Sender<Vec<MeshStats>>),
    Status(oneshot::Sender<MonitorStatus>),
    Shutdown,
}

/// Handle on a running monitor.
pub struct Monitor {
    tx: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<MonitorEvent>,
    task: JoinHandle<Result<Store>>,
}

impl Monitor {
    /// Starts watching. The store keeps the retired channels across runs;
    /// it is handed back by [`Monitor::shutdown`].
    pub async fn start(
        config: MonitorConfig,
        ios: Option<RelayDirectory>,
        android: Option<RelayDirectory>,
        store: Store,
    ) -> Result<Monitor> {
        Self::start_inner(config, ios, android, store, None).await
    }

    /// Like [`Monitor::start`], reading the lists from `sources` and
    /// reloading them every `config.reload_every`. A list that cannot be
    /// read falls back to its last valid content in the store (D5); with no
    /// list at all, starting fails.
    pub async fn start_from_sources(
        config: MonitorConfig,
        sources: ListSources,
        store: Store,
    ) -> Result<Monitor> {
        let now = (config.listener.clock)();
        let mut fetched = Vec::new();
        for (platform, source) in sources.each() {
            fetched.push((platform, source.clone(), fetch(&source).await));
        }
        // Database work off the async threads; logging back here.
        let (store, outcomes) = tokio::task::spawn_blocking(move || {
            let mut store = store;
            let mut outcomes = Vec::new();
            for (platform, source, result) in fetched {
                let outcome = match result {
                    Ok((csv, dir)) => {
                        let record = load_record(platform, &source, csv, &dir, now);
                        let sha = record.sha256.clone();
                        store.apply(&[Write::RelayList(record)])?;
                        Ok((dir, sha))
                    }
                    Err(e) => Err((e, store.last_relay_list(platform)?)),
                };
                outcomes.push((platform, source, outcome));
            }
            Ok::<_, Error>((store, outcomes))
        })
        .await
        .map_err(|_| Error::MonitorStopped)??;
        let mut loaded = Vec::new();
        for (platform, source, outcome) in outcomes {
            let (platform_name, source) = (platform.as_str(), source.describe());
            match outcome {
                Ok((dir, sha)) => {
                    tracing::info!(platform = platform_name, %source, relays = dir.len(), "relay list loaded");
                    loaded.push((platform, dir, sha));
                }
                Err((e, Some(last))) => {
                    tracing::warn!(platform = platform_name, %source, error = %e, loaded_at = last.loaded_at, "relay list unavailable, using the last valid one");
                    loaded.push((platform, RelayDirectory::parse(&last.csv), last.sha256));
                }
                Err((e, None)) => {
                    tracing::warn!(platform = platform_name, %source, error = %e, "relay list unavailable, none in the database");
                }
            }
        }
        if loaded.is_empty() {
            return Err(Error::NoRelayList);
        }
        let mut lists = Lists {
            sources,
            sha256: BTreeMap::new(),
        };
        let (mut ios, mut android) = (None, None);
        for (platform, dir, sha) in loaded {
            lists.sha256.insert(platform, sha);
            match platform {
                Platform::Ios => ios = Some(dir),
                Platform::Android => android = Some(dir),
            }
        }
        Self::start_inner(config, ios, android, store, Some(lists)).await
    }

    async fn start_inner(
        config: MonitorConfig,
        ios: Option<RelayDirectory>,
        android: Option<RelayDirectory>,
        store: Store,
        lists: Option<Lists>,
    ) -> Result<Monitor> {
        let clock = config.listener.clock.clone();
        let start = normalize(&config.start)?;
        let mut channels = enumerate(&start, config.depth, &config.levels, config.max_channels)?;

        // Retired channels (D6) and descents (D12) of previous runs, unless
        // asked otherwise.
        let (reinstate_all, forget_descents) = (config.reinstate_all, config.forget_descents);
        let now = clock();
        let (store, previously_retired, descents) = tokio::task::spawn_blocking(move || {
            let mut store = store;
            let descents = if forget_descents {
                let forgotten = store.descents()?;
                store.apply(&[Write::ForgetDescents])?;
                tracing::debug!(count = forgotten.len(), "descents forgotten");
                Vec::new()
            } else {
                store.descents()?
            };
            let retired = store.retired_channels()?;
            if reinstate_all && !retired.is_empty() {
                let writes: Vec<Write> = retired
                    .iter()
                    .map(|g| watched(g, now, "--reinstate-all"))
                    .collect();
                store.apply(&writes)?;
            }
            Ok::<_, Error>((store, retired, descents))
        })
        .await
        .map_err(|_| Error::MonitorStopped)??;
        if forget_descents {
            tracing::info!("channels added by previous descents forgotten (--forget-descents)");
        }
        // Only the descents inside this area, within the cap.
        let known: std::collections::BTreeSet<String> = channels.iter().cloned().collect();
        let restorable: Vec<String> = descents
            .into_iter()
            .filter(|g| g.starts_with(&start) && Level::of(g).is_some() && !known.contains(g))
            .collect();
        let room = config.max_channels.saturating_sub(channels.len());
        if restorable.len() > room {
            tracing::warn!(
                skipped = restorable.len() - room,
                max = config.max_channels,
                "channels of previous descents not restored: over the maximum"
            );
        }
        let restored = restorable.len().min(room);
        if restored > 0 {
            tracing::info!(count = restored, "channels of previous descents restored");
        }
        channels.extend(restorable.into_iter().take(room));
        let mut plan = ListenPlan::build(
            ios.as_ref(),
            android.as_ref(),
            &channels,
            config.plan.clone(),
        )?;
        if reinstate_all {
            for g in &previously_retired {
                tracing::info!(channel = %g, "channel {g} reinstated (--reinstate-all)");
            }
        } else {
            let refs: Vec<&str> = previously_retired.iter().map(String::as_str).collect();
            let kept = plan.retire(&refs);
            tracing::info!(
                count = kept.len(),
                "channels still retired from a previous run"
            );
        }

        let writer = StoreWriter::spawn(store);
        let mut verdicts = BTreeMap::new();
        if config.probe {
            verdicts = probe_relays(&plan, config.probe_concurrency, config.probe_timeout).await;
            let now = clock();
            let mut dropped = Vec::new();
            for (relay, v) in &verdicts {
                writer.send(v.to_write(relay, now))?;
                if !v.status.worth_listening() {
                    tracing::warn!(%relay, verdict = %v.text, "relay set aside");
                    dropped.push(relay.as_str());
                }
            }
            plan.drop_relays(&dropped);
        }

        let mut aggregator = Aggregator::new(config.aggregator.clone());
        let watched = plan.watched();
        aggregator.watch(
            &watched.iter().map(String::as_str).collect::<Vec<_>>(),
            clock(),
        );

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let mut listener = Listener::new(config.listener.clone(), events_tx);
        listener.sync(&plan);
        tracing::info!(
            channels = channels.len(),
            watched = watched.len(),
            relays = plan.relays().len(),
            connections = listener.connection_count(),
            "monitor started"
        );

        let (tx, rx) = mpsc::unbounded_channel();
        let (reload_tx, reload_rx) = mpsc::unbounded_channel();
        let (probed_tx, probed_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let state = State {
            config,
            start,
            ios,
            android,
            channels,
            plan,
            aggregator,
            listener,
            writer,
            verdicts,
            connections: BTreeMap::new(),
            lists,
            reloading: false,
            reload_tx,
            probed_tx,
            events: events.clone(),
        };
        let task = tokio::spawn(state.run(rx, events_rx, reload_rx, probed_rx));
        Ok(Monitor { tx, events, task })
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Result<T> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(make(reply))
            .map_err(|_| Error::MonitorStopped)?;
        answer.await.map_err(|_| Error::MonitorStopped)
    }

    /// Watches retired channels again. Returns those actually reinstated.
    pub async fn reinstate(&self, channels: Vec<String>) -> Result<Vec<String>> {
        self.ask(|r| Command::Reinstate(channels, r)).await
    }

    pub async fn reinstate_all(&self) -> Result<Vec<String>> {
        self.ask(Command::ReinstateAll).await
    }

    /// Watches the channels of the next channel level inside `geohash`
    /// (a channel or a mesh of the watched area): `u09tv` gives its 32
    /// level-6 channels. Level 7 is reachable this way (D1: on request).
    /// Channels already watched are left alone, retired ones reinstated.
    /// Fails over `max_channels` in total, without changing anything.
    /// Returns the channels added or reinstated.
    pub async fn descend(&self, geohash: &str) -> Result<Vec<String>> {
        let g = geohash.to_owned();
        self.ask(|r| Command::Descend(g, r)).await?
    }

    /// Watches these channels (inside the watched area, of a channel
    /// level), like [`Monitor::descend`].
    pub async fn watch_channels(&self, channels: Vec<String>) -> Result<Vec<String>> {
        self.ask(|r| Command::Watch(channels, r)).await?
    }

    /// Every relay of the plan, set aside ones included, in URL order.
    pub async fn relays(&self) -> Result<Vec<RelayInfo>> {
        self.ask(Command::Relays).await
    }

    /// Changes from now on. A receiver too slow for [`EVENT_CAPACITY`]
    /// events gets `Lagged` and should take a full snapshot again.
    pub fn subscribe(&self) -> broadcast::Receiver<MonitorEvent> {
        self.events.subscribe()
    }

    pub async fn snapshot(&self) -> Result<Vec<CellStats>> {
        self.ask(Command::Snapshot).await
    }

    pub async fn mesh(&self, precision: usize) -> Result<Vec<MeshStats>> {
        self.ask(|r| Command::Mesh(precision, r)).await
    }

    pub async fn status(&self) -> Result<MonitorStatus> {
        self.ask(Command::Status).await
    }

    /// Disconnects, commits what is pending and hands the store back.
    pub async fn shutdown(self) -> Result<Store> {
        let _ = self.tx.send(Command::Shutdown);
        self.task.await.map_err(|_| Error::MonitorStopped)?
    }
}

fn watched(geohash: &str, at: i64, reason: &str) -> Write {
    Write::ChannelState {
        geohash: geohash.to_owned(),
        state: ChannelState::Watched,
        changed_at: at,
        reason: Some(reason.to_owned()),
    }
}

struct State {
    config: MonitorConfig,
    /// `config.start`, normalised.
    start: String,
    /// Relay lists in use (descents and reloads compute plans with them).
    ios: Option<RelayDirectory>,
    android: Option<RelayDirectory>,
    /// Every channel, watched or retired (to rebuild the plan on reload).
    channels: Vec<String>,
    plan: ListenPlan,
    aggregator: Aggregator,
    listener: Listener,
    writer: StoreWriter,
    verdicts: BTreeMap<String, Verdict>,
    connections: BTreeMap<(String, usize), ConnState>,
    lists: Option<Lists>,
    reloading: bool,
    reload_tx: mpsc::UnboundedSender<Reloaded>,
    /// Verdicts of relays probed after a descent.
    probed_tx: mpsc::UnboundedSender<BTreeMap<String, Verdict>>,
    events: broadcast::Sender<MonitorEvent>,
}

impl State {
    fn now(&self) -> i64 {
        (self.config.listener.clock)()
    }

    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut events: mpsc::UnboundedReceiver<ListenerEvent>,
        mut reloads: mpsc::UnboundedReceiver<Reloaded>,
        mut probed: mpsc::UnboundedReceiver<BTreeMap<String, Verdict>>,
    ) -> Result<Store> {
        let mut ticker = tokio::time::interval(self.config.tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let every = self.config.reload_every;
        let mut reload_timer = self.lists.is_some().then(|| {
            let mut t = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            t
        });
        loop {
            tokio::select! {
                Some(event) = events.recv() => self.on_event(event),
                _ = ticker.tick() => self.on_tick(),
                _ = async {
                    match &mut reload_timer {
                        Some(t) => t.tick().await,
                        None => std::future::pending().await,
                    }
                } => self.start_reload(),
                Some(reloaded) = reloads.recv() => self.on_reloaded(reloaded),
                Some(verdicts) = probed.recv() => self.on_probed(verdicts),
                command = commands.recv() => match command {
                    None | Some(Command::Shutdown) => break,
                    Some(c) => self.on_command(c),
                },
            }
        }
        let buckets = self.aggregator.drain_buckets(self.now());
        if !buckets.is_empty() {
            self.writer.send(Write::Buckets(buckets))?;
        }
        self.listener.shutdown().await;
        let writer = self.writer;
        tokio::task::spawn_blocking(move || writer.close())
            .await
            .map_err(|_| Error::StoreClosed)?
    }

    /// Fetches the lists and, if one changed, builds the new plan and
    /// probes its new relays, in a separate task.
    fn start_reload(&mut self) {
        let Some(lists) = &self.lists else { return };
        if self.reloading {
            return;
        }
        self.reloading = true;
        let sources = lists.sources.clone();
        let (mut ios, mut android) = (self.ios.clone(), self.android.clone());
        let current = lists.sha256.clone();
        let channels = self.channels.clone();
        let plan_config = self.config.plan.clone();
        let known = self.verdicts.clone();
        let (probe, concurrency, timeout) = (
            self.config.probe,
            self.config.probe_concurrency,
            self.config.probe_timeout,
        );
        let clock = self.config.listener.clock.clone();
        let tx = self.reload_tx.clone();
        tokio::spawn(async move {
            let mut out = Reloaded {
                loads: Vec::new(),
                failures: Vec::new(),
                changed: None,
            };
            let mut changed = false;
            for (platform, source) in sources.each() {
                match fetch(&source).await {
                    Ok((csv, dir)) => {
                        let record = load_record(platform, &source, csv, &dir, clock());
                        if current.get(&platform) != Some(&record.sha256) {
                            changed = true;
                            match platform {
                                Platform::Ios => ios = Some(dir),
                                Platform::Android => android = Some(dir),
                            }
                        }
                        out.loads.push(record);
                    }
                    Err(e) => out
                        .failures
                        .push((platform, source.describe(), e.to_string())),
                }
            }
            if changed {
                match ListenPlan::build(ios.as_ref(), android.as_ref(), &channels, plan_config) {
                    Ok(plan) => {
                        let verdicts = if probe {
                            probe_new_relays(&plan, &known, concurrency, timeout).await
                        } else {
                            BTreeMap::new()
                        };
                        out.changed = Some(NewLists {
                            ios,
                            android,
                            plan,
                            verdicts,
                        });
                    }
                    // Channels were validated at start: cannot happen.
                    Err(e) => tracing::error!(error = %e, "cannot rebuild the listen plan"),
                }
            }
            let _ = tx.send(out);
        });
    }

    fn on_reloaded(&mut self, reloaded: Reloaded) {
        self.reloading = false;
        let Some(lists) = &mut self.lists else { return };
        for (platform, source, error) in &reloaded.failures {
            tracing::warn!(platform = platform.as_str(), %source, %error, "relay list reload failed, keeping the current one");
        }
        for load in &reloaded.loads {
            lists.sha256.insert(load.platform, load.sha256.clone());
            let _ = self.writer.send(Write::RelayList(load.clone()));
        }
        let Some(NewLists {
            ios,
            android,
            mut plan,
            verdicts,
        }) = reloaded.changed
        else {
            if !reloaded.loads.is_empty() {
                tracing::info!("relay lists reloaded: unchanged");
            }
            return;
        };
        self.ios = ios;
        self.android = android;
        // Carry the retirements and the relays set aside over.
        let retired = self.plan.retired();
        plan.retire(&retired.iter().map(String::as_str).collect::<Vec<_>>());
        let now = self.now();
        for (relay, v) in &verdicts {
            let _ = self.writer.send(v.to_write(relay, now));
        }
        self.verdicts.extend(verdicts);
        let dropped: Vec<&str> = self
            .verdicts
            .iter()
            .filter(|(_, v)| !v.status.worth_listening())
            .map(|(r, _)| r.as_str())
            .collect();
        plan.drop_relays(&dropped);
        let diff = self.plan.diff(&plan);
        tracing::info!(
            relays = plan.relays().len(),
            changed_relays = diff.relays.len(),
            "relay lists reloaded: new plan"
        );
        self.plan = plan;
        self.listener.sync(&self.plan);
        self.emit(MonitorEvent::PlanChanged);
    }

    fn emit(&self, event: MonitorEvent) {
        // No subscriber is fine.
        let _ = self.events.send(event);
    }

    /// Adds channels to the plan and listens to them at once; their new
    /// relays are probed in the background (see `on_probed`).
    fn add(&mut self, channels: &[String]) -> Result<Vec<String>> {
        for g in channels {
            if Level::of(g).is_none() {
                return Err(Error::InvalidGeohash(g.clone()));
            }
            if !g.starts_with(&self.start) {
                return Err(Error::OutsideArea {
                    geohash: g.clone(),
                    start: self.start.clone(),
                });
            }
        }
        let fresh: Vec<String> = channels
            .iter()
            .filter(|g| !self.plan.contains(g))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let count = self.plan.len() + fresh.len();
        if count > self.config.max_channels {
            return Err(Error::TooManyChannels {
                count: count as u64,
                max: self.config.max_channels,
            });
        }
        let before: std::collections::BTreeSet<String> =
            self.plan.all_relays().into_iter().collect();
        let added = self
            .plan
            .extend(self.ios.as_ref(), self.android.as_ref(), &fresh)?;
        let now = self.now();
        self.channels.extend(added.iter().cloned());
        self.aggregator
            .watch(&added.iter().map(String::as_str).collect::<Vec<_>>(), now);
        for g in &added {
            let _ = self.writer.send(Write::Descent {
                geohash: g.clone(),
                added_at: now,
            });
        }
        let retired: Vec<&str> = channels
            .iter()
            .map(String::as_str)
            .filter(|g| self.plan.is_retired(g))
            .collect();
        let reinstated = self.reinstate(&retired);
        if !added.is_empty() {
            self.listener.sync(&self.plan);
            self.emit(MonitorEvent::Added(added.clone()));
        }
        let new_relays = self
            .plan
            .all_relays()
            .into_iter()
            .any(|r| !before.contains(&r));
        if self.config.probe && new_relays {
            let (plan, known, tx) = (
                self.plan.clone(),
                self.verdicts.clone(),
                self.probed_tx.clone(),
            );
            let (concurrency, timeout) = (self.config.probe_concurrency, self.config.probe_timeout);
            tokio::spawn(async move {
                let _ = tx.send(probe_new_relays(&plan, &known, concurrency, timeout).await);
            });
        }
        let mut out = added;
        out.extend(reinstated);
        out.sort();
        Ok(out)
    }

    fn descend(&mut self, geohash: &str) -> Result<Vec<String>> {
        let g = normalize(geohash)?;
        if !g.starts_with(&self.start) {
            return Err(Error::OutsideArea {
                geohash: g,
                start: self.start.clone(),
            });
        }
        let next = Level::ALL
            .iter()
            .copied()
            .find(|l| l.precision() > g.len())
            .ok_or_else(|| Error::NoFinerLevel(g.clone()))?;
        let children = enumerate(&g, 1, &[next], self.config.max_channels)?;
        let changed = self.add(&children)?;
        tracing::info!(cell = %g, added = changed.len(), "descended into {g}: {} channels added or reinstated", changed.len());
        Ok(changed)
    }

    /// Verdicts of the relays new after a descent.
    fn on_probed(&mut self, verdicts: BTreeMap<String, Verdict>) {
        let now = self.now();
        let mut dropped = Vec::new();
        for (relay, v) in &verdicts {
            let _ = self.writer.send(v.to_write(relay, now));
            if !v.status.worth_listening() {
                tracing::warn!(%relay, verdict = %v.text, "relay set aside");
                dropped.push(relay.as_str());
            }
        }
        self.plan.drop_relays(&dropped);
        if !dropped.is_empty() {
            self.listener.sync(&self.plan);
        }
        self.verdicts.extend(verdicts);
        self.emit(MonitorEvent::RelaysChanged);
    }

    fn relays(&self) -> Vec<RelayInfo> {
        let listened: std::collections::BTreeSet<String> = self.plan.relays().into_iter().collect();
        let mut out: BTreeMap<String, RelayInfo> = BTreeMap::new();
        for channel in self.plan.watched().iter().chain(&self.plan.retired()) {
            let watched = !self.plan.is_retired(channel);
            for s in self.plan.selection(channel).unwrap_or_default() {
                let r = out.entry(s.relay.url.clone()).or_insert_with(|| RelayInfo {
                    url: s.relay.url.clone(),
                    lat: s.relay.lat,
                    lon: s.relay.lon,
                    ios: false,
                    android: false,
                    status: self.verdicts.get(&s.relay.url).map(|v| v.status),
                    listened: listened.contains(&s.relay.url),
                    channels: 0,
                    connections: self
                        .connections
                        .iter()
                        .filter(|((relay, _), _)| *relay == s.relay.url)
                        .map(|(_, state)| state.clone())
                        .collect(),
                });
                r.ios |= s.ios.is_some();
                r.android |= s.android.is_some();
                r.channels += u32::from(watched);
            }
        }
        out.into_values().collect()
    }

    fn on_event(&mut self, event: ListenerEvent) {
        match event {
            ListenerEvent::Observation(obs) => {
                let now = self.now();
                if let Err(why) = self.aggregator.observe(&obs, now) {
                    tracing::trace!(relay = %obs.relay, ?why, "event not counted");
                }
            }
            ListenerEvent::State {
                relay,
                connection,
                state,
            } => {
                if let ConnState::Abandoned { auth, reason } = &state {
                    let status = if *auth { Status::Auth } else { Status::Dead };
                    let verdict = Verdict {
                        status,
                        text: reason.clone(),
                    };
                    let _ = self.writer.send(verdict.to_write(&relay, self.now()));
                    self.verdicts.insert(relay.clone(), verdict);
                    self.plan.drop_relays(&[relay.as_str()]);
                    self.listener.sync(&self.plan);
                }
                self.connections.insert((relay, connection), state);
                self.emit(MonitorEvent::RelaysChanged);
            }
            ListenerEvent::Closed {
                relay,
                subscription,
                reason,
            } => {
                tracing::debug!(%relay, %subscription, %reason, "subscription closed by the relay")
            }
            ListenerEvent::Notice { relay, text } => tracing::debug!(%relay, %text, "relay notice"),
        }
    }

    fn on_tick(&mut self) {
        let now = self.now();
        let inactive = self.aggregator.inactive(now);
        if !inactive.is_empty() {
            let refs: Vec<&str> = inactive.iter().map(String::as_str).collect();
            let retired = self.plan.retire(&refs);
            self.aggregator.unwatch(&refs);
            let minutes = self.config.aggregator.inactivity_secs / 60;
            for g in &retired {
                tracing::info!(channel = %g, "channel {g} retired from monitoring: inactive for {minutes} min");
                let _ = self.writer.send(Write::ChannelState {
                    geohash: g.clone(),
                    state: ChannelState::Retired,
                    changed_at: now,
                    reason: Some(format!("inactive for {minutes} min")),
                });
            }
            self.listener.sync(&self.plan);
            self.emit(MonitorEvent::Retired(retired));
        }
        let buckets = self.aggregator.drain_buckets(now);
        if !buckets.is_empty() {
            let _ = self.writer.send(Write::Buckets(buckets));
        }
        self.emit(MonitorEvent::Tick { at: now });
    }

    fn reinstate(&mut self, channels: &[&str]) -> Vec<String> {
        let now = self.now();
        let changed = self.plan.reinstate(channels);
        let refs: Vec<&str> = changed.iter().map(String::as_str).collect();
        self.aggregator.watch(&refs, now);
        for g in &changed {
            tracing::info!(channel = %g, "channel {g} reinstated on request");
            let _ = self.writer.send(watched(g, now, "reinstated on request"));
        }
        if !changed.is_empty() {
            self.listener.sync(&self.plan);
            self.emit(MonitorEvent::Reinstated(changed.clone()));
        }
        changed
    }

    fn on_command(&mut self, command: Command) {
        match command {
            Command::Reinstate(channels, reply) => {
                let refs: Vec<&str> = channels.iter().map(String::as_str).collect();
                let _ = reply.send(self.reinstate(&refs));
            }
            Command::ReinstateAll(reply) => {
                let retired = self.plan.retired();
                let refs: Vec<&str> = retired.iter().map(String::as_str).collect();
                let _ = reply.send(self.reinstate(&refs));
            }
            Command::Watch(channels, reply) => {
                let normalized: Result<Vec<String>> =
                    channels.iter().map(|g| normalize(g)).collect();
                let _ = reply.send(normalized.and_then(|c| self.add(&c)));
            }
            Command::Descend(geohash, reply) => {
                let _ = reply.send(self.descend(&geohash));
            }
            Command::Relays(reply) => {
                let _ = reply.send(self.relays());
            }
            Command::Snapshot(reply) => {
                let now = self.now();
                let _ = reply.send(self.aggregator.snapshot(now, &self.plan, &self.verdicts));
            }
            Command::Mesh(precision, reply) => {
                let now = self.now();
                let _ = reply.send(self.aggregator.mesh(precision, now));
            }
            Command::Status(reply) => {
                let _ = reply.send(MonitorStatus {
                    watched: self.plan.watched(),
                    retired: self.plan.retired(),
                    relays: self.plan.relays(),
                    dropped_relays: self.plan.dead_relays(),
                    connections: self
                        .connections
                        .iter()
                        .filter(|((relay, _), _)| self.plan.relays().contains(relay))
                        .map(|((relay, connection), state)| ConnectionStatus {
                            relay: relay.clone(),
                            connection: *connection,
                            state: state.clone(),
                        })
                        .collect(),
                    rejected: self.aggregator.rejected(),
                });
            }
            Command::Shutdown => {}
        }
    }
}
