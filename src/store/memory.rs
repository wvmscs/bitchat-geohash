// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Store in memory, saved whole as a JSON snapshot: the browser build's
//! [`Store`](super::Store), kept in the browser's database by the page.
//! Built natively too, so that the same tests check both stores.
//!
//! Same records and rules as the SQLite schema (allowed statuses, channel
//! origin kept across state changes), but it keeps only what is read back:
//! the latest list of each platform with its content, the latest probe of
//! each relay, every channel, and the buckets of the last
//! [`BUCKET_RETENTION_SECS`]. Like the schema, no pubkey, nickname or
//! message content.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{CellBucket, ChannelState, RelayListLoad, Write};
use crate::directory::Platform;
use crate::error::{Error, Result};
use crate::probe::Status;
use crate::rt::Instant;

/// Snapshot format this code writes; a newer snapshot is refused.
pub const SNAPSHOT_VERSION: u32 = 1;

/// Buckets older than this before the newest one are dropped (7 days).
pub const BUCKET_RETENTION_SECS: i64 = 7 * 24 * 3600;

/// Least time between two saves while writes keep coming.
pub const SAVE_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Tables {
    version: u32,
    /// Platform (`ios`, `android`) -> its latest load.
    relay_list_load: BTreeMap<String, ListRow>,
    /// SHA-256 -> content, for the loads kept.
    relay_list_csv: BTreeMap<String, String>,
    /// Relay -> its latest probe.
    probe_result: BTreeMap<String, ProbeRow>,
    /// Geohash -> state.
    channel: BTreeMap<String, ChannelRow>,
    /// Geohash -> minute -> counters.
    cell_bucket: BTreeMap<String, BTreeMap<i64, BucketRow>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ListRow {
    source: String,
    loaded_at: i64,
    sha256: String,
    relays: u32,
    rejected: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProbeRow {
    probed_at: i64,
    status: String,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ChannelRow {
    state: String,
    changed_at: i64,
    reason: Option<String>,
    /// `start` or `descent`.
    origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct BucketRow {
    messages: u32,
    presences: u32,
    participants: u32,
}

/// Called with the snapshot to keep, at most every [`SAVE_EVERY`] while
/// writes come, and when the writer is flushed or closed.
pub type Saver = Rc<dyn Fn(String)>;

pub struct MemoryStore {
    tables: Tables,
    saver: Option<Saver>,
    dirty: bool,
    saved_at: Option<Instant>,
}

impl MemoryStore {
    pub fn open_in_memory() -> Result<Self> {
        Ok(MemoryStore {
            tables: Tables {
                version: SNAPSHOT_VERSION,
                ..Tables::default()
            },
            saver: None,
            dirty: false,
            saved_at: None,
        })
    }

    /// From a snapshot ([`MemoryStore::snapshot`]). Refuses a newer one.
    pub fn restore(snapshot: &str) -> Result<Self> {
        let tables: Tables =
            serde_json::from_str(snapshot).map_err(|e| Error::Snapshot(e.to_string()))?;
        if tables.version > SNAPSHOT_VERSION {
            return Err(Error::SchemaTooNew {
                found: tables.version,
                supported: SNAPSHOT_VERSION,
            });
        }
        let mut store = Self::open_in_memory()?;
        store.tables = Tables {
            version: SNAPSHOT_VERSION,
            ..tables
        };
        Ok(store)
    }

    /// Everything, as JSON.
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&self.tables).expect("plain data serialises")
    }

    /// Where snapshots go once a writer holds the store.
    pub fn set_saver(&mut self, saver: Saver) {
        self.saver = Some(saver);
    }

    pub fn saver(&self) -> Option<Saver> {
        self.saver.clone()
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.tables.version)
    }

    /// Applies writes, all or none.
    pub fn apply(&mut self, writes: &[Write]) -> Result<()> {
        for w in writes {
            if let Write::Probe { status, .. } = w
                && Status::parse(status).is_none()
            {
                return Err(Error::InvalidRecord(format!("probe status {status:?}")));
            }
        }
        for w in writes {
            self.apply_one(w);
        }
        self.dirty |= !writes.is_empty();
        Ok(())
    }

    fn apply_one(&mut self, w: &Write) {
        let t = &mut self.tables;
        match w {
            Write::RelayList(l) => {
                let row = ListRow {
                    source: l.source.clone(),
                    loaded_at: l.loaded_at,
                    sha256: l.sha256.clone(),
                    relays: l.relays,
                    rejected: l.rejected,
                };
                let key = l.platform.as_str().to_owned();
                // The latest load wins; at the same time, the last written.
                if t.relay_list_load
                    .get(&key)
                    .is_none_or(|old| old.loaded_at <= l.loaded_at)
                {
                    t.relay_list_load.insert(key, row);
                    t.relay_list_csv
                        .entry(l.sha256.clone())
                        .or_insert_with(|| l.csv.clone());
                    let kept: Vec<&String> =
                        t.relay_list_load.values().map(|r| &r.sha256).collect();
                    t.relay_list_csv.retain(|sha, _| kept.contains(&sha));
                }
            }
            Write::Probe {
                relay,
                probed_at,
                status,
                detail,
            } => {
                if t.probe_result
                    .get(relay)
                    .is_none_or(|old| old.probed_at <= *probed_at)
                {
                    t.probe_result.insert(
                        relay.clone(),
                        ProbeRow {
                            probed_at: *probed_at,
                            status: status.clone(),
                            detail: detail.clone(),
                        },
                    );
                }
            }
            Write::ChannelState {
                geohash,
                state,
                changed_at,
                reason,
            } => {
                let origin = t
                    .channel
                    .get(geohash)
                    .map_or_else(|| "start".to_owned(), |old| old.origin.clone());
                t.channel.insert(
                    geohash.clone(),
                    ChannelRow {
                        state: state.as_str().to_owned(),
                        changed_at: *changed_at,
                        reason: reason.clone(),
                        origin,
                    },
                );
            }
            Write::Descent { geohash, added_at } => {
                t.channel
                    .entry(geohash.clone())
                    .and_modify(|row| row.origin = "descent".to_owned())
                    .or_insert_with(|| ChannelRow {
                        state: ChannelState::Watched.as_str().to_owned(),
                        changed_at: *added_at,
                        reason: Some("descent".to_owned()),
                        origin: "descent".to_owned(),
                    });
            }
            Write::ForgetDescents => t.channel.retain(|_, row| row.origin != "descent"),
            Write::Buckets(buckets) => {
                for b in buckets {
                    t.cell_bucket.entry(b.geohash.clone()).or_default().insert(
                        b.bucket_start,
                        BucketRow {
                            messages: b.messages,
                            presences: b.presences,
                            participants: b.participants,
                        },
                    );
                }
                let newest = t
                    .cell_bucket
                    .values()
                    .filter_map(|m| m.last_key_value().map(|(k, _)| *k))
                    .max();
                if let Some(newest) = newest {
                    let from = newest - BUCKET_RETENTION_SECS;
                    for minutes in t.cell_bucket.values_mut() {
                        minutes.retain(|start, _| *start >= from);
                    }
                    t.cell_bucket.retain(|_, minutes| !minutes.is_empty());
                }
            }
        }
    }

    /// Latest load of a platform's list: the fallback when a download fails.
    pub fn last_relay_list(&self, platform: Platform) -> Result<Option<RelayListLoad>> {
        let t = &self.tables;
        Ok(t.relay_list_load.get(platform.as_str()).and_then(|row| {
            Some(RelayListLoad {
                platform,
                source: row.source.clone(),
                loaded_at: row.loaded_at,
                sha256: row.sha256.clone(),
                relays: row.relays,
                rejected: row.rejected,
                csv: t.relay_list_csv.get(&row.sha256)?.clone(),
            })
        }))
    }

    /// Latest probe of a relay: `(probed_at, status, detail)`.
    pub fn last_probe(&self, relay: &str) -> Result<Option<(i64, String, String)>> {
        Ok(self
            .tables
            .probe_result
            .get(relay)
            .map(|p| (p.probed_at, p.status.clone(), p.detail.clone())))
    }

    /// Every known channel and its state, in geohash order.
    pub fn channel_states(&self) -> Result<Vec<(String, ChannelState)>> {
        Ok(self
            .tables
            .channel
            .iter()
            .filter_map(|(g, row)| Some((g.clone(), ChannelState::parse(&row.state)?)))
            .collect())
    }

    /// Retired channels, in geohash order.
    pub fn retired_channels(&self) -> Result<Vec<String>> {
        Ok(self
            .channel_states()?
            .into_iter()
            .filter(|(_, s)| *s == ChannelState::Retired)
            .map(|(g, _)| g)
            .collect())
    }

    /// Channels added by descents, in geohash order.
    pub fn descents(&self) -> Result<Vec<String>> {
        Ok(self
            .tables
            .channel
            .iter()
            .filter(|(_, row)| row.origin == "descent")
            .map(|(g, _)| g.clone())
            .collect())
    }

    /// Buckets of a channel with `from <= bucket_start < to`, oldest first.
    pub fn buckets(&self, geohash: &str, from: i64, to: i64) -> Result<Vec<CellBucket>> {
        let Some(minutes) = self.tables.cell_bucket.get(geohash) else {
            return Ok(Vec::new());
        };
        if from >= to {
            return Ok(Vec::new());
        }
        Ok(minutes
            .range(from..to)
            .map(|(start, b)| CellBucket {
                geohash: geohash.to_owned(),
                bucket_start: *start,
                messages: b.messages,
                presences: b.presences,
                participants: b.participants,
            })
            .collect())
    }

    /// Field names of every table, `table.field`, read from a snapshot with
    /// one row in each: lets tests check that no personal data has a place.
    pub fn columns(&self) -> Result<Vec<String>> {
        let row = |s: &str| s.to_owned();
        let sample = Tables {
            version: SNAPSHOT_VERSION,
            relay_list_load: [(
                row("ios"),
                ListRow {
                    source: row(""),
                    loaded_at: 0,
                    sha256: row(""),
                    relays: 0,
                    rejected: 0,
                },
            )]
            .into(),
            relay_list_csv: BTreeMap::new(),
            probe_result: [(
                row(""),
                ProbeRow {
                    probed_at: 0,
                    status: row(""),
                    detail: row(""),
                },
            )]
            .into(),
            channel: [(
                row(""),
                ChannelRow {
                    state: row(""),
                    changed_at: 0,
                    reason: None,
                    origin: row(""),
                },
            )]
            .into(),
            cell_bucket: [(
                row(""),
                [(
                    0,
                    BucketRow {
                        messages: 0,
                        presences: 0,
                        participants: 0,
                    },
                )]
                .into(),
            )]
            .into(),
        };
        let value = serde_json::to_value(&sample).expect("plain data serialises");
        let mut out = vec![
            // Keyed maps: their key is a field too.
            "relay_list_load.platform".to_owned(),
            "relay_list_csv.sha256".to_owned(),
            "relay_list_csv.csv".to_owned(),
            "probe_result.relay".to_owned(),
            "channel.geohash".to_owned(),
            "cell_bucket.geohash".to_owned(),
            "cell_bucket.bucket_start".to_owned(),
        ];
        // The rows sit one level down (two for the buckets).
        for (table, rows) in value.as_object().into_iter().flatten() {
            let mut level = rows;
            while let Some(first) = level.as_object().and_then(|m| m.values().next()) {
                if let Some(fields) = first.as_object()
                    && fields.values().all(|v| !v.is_object())
                {
                    out.extend(fields.keys().map(|f| format!("{table}.{f}")));
                    break;
                }
                level = first;
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Hands the snapshot to the saver if writes are pending and `force` or
    /// [`SAVE_EVERY`] passed since the last save.
    fn save(&mut self, force: bool) {
        let due = force || self.saved_at.is_none_or(|at| at.elapsed() >= SAVE_EVERY);
        if !self.dirty || !due {
            return;
        }
        let snapshot = self.snapshot();
        if let Some(saver) = &self.saver {
            saver(snapshot);
            self.dirty = false;
            self.saved_at = Some(Instant::now());
        }
    }
}

/// The writer of the browser build: one thread there, so writes are applied
/// at once, and the snapshot saved now and then ([`SAVE_EVERY`]); in the
/// browser, writes left unsaved by that limit are saved [`SAVE_EVERY`]
/// later. Same interface as the SQLite writer thread.
pub struct MemoryWriter {
    store: Rc<RefCell<MemoryStore>>,
    pending_error: RefCell<Option<Error>>,
    /// A deferred save is on its way.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    deferred: Rc<Cell<bool>>,
}

impl MemoryWriter {
    pub fn spawn(store: MemoryStore) -> Self {
        MemoryWriter {
            store: Rc::new(RefCell::new(store)),
            pending_error: RefCell::new(None),
            deferred: Rc::new(Cell::new(false)),
        }
    }

    /// Applies a write; never blocks. A failure is reported by the next
    /// [`MemoryWriter::flush`].
    pub fn send(&self, w: Write) -> Result<()> {
        let mut store = self.store.borrow_mut();
        if let Err(e) = store.apply(std::slice::from_ref(&w)) {
            tracing::error!(error = %e, "store write failed");
            self.pending_error.borrow_mut().get_or_insert(e);
        }
        store.save(false);
        #[cfg(target_arch = "wasm32")]
        if store.dirty && !self.deferred.replace(true) {
            // The task holds no strong reference: `close` takes the store.
            let (store, deferred) = (Rc::downgrade(&self.store), self.deferred.clone());
            crate::rt::spawn(async move {
                crate::rt::sleep(SAVE_EVERY).await;
                deferred.set(false);
                if let Some(store) = store.upgrade() {
                    store.borrow_mut().save(true);
                }
            });
        }
        Ok(())
    }

    /// Saves, and returns the first error met since the previous flush.
    pub fn flush(&self) -> Result<()> {
        self.store.borrow_mut().save(true);
        self.pending_error.borrow_mut().take().map_or(Ok(()), Err)
    }

    /// Saves and hands the store back.
    pub fn close(self) -> Result<MemoryStore> {
        let mut store = Rc::try_unwrap(self.store)
            .map_err(|_| Error::StoreClosed)?
            .into_inner();
        store.save(true);
        Ok(store)
    }
}
