//! Event deduplication, sliding window, inactivity detection and per-cell
//! counters (lot 6).
//!
//! Times are Unix seconds given by the caller (injected clock). Pubkeys are
//! kept in memory only while their events are inside the window; nothing
//! that leaves this module ([`CellStats`], [`MeshStats`], [`CellBucket`])
//! carries one (spec §5).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::nostr::Event;

use crate::channels::Level;
use crate::plan::ListenPlan;
use crate::probe::{Status, Verdict};
use crate::store::CellBucket;
use crate::{KIND_GEO_MESSAGE, KIND_GEO_PRESENCE};

#[derive(Debug, Clone, PartialEq)]
pub struct AggregatorConfig {
    /// Counting window, on `created_at` (default 15 min).
    pub window_secs: i64,
    /// A watched channel without any event for this long is inactive
    /// (default 15 min).
    pub inactivity_secs: i64,
    /// At level 6, fewer participants than this are shown as "low", without
    /// a number. Default 0: every count is shown (D20).
    pub k_threshold: u32,
}

impl Default for AggregatorConfig {
    fn default() -> Self {
        AggregatorConfig {
            window_secs: 15 * 60,
            inactivity_secs: 15 * 60,
            k_threshold: 0,
        }
    }
}

/// An event received from a relay.
#[derive(Debug, Clone)]
pub struct Observation {
    pub relay: String,
    pub event: Event,
}

/// Why an event was not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rejection {
    /// Already counted (same id from another relay or again).
    Duplicate,
    BadSignature,
    /// Not kind 20000 or 20001.
    WrongKind,
    /// No `g` tag.
    NoChannel,
    /// `g` tag of a channel that is not watched (outside the plan, or retired).
    NotWatched,
    /// `created_at` older than the window.
    OutOfWindow,
}

/// One relay chosen for a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellRelay {
    pub url: String,
    /// Rank in the iOS choice.
    pub ios: Option<usize>,
    /// Ranks in the Android choice: two when it picks the server twice.
    pub android: Vec<usize>,
    /// Probe status; `None` when not probed.
    pub status: Option<Status>,
    /// Whether this relay delivered at least one event of the cell.
    pub delivered: bool,
}

/// Counters of one channel over the window. No pubkey, by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellStats {
    pub geohash: String,
    /// Geohash length (2, 4, 5, 6 or 7).
    pub level: u8,
    pub watched: bool,
    /// Listened to for a while only, before a possible descent (D21).
    pub preview: bool,
    pub messages: u32,
    pub presences: u32,
    /// Distinct participants; `None` at level 6 below the k threshold, when
    /// one is set ("low activity", no number).
    pub participants: Option<u32>,
    /// Latest `created_at` seen, window or not.
    pub last_seen: Option<i64>,
    pub relays: Vec<CellRelay>,
    /// Relays chosen by only one of the two apps.
    pub divergence: Vec<String>,
}

/// Activity of the channels inside a mesh (a geohash prefix, precision 3
/// included), distinct from the mesh's own channel activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshStats {
    pub geohash: String,
    /// Channels inside the mesh with at least one event in the window.
    pub active_channels: u32,
    pub messages: u32,
    pub presences: u32,
    /// Distinct participants over those channels; `None` below the k
    /// threshold when the mesh is at level 6 or finer.
    pub participants: Option<u32>,
}

struct Seen {
    created_at: i64,
    kind: u32,
    pubkey: String,
}

#[derive(Default)]
struct Cell {
    /// Start of the current observation (watch or reinstatement).
    watched_since: Option<i64>,
    /// Latest activity, `created_at` capped at reception time.
    last_activity: Option<i64>,
    last_seen: Option<i64>,
    /// Events inside the window, in arrival order.
    events: VecDeque<Seen>,
    delivered: BTreeSet<String>,
    /// Minutes with new events not yet handed to the store.
    dirty_minutes: BTreeSet<i64>,
}

impl Cell {
    fn watched(&self) -> bool {
        self.watched_since.is_some()
    }

    fn counts(&self, from: i64, to: i64) -> (u32, u32, u32) {
        let (mut messages, mut presences) = (0, 0);
        let mut people = HashSet::new();
        for e in self
            .events
            .iter()
            .filter(|e| e.created_at >= from && e.created_at < to)
        {
            if e.kind == KIND_GEO_MESSAGE {
                messages += 1;
            } else {
                presences += 1;
            }
            people.insert(e.pubkey.as_str());
        }
        (messages, presences, people.len() as u32)
    }
}

/// Counts what relays deliver, per channel.
pub struct Aggregator {
    config: AggregatorConfig,
    cells: BTreeMap<String, Cell>,
    /// Counted event ids -> (channel, `created_at`), to note duplicates'
    /// relays and to drop ids with the window.
    ids: HashMap<String, (String, i64)>,
    rejected: BTreeMap<Rejection, u64>,
}

impl Aggregator {
    pub fn new(config: AggregatorConfig) -> Self {
        Aggregator {
            config,
            cells: BTreeMap::new(),
            ids: HashMap::new(),
            rejected: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> &AggregatorConfig {
        &self.config
    }

    /// Starts (or restarts, after a reinstatement) observing channels: the
    /// inactivity delay counts from `now`.
    pub fn watch(&mut self, channels: &[&str], now: i64) {
        for &c in channels {
            self.cells.entry(c.to_owned()).or_default().watched_since = Some(now);
        }
    }

    /// Stops counting events of these channels (retired). Counters already
    /// in the window are kept until they leave it.
    pub fn unwatch(&mut self, channels: &[&str]) {
        for &c in channels {
            if let Some(cell) = self.cells.get_mut(c) {
                cell.watched_since = None;
            }
        }
    }

    /// Forgets channels that left the plan (end of a preview), counters
    /// included.
    pub fn forget(&mut self, channels: &[&str]) {
        for &c in channels {
            self.cells.remove(c);
        }
    }

    /// Counts an event, or says why not.
    pub fn observe(&mut self, obs: &Observation, now: i64) -> Result<(), Rejection> {
        let result = self.try_observe(obs, now);
        if let Err(r) = result {
            *self.rejected.entry(r).or_default() += 1;
        }
        result
    }

    fn try_observe(&mut self, obs: &Observation, now: i64) -> Result<(), Rejection> {
        let e = &obs.event;
        if let Some((channel, _)) = self.ids.get(&e.id) {
            // Same id, so same content: only note who delivered it.
            if let Some(cell) = self.cells.get_mut(channel) {
                cell.delivered.insert(obs.relay.clone());
            }
            return Err(Rejection::Duplicate);
        }
        if e.kind != KIND_GEO_MESSAGE && e.kind != KIND_GEO_PRESENCE {
            return Err(Rejection::WrongKind);
        }
        let channel = e.tag_value("g").ok_or(Rejection::NoChannel)?;
        if !self.cells.get(channel).is_some_and(Cell::watched) {
            return Err(Rejection::NotWatched);
        }
        if e.created_at <= now - self.config.window_secs {
            return Err(Rejection::OutOfWindow);
        }
        e.verify().map_err(|_| Rejection::BadSignature)?;

        let channel = channel.to_owned();
        let cell = self.cells.get_mut(&channel).expect("checked above");
        cell.events.push_back(Seen {
            created_at: e.created_at,
            kind: e.kind,
            pubkey: e.pubkey.clone(),
        });
        cell.delivered.insert(obs.relay.clone());
        let activity = e.created_at.min(now);
        cell.last_activity = cell.last_activity.max(Some(activity));
        cell.last_seen = cell.last_seen.max(Some(e.created_at));
        cell.dirty_minutes.insert(minute(e.created_at));
        self.ids.insert(e.id.clone(), (channel, e.created_at));
        Ok(())
    }

    /// Rejected events so far, by reason.
    pub fn rejected(&self) -> BTreeMap<Rejection, u64> {
        self.rejected.clone()
    }

    /// Drops events, pubkeys and ids that left the window.
    pub fn purge(&mut self, now: i64) {
        let from = now - self.config.window_secs;
        for cell in self.cells.values_mut() {
            cell.events.retain(|e| e.created_at > from);
        }
        self.ids.retain(|_, (_, created_at)| *created_at > from);
    }

    /// Watched channels without any event for `inactivity_secs`, counted
    /// from the latest event or, if none, from the start of observation.
    pub fn inactive(&self, now: i64) -> Vec<String> {
        self.cells
            .iter()
            .filter_map(|(c, cell)| {
                let since = cell.watched_since?;
                let last = cell.last_activity.map_or(since, |a| a.max(since));
                (now - last >= self.config.inactivity_secs).then(|| c.clone())
            })
            .collect()
    }

    /// Stats of every channel of the plan, in geohash order.
    pub fn snapshot(
        &mut self,
        now: i64,
        plan: &ListenPlan,
        verdicts: &BTreeMap<String, Verdict>,
    ) -> Vec<CellStats> {
        self.purge(now);
        let empty = Cell::default();
        let mut channels = plan.watched();
        channels.extend(plan.retired());
        channels.sort();
        channels
            .into_iter()
            .map(|geohash| {
                let cell = self.cells.get(&geohash).unwrap_or(&empty);
                let (messages, presences, participants) = cell.counts(i64::MIN, i64::MAX);
                let selection = plan.selection(&geohash).unwrap_or_default();
                let relays = selection
                    .iter()
                    .map(|s| CellRelay {
                        url: s.relay.url.clone(),
                        ios: s.ios,
                        android: s.android.clone(),
                        status: verdicts.get(&s.relay.url).map(|v| v.status),
                        delivered: cell.delivered.contains(&s.relay.url),
                    })
                    .collect();
                let divergence = plan
                    .divergence(&geohash)
                    .into_iter()
                    .map(|s| s.relay.url)
                    .collect();
                CellStats {
                    level: geohash.len() as u8,
                    watched: !plan.is_retired(&geohash),
                    preview: false,
                    messages,
                    presences,
                    participants: self.shown(geohash.len(), participants),
                    last_seen: cell.last_seen,
                    relays,
                    divergence,
                    geohash,
                }
            })
            .collect()
    }

    /// Activity inside each mesh of `precision` (3 included) that contains
    /// at least one longer channel with events in the window.
    pub fn mesh(&mut self, precision: usize, now: i64) -> Vec<MeshStats> {
        self.purge(now);
        struct Acc<'a> {
            active: u32,
            messages: u32,
            presences: u32,
            people: HashSet<&'a str>,
        }
        let mut meshes: BTreeMap<&str, Acc> = BTreeMap::new();
        for (geohash, cell) in &self.cells {
            if geohash.len() <= precision || cell.events.is_empty() {
                continue;
            }
            let acc = meshes.entry(&geohash[..precision]).or_insert_with(|| Acc {
                active: 0,
                messages: 0,
                presences: 0,
                people: HashSet::new(),
            });
            acc.active += 1;
            for e in &cell.events {
                if e.kind == KIND_GEO_MESSAGE {
                    acc.messages += 1;
                } else {
                    acc.presences += 1;
                }
                acc.people.insert(&e.pubkey);
            }
        }
        meshes
            .into_iter()
            .map(|(geohash, acc)| MeshStats {
                geohash: geohash.to_owned(),
                active_channels: acc.active,
                messages: acc.messages,
                presences: acc.presences,
                participants: self.shown(precision, acc.people.len() as u32),
            })
            .collect()
    }

    /// Per-minute counters of complete minutes with new events since the
    /// previous call, for the store. A late event in an already handed
    /// minute (still inside the window) hands that minute again, recounted.
    pub fn drain_buckets(&mut self, now: i64) -> Vec<CellBucket> {
        self.purge(now);
        let mut out = Vec::new();
        for (geohash, cell) in &mut self.cells {
            let complete: Vec<i64> = cell
                .dirty_minutes
                .iter()
                .copied()
                .take_while(|m| m + 60 <= now)
                .collect();
            for m in complete {
                cell.dirty_minutes.remove(&m);
                let (messages, presences, participants) = cell.counts(m, m + 60);
                if messages + presences > 0 {
                    out.push(CellBucket {
                        geohash: geohash.clone(),
                        bucket_start: m,
                        messages,
                        presences,
                        participants,
                    });
                }
            }
        }
        out
    }

    /// Hides small participant counts at level 6 and finer.
    fn shown(&self, precision: usize, participants: u32) -> Option<u32> {
        let hidden = precision >= Level::Neighbourhood.precision()
            && participants > 0
            && participants < self.config.k_threshold;
        (!hidden).then_some(participants)
    }
}

fn minute(t: i64) -> i64 {
    t.div_euclid(60) * 60
}
