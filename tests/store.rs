// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQLite store: schema, persistence, writer thread, privacy.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bitchat_geohash::Error;
use bitchat_geohash::directory::Platform;
use bitchat_geohash::store::{
    CellBucket, ChannelState, RelayListLoad, SCHEMA_VERSION, Store, StoreWriter, Write, sha256_hex,
};

/// A database file in the temp dir, removed on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        TempDb(std::env::temp_dir().join(format!(
            "bitchat-geohash-{name}-{}-{nanos}.sqlite",
            std::process::id()
        )))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

fn list(platform: Platform, csv: &str, at: i64) -> Write {
    Write::RelayList(RelayListLoad {
        platform,
        source: "tests/data".into(),
        loaded_at: at,
        sha256: sha256_hex(csv),
        relays: 1,
        rejected: 0,
        csv: csv.into(),
    })
}

fn retired(geohash: &str, at: i64) -> Write {
    Write::ChannelState {
        geohash: geohash.into(),
        state: ChannelState::Retired,
        changed_at: at,
        reason: Some("inactive for 5 min".into()),
    }
}

fn bucket(geohash: &str, start: i64, messages: u32) -> CellBucket {
    CellBucket {
        geohash: geohash.into(),
        bucket_start: start,
        messages,
        presences: 0,
        participants: 1,
    }
}

#[test]
fn new_database_is_at_the_current_version() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn channel_state_survives_a_restart() {
    let db = TempDb::new("restart");
    let mut store = Store::open(&db.0).unwrap();
    store
        .apply(&[retired("u09tv", 100), retired("u09tu", 100)])
        .unwrap();
    store
        .apply(&[Write::ChannelState {
            geohash: "u09tu".into(),
            state: ChannelState::Watched,
            changed_at: 200,
            reason: None,
        }])
        .unwrap();
    drop(store);
    let store = Store::open(&db.0).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        store.channel_states().unwrap(),
        [
            ("u09tu".to_owned(), ChannelState::Watched),
            ("u09tv".to_owned(), ChannelState::Retired)
        ]
    );
    assert_eq!(store.retired_channels().unwrap(), ["u09tv"]);
}

#[test]
fn newer_database_is_refused() {
    let db = TempDb::new("newer");
    drop(Store::open(&db.0).unwrap());
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    drop(conn);
    match Store::open(&db.0) {
        Err(Error::SchemaTooNew { found, supported }) => {
            assert_eq!((found, supported), (SCHEMA_VERSION + 1, SCHEMA_VERSION));
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("opened a newer database"),
    }
}

#[test]
fn relay_list_fallback_is_the_latest_load() {
    let db = TempDb::new("lists");
    let mut store = Store::open(&db.0).unwrap();
    assert!(store.last_relay_list(Platform::Ios).unwrap().is_none());
    let (a, b) = (
        "Relay URL,Latitude,Longitude\na.example,1,1\n",
        "Relay URL,Latitude,Longitude\nb.example,2,2\n",
    );
    store
        .apply(&[
            list(Platform::Ios, a, 100),
            list(Platform::Ios, b, 200),
            list(Platform::Ios, a, 300),
        ])
        .unwrap();
    store.apply(&[list(Platform::Android, b, 400)]).unwrap();
    let ios = store.last_relay_list(Platform::Ios).unwrap().unwrap();
    assert_eq!(
        (ios.loaded_at, ios.csv.as_str(), ios.sha256.clone()),
        (300, a, sha256_hex(a))
    );
    let android = store.last_relay_list(Platform::Android).unwrap().unwrap();
    assert_eq!((android.loaded_at, android.csv.as_str()), (400, b));
    // Each distinct content is stored once.
    drop(store);
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM relay_list_csv", [], |r| r.get(0))
        .unwrap();
    let loads: i64 = conn
        .query_row("SELECT count(*) FROM relay_list_load", [], |r| r.get(0))
        .unwrap();
    assert_eq!((n, loads), (2, 4));
}

#[test]
fn sha256_is_hex_of_the_content() {
    assert_eq!(
        sha256_hex(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn latest_probe() {
    let mut store = Store::open_in_memory().unwrap();
    let probe = |at, status: &str| Write::Probe {
        relay: "wss://relay.example".into(),
        probed_at: at,
        status: status.into(),
        detail: format!("{status} at {at}"),
    };
    store
        .apply(&[probe(100, "down"), probe(200, "ok")])
        .unwrap();
    assert_eq!(
        store.last_probe("wss://relay.example").unwrap(),
        Some((200, "ok".to_owned(), "ok at 200".to_owned()))
    );
    assert_eq!(store.last_probe("wss://other.example").unwrap(), None);
    // Unknown status: refused by the schema.
    assert!(store.apply(&[probe(300, "bogus")]).is_err());
}

#[test]
fn buckets_are_replaced_and_queried_by_range() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .apply(&[Write::Buckets(vec![
            bucket("u09tv", 60, 1),
            bucket("u09tv", 120, 2),
            bucket("u09tu", 60, 9),
        ])])
        .unwrap();
    store
        .apply(&[Write::Buckets(vec![bucket("u09tv", 120, 5)])])
        .unwrap();
    assert_eq!(
        store.buckets("u09tv", 0, 1_000).unwrap(),
        [bucket("u09tv", 60, 1), bucket("u09tv", 120, 5)]
    );
    assert_eq!(
        store.buckets("u09tv", 61, 1_000).unwrap(),
        [bucket("u09tv", 120, 5)]
    );
    assert!(store.buckets("u09tv", 0, 60).unwrap().is_empty());
}

#[test]
fn writer_thread_commits_in_order() {
    let writer = StoreWriter::spawn(Store::open_in_memory().unwrap());
    for i in 0..2_000 {
        writer
            .send(Write::Buckets(vec![bucket("u09tv", i * 60, i as u32)]))
            .unwrap();
    }
    writer.send(retired("u09tv", 10)).unwrap();
    writer.flush().unwrap();
    let store = writer.close().unwrap();
    assert_eq!(store.buckets("u09tv", 0, i64::MAX).unwrap().len(), 2_000);
    assert_eq!(store.retired_channels().unwrap(), ["u09tv"]);
}

#[test]
fn one_bad_write_does_not_lose_the_others() {
    let writer = StoreWriter::spawn(Store::open_in_memory().unwrap());
    writer.send(retired("u09tv", 10)).unwrap();
    writer
        .send(Write::Probe {
            relay: "wss://relay.example".into(),
            probed_at: 10,
            status: "bogus".into(),
            detail: String::new(),
        })
        .unwrap();
    writer.send(retired("u09tu", 10)).unwrap();
    assert!(writer.flush().is_err());
    // The error is reported once.
    writer.flush().unwrap();
    let store = writer.close().unwrap();
    assert_eq!(store.retired_channels().unwrap(), ["u09tu", "u09tv"]);
}

/// Spec §5: no pubkey, nickname or message content in the database.
/// Extend `FORBIDDEN` if a new kind of personal data appears.
#[test]
fn schema_has_no_place_for_personal_data() {
    const FORBIDDEN: &[&str] = &[
        "pubkey",
        "npub",
        "author",
        "nick",
        "content",
        "message_text",
        "sig",
        "tags",
    ];
    let store = Store::open_in_memory().unwrap();
    let columns = store.columns().unwrap();
    assert!(
        columns.contains(&"cell_bucket.participants".to_owned()),
        "{columns:?}"
    );
    for col in &columns {
        let name = col.split('.').nth(1).unwrap();
        assert!(!FORBIDDEN.iter().any(|f| name.contains(f)), "column {col}");
    }
}

#[test]
fn descents_are_kept_until_forgotten() {
    let db = TempDb::new("descents");
    let mut store = Store::open(&db.0).unwrap();
    store
        .apply(&[
            retired("u09tv", 100),
            Write::Descent {
                geohash: "u09tvq".into(),
                added_at: 200,
            },
            Write::Descent {
                geohash: "u09tvw".into(),
                added_at: 200,
            },
        ])
        .unwrap();
    // A later state change keeps the origin.
    store.apply(&[retired("u09tvq", 300)]).unwrap();
    drop(store);
    let mut store = Store::open(&db.0).unwrap();
    assert_eq!(store.descents().unwrap(), ["u09tvq", "u09tvw"]);
    assert_eq!(store.retired_channels().unwrap(), ["u09tv", "u09tvq"]);
    store.apply(&[Write::ForgetDescents]).unwrap();
    assert!(store.descents().unwrap().is_empty());
    assert_eq!(store.retired_channels().unwrap(), ["u09tv"]);
}

#[test]
fn version_1_database_is_migrated_with_its_channels() {
    let db = TempDb::new("v1");
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    conn.execute_batch(include_str!("../src/store/migrations/0001_init.sql"))
        .unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    conn.execute(
        "INSERT INTO channel (geohash, state, changed_at, reason) VALUES ('u09tv', 'retired', 1, NULL)",
        [],
    )
    .unwrap();
    drop(conn);
    let store = Store::open(&db.0).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.retired_channels().unwrap(), ["u09tv"]);
    assert!(
        store.descents().unwrap().is_empty(),
        "old channels are part of the start area"
    );
}
