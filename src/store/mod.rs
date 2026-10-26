//! SQLite store and versioned schema migrations (lot 4).
//!
//! [`Store`] is synchronous. From async code, writes go through a
//! [`StoreWriter`]: one dedicated thread that batches them into transactions,
//! so no tokio task ever blocks on the database. Reads (at start-up, in
//! tests) use the `Store` directly, before the writer takes it or after
//! [`StoreWriter::close`] hands it back.
//!
//! Times are Unix seconds, always given by the caller (injected clock). The
//! schema holds no pubkey, nickname or message content (spec §5).

mod migrations;

use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::directory::Platform;
use crate::error::{Error, Result};

pub use migrations::SCHEMA_VERSION;

/// Whether a channel is listened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelState {
    Watched,
    Retired,
}

impl ChannelState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelState::Watched => "watched",
            ChannelState::Retired => "retired",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "watched" => Some(ChannelState::Watched),
            "retired" => Some(ChannelState::Retired),
            _ => None,
        }
    }
}

/// Counters of one channel over one minute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellBucket {
    pub geohash: String,
    /// Start of the minute, Unix seconds (a multiple of 60).
    pub bucket_start: i64,
    pub messages: u32,
    pub presences: u32,
    pub participants: u32,
}

/// A relay list load, with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayListLoad {
    pub platform: Platform,
    pub source: String,
    pub loaded_at: i64,
    pub sha256: String,
    pub relays: u32,
    pub rejected: u32,
    pub csv: String,
}

/// One write, for [`StoreWriter::send`].
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    RelayList(RelayListLoad),
    Probe {
        relay: String,
        probed_at: i64,
        status: String,
        detail: String,
    },
    ChannelState {
        geohash: String,
        state: ChannelState,
        changed_at: i64,
        reason: Option<String>,
    },
    Buckets(Vec<CellBucket>),
}

/// SHA-256 of a relay list, hex: tells whether a reload changed anything.
pub fn sha256_hex(csv: &str) -> String {
    hex(&Sha256::digest(csv.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (or creates) the database and migrates it to [`SCHEMA_VERSION`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        // WAL is not available in memory; SQLite then keeps "memory".
        conn.pragma_update(None, "journal_mode", "wal")?;
        migrations::apply(&mut conn, migrations::MIGRATIONS)?;
        Ok(Store { conn })
    }

    pub fn schema_version(&self) -> Result<u32> {
        migrations::user_version(&self.conn)
    }

    /// Applies writes in one transaction.
    pub fn apply(&mut self, writes: &[Write]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for w in writes {
            apply_one(&tx, w)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Latest load of a platform's list: the fallback when a download fails.
    pub fn last_relay_list(&self, platform: Platform) -> Result<Option<RelayListLoad>> {
        Ok(self
            .conn
            .query_row(
                "SELECT l.source, l.loaded_at, l.sha256, l.relays, l.rejected, c.csv
                 FROM relay_list_load l JOIN relay_list_csv c USING (sha256)
                 WHERE l.platform = ?1 ORDER BY l.loaded_at DESC, l.id DESC LIMIT 1",
                [platform.as_str()],
                |r| {
                    Ok(RelayListLoad {
                        platform,
                        source: r.get(0)?,
                        loaded_at: r.get(1)?,
                        sha256: r.get(2)?,
                        relays: r.get(3)?,
                        rejected: r.get(4)?,
                        csv: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// Latest probe of a relay: `(probed_at, status, detail)`.
    pub fn last_probe(&self, relay: &str) -> Result<Option<(i64, String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT probed_at, status, detail FROM probe_result
                 WHERE relay = ?1 ORDER BY probed_at DESC, id DESC LIMIT 1",
                [relay],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    /// Every known channel and its state, in geohash order.
    pub fn channel_states(&self) -> Result<Vec<(String, ChannelState)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT geohash, state FROM channel ORDER BY geohash")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (geohash, state) = row?;
            // The CHECK constraint guarantees a known value.
            let state = ChannelState::parse(&state).expect("CHECK constraint on channel.state");
            out.push((geohash, state));
        }
        Ok(out)
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

    /// Buckets of a channel with `from <= bucket_start < to`, oldest first.
    pub fn buckets(&self, geohash: &str, from: i64, to: i64) -> Result<Vec<CellBucket>> {
        let mut stmt = self.conn.prepare(
            "SELECT bucket_start, messages, presences, participants FROM cell_bucket
             WHERE geohash = ?1 AND bucket_start >= ?2 AND bucket_start < ?3
             ORDER BY bucket_start",
        )?;
        let rows = stmt.query_map(params![geohash, from, to], |r| {
            Ok(CellBucket {
                geohash: geohash.to_owned(),
                bucket_start: r.get(0)?,
                messages: r.get(1)?,
                presences: r.get(2)?,
                participants: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Column names of every table, `table.column`: lets tests check that
    /// no personal data has a place in the schema.
    pub fn columns(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.name || '.' || p.name FROM sqlite_master m, pragma_table_info(m.name) p
             WHERE m.type = 'table' ORDER BY 1",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn apply_one(tx: &rusqlite::Transaction, w: &Write) -> Result<()> {
    match w {
        Write::RelayList(l) => {
            tx.execute(
                "INSERT OR IGNORE INTO relay_list_csv (sha256, csv) VALUES (?1, ?2)",
                params![l.sha256, l.csv],
            )?;
            tx.execute(
                "INSERT INTO relay_list_load (platform, source, loaded_at, sha256, relays, rejected)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![l.platform.as_str(), l.source, l.loaded_at, l.sha256, l.relays, l.rejected],
            )?;
        }
        Write::Probe {
            relay,
            probed_at,
            status,
            detail,
        } => {
            tx.execute(
                "INSERT INTO probe_result (relay, probed_at, status, detail) VALUES (?1, ?2, ?3, ?4)",
                params![relay, probed_at, status, detail],
            )?;
        }
        Write::ChannelState {
            geohash,
            state,
            changed_at,
            reason,
        } => {
            tx.execute(
                "INSERT INTO channel (geohash, state, changed_at, reason) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (geohash) DO UPDATE
                 SET state = excluded.state, changed_at = excluded.changed_at, reason = excluded.reason",
                params![geohash, state.as_str(), changed_at, reason],
            )?;
        }
        Write::Buckets(buckets) => {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO cell_bucket
                 (geohash, bucket_start, messages, presences, participants)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for b in buckets {
                stmt.execute(params![
                    b.geohash,
                    b.bucket_start,
                    b.messages,
                    b.presences,
                    b.participants
                ])?;
            }
        }
    }
    Ok(())
}

enum Msg {
    Write(Write),
    Flush(mpsc::Sender<Result<()>>),
}

/// Most writes per transaction.
const BATCH: usize = 512;

/// The dedicated writer thread. Writes are queued without blocking and
/// applied in batches, one transaction per batch.
pub struct StoreWriter {
    tx: mpsc::Sender<Msg>,
    thread: JoinHandle<Store>,
}

impl StoreWriter {
    pub fn spawn(store: Store) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("bitchat-geohash-store".into())
            .spawn(move || writer_loop(store, rx))
            .expect("cannot spawn the store writer thread");
        StoreWriter { tx, thread }
    }

    /// Queues a write; never blocks.
    pub fn send(&self, w: Write) -> Result<()> {
        self.tx.send(Msg::Write(w)).map_err(|_| Error::StoreClosed)
    }

    /// Waits until every write queued before is committed, and returns the
    /// first error met since the previous flush. Blocking: from async code,
    /// call it in `spawn_blocking`.
    pub fn flush(&self) -> Result<()> {
        let (reply, done) = mpsc::channel();
        self.tx
            .send(Msg::Flush(reply))
            .map_err(|_| Error::StoreClosed)?;
        done.recv().map_err(|_| Error::StoreClosed)?
    }

    /// Commits what is queued, stops the thread and hands the store back.
    pub fn close(self) -> Result<Store> {
        drop(self.tx);
        self.thread.join().map_err(|_| Error::StoreClosed)
    }
}

fn writer_loop(mut store: Store, rx: mpsc::Receiver<Msg>) -> Store {
    let mut pending_error: Option<Error> = None;
    while let Ok(first) = rx.recv() {
        let mut batch = Vec::new();
        let mut flushes = Vec::new();
        let mut next = Some(first);
        while let Some(msg) = next {
            match msg {
                Msg::Write(w) => batch.push(w),
                Msg::Flush(reply) => flushes.push(reply),
            }
            next = if batch.len() < BATCH {
                rx.try_recv().ok()
            } else {
                None
            };
        }
        // A failed batch is retried write by write, so that one bad write
        // does not take the others down with it.
        if !batch.is_empty() && store.apply(&batch).is_err() {
            for w in &batch {
                if let Err(e) = store.apply(std::slice::from_ref(w)) {
                    tracing::error!(error = %e, "store write failed");
                    pending_error.get_or_insert(e);
                }
            }
        }
        for reply in flushes {
            let _ = reply.send(pending_error.take().map_or(Ok(()), Err));
        }
    }
    store
}
