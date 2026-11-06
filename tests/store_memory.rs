// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The in-memory store of the browser build: the same behaviour as the
//! SQLite store (each check runs on both), and its snapshot.

use std::cell::RefCell;
use std::rc::Rc;

use bitchat_geohash::Error;
use bitchat_geohash::directory::Platform;
use bitchat_geohash::store::memory::{
    BUCKET_RETENTION_SECS, MemoryStore, MemoryWriter, SNAPSHOT_VERSION,
};
use bitchat_geohash::store::{
    CellBucket, ChannelState, RelayListLoad, Store, StoreWriter, Write, sha256_hex,
};

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

fn probe(at: i64, status: &str) -> Write {
    Write::Probe {
        relay: "wss://relay.example".into(),
        probed_at: at,
        status: status.into(),
        detail: format!("{status} at {at}"),
    }
}

/// One test per store: `$store` and `$writer` name its types in `$body`.
macro_rules! on_both {
    ($name:ident, |$store:ident, $writer:ident| $body:block) => {
        mod $name {
            use super::*;

            #[test]
            fn sqlite() {
                #[allow(dead_code)]
                type $store = Store;
                #[allow(dead_code)]
                type $writer = StoreWriter;
                $body
            }

            #[test]
            fn memory() {
                #[allow(dead_code)]
                type $store = MemoryStore;
                #[allow(dead_code)]
                type $writer = MemoryWriter;
                $body
            }
        }
    };
}

on_both!(latest_probe, |S, W| {
    let mut store = S::open_in_memory().unwrap();
    store
        .apply(&[probe(100, "down"), probe(200, "ok")])
        .unwrap();
    // An older one, written later, does not hide the latest.
    store.apply(&[probe(150, "dead")]).unwrap();
    assert_eq!(
        store.last_probe("wss://relay.example").unwrap(),
        Some((200, "ok".to_owned(), "ok at 200".to_owned()))
    );
    assert_eq!(store.last_probe("wss://other.example").unwrap(), None);
    // Unknown status: refused, and nothing of the batch applied.
    assert!(
        store
            .apply(&[probe(300, "ok"), probe(300, "bogus")])
            .is_err()
    );
    assert_eq!(
        store.last_probe("wss://relay.example").unwrap().unwrap().0,
        200
    );
});

on_both!(latest_relay_list, |S, W| {
    let mut store = S::open_in_memory().unwrap();
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
    assert_eq!((ios.loaded_at, ios.csv.as_str()), (300, a));
    assert_eq!(ios.sha256, sha256_hex(a));
    let android = store.last_relay_list(Platform::Android).unwrap().unwrap();
    assert_eq!((android.loaded_at, android.csv.as_str()), (400, b));
});

on_both!(buckets_are_replaced_and_queried_by_range, |S, W| {
    let mut store = S::open_in_memory().unwrap();
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
    assert!(store.buckets("u09tv", 500, 100).unwrap().is_empty());
});

on_both!(channel_states_and_descents, |S, W| {
    let mut store = S::open_in_memory().unwrap();
    store
        .apply(&[
            retired("u09tv", 100),
            retired("u09tu", 100),
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
    store
        .apply(&[Write::ChannelState {
            geohash: "u09tu".into(),
            state: ChannelState::Watched,
            changed_at: 200,
            reason: None,
        }])
        .unwrap();
    // A later state change keeps the origin.
    store.apply(&[retired("u09tvq", 300)]).unwrap();
    assert_eq!(
        store.channel_states().unwrap(),
        [
            ("u09tu".to_owned(), ChannelState::Watched),
            ("u09tv".to_owned(), ChannelState::Retired),
            ("u09tvq".to_owned(), ChannelState::Retired),
            ("u09tvw".to_owned(), ChannelState::Watched),
        ]
    );
    assert_eq!(store.descents().unwrap(), ["u09tvq", "u09tvw"]);
    // A descent into a known channel marks it, without changing its state.
    store
        .apply(&[Write::Descent {
            geohash: "u09tv".into(),
            added_at: 400,
        }])
        .unwrap();
    assert_eq!(store.retired_channels().unwrap(), ["u09tv", "u09tvq"]);
    store.apply(&[Write::ForgetDescents]).unwrap();
    assert!(store.descents().unwrap().is_empty());
    assert!(store.retired_channels().unwrap().is_empty());
    assert_eq!(store.channel_states().unwrap().len(), 1);
});

on_both!(writer_applies_in_order, |S, W| {
    let writer = W::spawn(S::open_in_memory().unwrap());
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
});

on_both!(one_bad_write_does_not_lose_the_others, |S, W| {
    let writer = W::spawn(S::open_in_memory().unwrap());
    writer.send(retired("u09tv", 10)).unwrap();
    writer.send(probe(10, "bogus")).unwrap();
    writer.send(retired("u09tu", 10)).unwrap();
    assert!(writer.flush().is_err());
    // The error is reported once.
    writer.flush().unwrap();
    let store = writer.close().unwrap();
    assert_eq!(store.retired_channels().unwrap(), ["u09tu", "u09tv"]);
});

// Spec §5: no pubkey, nickname or message content in either store.
on_both!(no_place_for_personal_data, |S, W| {
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
    let columns = S::open_in_memory().unwrap().columns().unwrap();
    for expected in [
        "cell_bucket.participants",
        "channel.origin",
        "probe_result.status",
    ] {
        assert!(columns.contains(&expected.to_owned()), "{columns:?}");
    }
    for col in &columns {
        let name = col.split('.').nth(1).unwrap();
        assert!(!FORBIDDEN.iter().any(|f| name.contains(f)), "column {col}");
    }
});

#[test]
fn both_stores_have_the_same_fields() {
    let sqlite = Store::open_in_memory().unwrap().columns().unwrap();
    let memory = MemoryStore::open_in_memory().unwrap().columns().unwrap();
    // SQLite's row ids are the only difference.
    let sqlite: Vec<String> = sqlite.into_iter().filter(|c| !c.ends_with(".id")).collect();
    assert_eq!(memory, sqlite);
}

#[test]
fn snapshot_restores_everything() {
    let mut store = MemoryStore::open_in_memory().unwrap();
    store
        .apply(&[
            list(
                Platform::Ios,
                "Relay URL,Latitude,Longitude\na.example,1,1\n",
                100,
            ),
            probe(100, "ok"),
            retired("u09tv", 100),
            Write::Descent {
                geohash: "u09tvq".into(),
                added_at: 200,
            },
            Write::Buckets(vec![bucket("u09tv", 60, 3)]),
        ])
        .unwrap();
    let again = MemoryStore::restore(&store.snapshot()).unwrap();
    assert_eq!(again.snapshot(), store.snapshot());
    assert_eq!(again.schema_version().unwrap(), SNAPSHOT_VERSION);
    assert_eq!(again.descents().unwrap(), ["u09tvq"]);
    assert_eq!(
        again.buckets("u09tv", 0, 120).unwrap(),
        [bucket("u09tv", 60, 3)]
    );
}

#[test]
fn newer_or_unreadable_snapshots_are_refused() {
    let newer = MemoryStore::open_in_memory().unwrap().snapshot().replace(
        &format!("\"version\":{SNAPSHOT_VERSION}"),
        &format!("\"version\":{}", SNAPSHOT_VERSION + 1),
    );
    assert!(matches!(
        MemoryStore::restore(&newer),
        Err(Error::SchemaTooNew { found, supported })
            if (found, supported) == (SNAPSHOT_VERSION + 1, SNAPSHOT_VERSION)
    ));
    assert!(matches!(
        MemoryStore::restore("{not json"),
        Err(Error::Snapshot(_))
    ));
}

#[test]
fn only_the_contents_of_kept_lists_stay() {
    let (a, b) = (
        "Relay URL,Latitude,Longitude\na.example,1,1\n",
        "Relay URL,Latitude,Longitude\nb.example,2,2\n",
    );
    let mut store = MemoryStore::open_in_memory().unwrap();
    store
        .apply(&[list(Platform::Ios, a, 100), list(Platform::Ios, b, 200)])
        .unwrap();
    let snapshot = store.snapshot();
    assert!(snapshot.contains("b.example") && !snapshot.contains("a.example"));
}

#[test]
fn old_buckets_are_dropped() {
    let mut store = MemoryStore::open_in_memory().unwrap();
    let late = BUCKET_RETENTION_SECS + 120;
    store
        .apply(&[Write::Buckets(vec![
            bucket("u09tu", 0, 1),
            bucket("u09tv", 60, 1),
        ])])
        .unwrap();
    store
        .apply(&[Write::Buckets(vec![bucket("u09tv", late, 2)])])
        .unwrap();
    assert!(store.buckets("u09tu", 0, i64::MAX).unwrap().is_empty());
    assert_eq!(
        store.buckets("u09tv", 0, i64::MAX).unwrap(),
        [bucket("u09tv", late, 2)]
    );
}

#[test]
fn snapshots_are_saved_now_and_then_and_at_flush() {
    let saved = Rc::new(RefCell::new(Vec::new()));
    let mut store = MemoryStore::open_in_memory().unwrap();
    let sink = saved.clone();
    store.set_saver(Rc::new(move |s| sink.borrow_mut().push(s)));
    let writer = MemoryWriter::spawn(store);
    for i in 0..100 {
        writer
            .send(Write::Buckets(vec![bucket("u09tv", i * 60, 1)]))
            .unwrap();
    }
    // The first write is saved at once, the next ones wait (5 s).
    assert_eq!(saved.borrow().len(), 1);
    writer.flush().unwrap();
    assert_eq!(saved.borrow().len(), 2);
    // Nothing new: nothing saved.
    writer.flush().unwrap();
    assert_eq!(saved.borrow().len(), 2);
    writer.send(retired("u09tv", 10)).unwrap();
    let store = writer.close().unwrap();
    assert_eq!(saved.borrow().len(), 3);
    let last = MemoryStore::restore(saved.borrow().last().unwrap()).unwrap();
    assert_eq!(last.snapshot(), store.snapshot());
}
