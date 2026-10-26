//! Wiring of the parts: control loop, retirement and reinstatement of
//! channels, 24 h relay list reload (lots 7 and 8).
//!
//! [`Monitor::start`] builds the plan, restores the retired channels from the
//! store (D6), probes the relays, then runs one task that:
//! * feeds what the relays deliver to the aggregator;
//! * every tick, retires the channels inactive for the delay (logged at
//!   INFO) and hands the complete minutes to the store;
//! * answers the handle's requests (reinstatement, snapshots, status).

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::aggregate::{Aggregator, AggregatorConfig, CellStats, MeshStats, Rejection};
use crate::channels::{DEFAULT_MAX_CHANNELS, Level, enumerate};
use crate::directory::RelayDirectory;
use crate::error::{Error, Result};
use crate::listener::{ConnState, Listener, ListenerConfig, ListenerEvent};
use crate::plan::{ListenPlan, PlanConfig};
use crate::probe::{PROBE_CONCURRENCY, PROBE_TIMEOUT, Status, Verdict, probe_relays};
use crate::store::{ChannelState, Store, StoreWriter, Write};

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
        }
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

enum Command {
    Reinstate(Vec<String>, oneshot::Sender<Vec<String>>),
    ReinstateAll(oneshot::Sender<Vec<String>>),
    Snapshot(oneshot::Sender<Vec<CellStats>>),
    Mesh(usize, oneshot::Sender<Vec<MeshStats>>),
    Status(oneshot::Sender<MonitorStatus>),
    Shutdown,
}

/// Handle on a running monitor.
pub struct Monitor {
    tx: mpsc::UnboundedSender<Command>,
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
        let clock = config.listener.clock.clone();
        let channels = enumerate(
            &config.start,
            config.depth,
            &config.levels,
            config.max_channels,
        )?;
        let mut plan = ListenPlan::build(
            ios.as_ref(),
            android.as_ref(),
            &channels,
            config.plan.clone(),
        )?;

        // Retired channels of previous runs (D6), unless asked otherwise.
        let reinstate_all = config.reinstate_all;
        let now = clock();
        let (store, previously_retired) = tokio::task::spawn_blocking(move || {
            let mut store = store;
            let retired = store.retired_channels()?;
            if reinstate_all && !retired.is_empty() {
                let writes: Vec<Write> = retired
                    .iter()
                    .map(|g| watched(g, now, "--reinstate-all"))
                    .collect();
                store.apply(&writes)?;
            }
            Ok::<_, Error>((store, retired))
        })
        .await
        .map_err(|_| Error::MonitorStopped)??;
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
        let state = State {
            config,
            plan,
            aggregator,
            listener,
            writer,
            verdicts,
            connections: BTreeMap::new(),
        };
        let task = tokio::spawn(state.run(rx, events_rx));
        Ok(Monitor { tx, task })
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
    plan: ListenPlan,
    aggregator: Aggregator,
    listener: Listener,
    writer: StoreWriter,
    verdicts: BTreeMap<String, Verdict>,
    connections: BTreeMap<(String, usize), ConnState>,
}

impl State {
    fn now(&self) -> i64 {
        (self.config.listener.clock)()
    }

    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut events: mpsc::UnboundedReceiver<ListenerEvent>,
    ) -> Result<Store> {
        let mut ticker = tokio::time::interval(self.config.tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                Some(event) = events.recv() => self.on_event(event),
                _ = ticker.tick() => self.on_tick(),
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
        }
        let buckets = self.aggregator.drain_buckets(now);
        if !buckets.is_empty() {
            let _ = self.writer.send(Write::Buckets(buckets));
        }
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
