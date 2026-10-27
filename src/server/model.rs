//! What the browser receives (lot 12): views of the monitor state, the SSE
//! messages, and the deltas between two states (D11).
//!
//! The library types (`CellStats`, `RelayInfo`, ...) have no `serde`: each
//! view is built field by field, so that a field added there never reaches
//! the browser unnoticed. Counts only, never a pubkey, a nickname or a
//! message content (invariant 1).

use std::collections::BTreeMap;

use serde::{Serialize, Serializer};

use crate::aggregate::{CellRelay, CellStats, MeshStats, Rejection};
use crate::listener::ConnState;
use crate::monitor::{MonitorConfig, MonitorStatus, RelayInfo};

/// Default map tiles, as in `app.js`.
pub const DEFAULT_TILES: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
pub const DEFAULT_ATTRIBUTION: &str =
    "&copy; <a href=\"https://www.openstreetmap.org/copyright\">OpenStreetMap</a> contributors";

/// A participant count, or `"low"` below the k threshold (level 6 and
/// finer, only when a threshold is set: none by default, D20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Value(u32),
    Low,
}

impl Serialize for Count {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Count::Value(n) => s.serialize_u32(*n),
            Count::Low => s.serialize_str("low"),
        }
    }
}

/// `[[south, west], [north, east]]`, as Leaflet takes it.
pub type Bbox = [[f64; 2]; 2];

/// Bounding box of a geohash cell. Cells come from the plan, hence valid;
/// an invalid one gets an empty box.
pub fn bbox(geohash: &str) -> Bbox {
    match geohash::decode_bbox(geohash) {
        Ok(r) => [[r.min().y, r.min().x], [r.max().y, r.max().x]],
        Err(_) => [[0.0; 2]; 2],
    }
}

fn participants(p: Option<u32>) -> Count {
    p.map_or(Count::Low, Count::Value)
}

/// One channel (cell) of the plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellView {
    pub geohash: String,
    pub level: u8,
    pub bbox: Bbox,
    pub watched: bool,
    /// Listened to for a while only, before a possible descent (D21).
    pub preview: bool,
    pub messages: u32,
    pub presences: u32,
    pub participants: Count,
    /// Unix time of the latest event.
    pub last_seen: Option<i64>,
    pub relays: Vec<CellRelayView>,
    /// Relays chosen by only one of the two apps.
    pub divergence: Vec<String>,
}

impl From<&CellStats> for CellView {
    fn from(c: &CellStats) -> Self {
        CellView {
            geohash: c.geohash.clone(),
            level: c.level,
            bbox: bbox(&c.geohash),
            watched: c.watched,
            preview: c.preview,
            messages: c.messages,
            presences: c.presences,
            participants: participants(c.participants),
            last_seen: c.last_seen,
            relays: c.relays.iter().map(CellRelayView::from).collect(),
            divergence: c.divergence.clone(),
        }
    }
}

/// A relay chosen for a cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CellRelayView {
    pub url: String,
    /// Rank in the iOS choice.
    pub ios: Option<usize>,
    /// Ranks in the Android choice (two when it picks the server twice).
    pub android: Vec<usize>,
    /// Probe or connection status (`ok`, `auth`, ...); `null` when unknown.
    pub status: Option<&'static str>,
    /// Delivered at least one event of the cell.
    pub delivered: bool,
}

impl From<&CellRelay> for CellRelayView {
    fn from(r: &CellRelay) -> Self {
        CellRelayView {
            url: r.url.clone(),
            ios: r.ios,
            android: r.android.clone(),
            status: r.status.map(|s| s.as_str()),
            delivered: r.delivered,
        }
    }
}

/// Activity of the channels inside a mesh, distinct from the mesh's own
/// channel (lot 16).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeshView {
    pub geohash: String,
    pub level: u8,
    pub bbox: Bbox,
    pub active_channels: u32,
    pub messages: u32,
    pub presences: u32,
    pub participants: Count,
}

impl From<&MeshStats> for MeshView {
    fn from(m: &MeshStats) -> Self {
        MeshView {
            geohash: m.geohash.clone(),
            level: m.geohash.len() as u8,
            bbox: bbox(&m.geohash),
            active_channels: m.active_channels,
            messages: m.messages,
            presences: m.presences,
            participants: participants(m.participants),
        }
    }
}

/// A relay of the plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RelayView {
    pub url: String,
    pub lat: f64,
    pub lon: f64,
    pub ios: bool,
    pub android: bool,
    pub status: Option<&'static str>,
    pub listened: bool,
    pub channels: u32,
    pub connections: Vec<ConnView>,
}

impl From<&RelayInfo> for RelayView {
    fn from(r: &RelayInfo) -> Self {
        RelayView {
            url: r.url.clone(),
            lat: r.lat,
            lon: r.lon,
            ios: r.ios,
            android: r.android,
            status: r.status.map(|s| s.as_str()),
            listened: r.listened,
            channels: r.channels,
            connections: r.connections.iter().map(ConnView::from).collect(),
        }
    }
}

/// State of one connection to a relay. `reason` is relay or network text:
/// the page shows it with `textContent` only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnView {
    Connecting,
    Connected,
    Waiting { retry_in_secs: u64 },
    Abandoned { auth: bool, reason: String },
}

impl From<&ConnState> for ConnView {
    fn from(s: &ConnState) -> Self {
        match s {
            ConnState::Connecting => ConnView::Connecting,
            ConnState::Connected => ConnView::Connected,
            ConnState::Waiting { retry_in } => ConnView::Waiting {
                retry_in_secs: retry_in.as_secs(),
            },
            ConnState::Abandoned { auth, reason } => ConnView::Abandoned {
                auth: *auth,
                reason: reason.clone(),
            },
        }
    }
}

/// What the monitor is doing, in numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StatusView {
    pub watched: u32,
    pub retired: u32,
    /// Relays listened to.
    pub relays: u32,
    /// Relays set aside (dead or AUTH).
    pub dropped_relays: Vec<String>,
    pub connections_up: u32,
    pub connections: u32,
    /// Events not counted, by reason.
    pub rejected: BTreeMap<&'static str, u64>,
}

impl From<&MonitorStatus> for StatusView {
    fn from(s: &MonitorStatus) -> Self {
        StatusView {
            watched: s.watched.len() as u32,
            retired: s.retired.len() as u32,
            relays: s.relays.len() as u32,
            dropped_relays: s.dropped_relays.clone(),
            connections_up: s
                .connections
                .iter()
                .filter(|c| c.state == ConnState::Connected)
                .count() as u32,
            connections: s.connections.len() as u32,
            rejected: s
                .rejected
                .iter()
                .map(|(r, n)| (rejection_name(*r), *n))
                .collect(),
        }
    }
}

fn rejection_name(r: Rejection) -> &'static str {
    match r {
        Rejection::Duplicate => "duplicate",
        Rejection::BadSignature => "bad_signature",
        Rejection::WrongKind => "wrong_kind",
        Rejection::NoChannel => "no_channel",
        Rejection::NotWatched => "not_watched",
        Rejection::OutOfWindow => "out_of_window",
    }
}

/// Settings the page needs, sent once in `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Params {
    pub start: String,
    pub depth: usize,
    /// Channel levels watched (geohash lengths).
    pub levels: Vec<usize>,
    pub max_channels: usize,
    pub window_secs: i64,
    pub inactivity_secs: i64,
    /// Participants below which a level 6 cell is "low"; 0: none.
    pub k: u32,
    /// Tile URL template and its attribution (HTML from the server
    /// configuration, not from relays).
    pub tiles: String,
    pub attribution: String,
}

impl Params {
    pub fn new(config: &MonitorConfig, tiles: &str, attribution: &str) -> Self {
        Params {
            start: config.start.clone(),
            depth: config.depth,
            levels: config.levels.iter().map(|l| l.precision()).collect(),
            max_channels: config.max_channels,
            window_secs: config.aggregator.window_secs,
            inactivity_secs: config.aggregator.inactivity_secs,
            k: config.aggregator.k_threshold,
            tiles: tiles.to_owned(),
            attribution: attribution.to_owned(),
        }
    }
}

/// One SSE message. `seq` grows by one per message: a browser that sees a
/// gap has missed one and reconnects for a new `hello`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Push {
    /// Full state, first message of each stream. Its `seq` is the last one
    /// sent: the next message has `seq + 1`.
    Hello {
        seq: u64,
        params: Params,
        cells: Vec<CellView>,
        relays: Vec<RelayView>,
        status: StatusView,
    },
    /// Cells new or changed since the previous message.
    Cells {
        seq: u64,
        cells: Vec<CellView>,
    },
    /// Cells no longer in the plan.
    Removed {
        seq: u64,
        cells: Vec<String>,
    },
    /// The whole relay list, when any relay changed.
    Relays {
        seq: u64,
        relays: Vec<RelayView>,
    },
    Status {
        seq: u64,
        status: StatusView,
    },
}

impl Push {
    /// SSE event name, same as the `type` field.
    pub fn event(&self) -> &'static str {
        match self {
            Push::Hello { .. } => "hello",
            Push::Cells { .. } => "cells",
            Push::Removed { .. } => "removed",
            Push::Relays { .. } => "relays",
            Push::Status { .. } => "status",
        }
    }

    pub fn seq(&self) -> u64 {
        match self {
            Push::Hello { seq, .. }
            | Push::Cells { seq, .. }
            | Push::Removed { seq, .. }
            | Push::Relays { seq, .. }
            | Push::Status { seq, .. } => *seq,
        }
    }

    pub fn to_json(&self) -> String {
        // Plain data with string keys: cannot fail.
        serde_json::to_string(self).expect("serialisable")
    }
}

/// The last state sent to the browsers, to compute the deltas once for all
/// of them (D11).
#[derive(Debug, Clone)]
pub struct Published {
    params: Params,
    seq: u64,
    cells: BTreeMap<String, CellView>,
    relays: Vec<RelayView>,
    status: Option<StatusView>,
}

impl Published {
    pub fn new(params: Params) -> Self {
        Published {
            params,
            seq: 0,
            cells: BTreeMap::new(),
            relays: Vec::new(),
            status: None,
        }
    }

    /// Sequence number of the last message.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The full state, for a new stream.
    pub fn hello(&self) -> Push {
        Push::Hello {
            seq: self.seq,
            params: self.params.clone(),
            cells: self.cells.values().cloned().collect(),
            relays: self.relays.clone(),
            status: self.status.clone().unwrap_or_default(),
        }
    }

    /// Records a new state and returns the messages that bring a browser
    /// from the previous one to it, in order: removed, cells, relays,
    /// status. Nothing when nothing changed.
    pub fn update(
        &mut self,
        cells: Vec<CellView>,
        relays: Vec<RelayView>,
        status: StatusView,
    ) -> Vec<Push> {
        let mut out = Vec::new();
        let next: BTreeMap<String, CellView> =
            cells.into_iter().map(|c| (c.geohash.clone(), c)).collect();
        let removed: Vec<String> = self
            .cells
            .keys()
            .filter(|g| !next.contains_key(*g))
            .cloned()
            .collect();
        if !removed.is_empty() {
            self.seq += 1;
            out.push(Push::Removed {
                seq: self.seq,
                cells: removed,
            });
        }
        let changed: Vec<CellView> = next
            .values()
            .filter(|c| self.cells.get(&c.geohash) != Some(c))
            .cloned()
            .collect();
        if !changed.is_empty() {
            self.seq += 1;
            out.push(Push::Cells {
                seq: self.seq,
                cells: changed,
            });
        }
        self.cells = next;
        if relays != self.relays {
            self.seq += 1;
            out.push(Push::Relays {
                seq: self.seq,
                relays: relays.clone(),
            });
            self.relays = relays;
        }
        if self.status.as_ref() != Some(&status) {
            self.seq += 1;
            out.push(Push::Status {
                seq: self.seq,
                status: status.clone(),
            });
            self.status = Some(status);
        }
        out
    }
}
