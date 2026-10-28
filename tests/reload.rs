// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Relay list sources, fallback and 24 h reload, against a local HTTP server
//! (never the real lists: invariant 7).

mod common;

use std::time::Duration;

use bitchat_geohash::Error;
use bitchat_geohash::directory::{ListSource, Platform, fetch};
use bitchat_geohash::monitor::{ListSources, Monitor, MonitorConfig};
use bitchat_geohash::store::Store;
use common::*;

fn reloading(clock: &TestClock) -> MonitorConfig {
    MonitorConfig {
        reload_every: Duration::from_millis(150),
        ..config("u09tv", clock)
    }
}

fn ios_from(url: &str) -> ListSources {
    ListSources {
        ios: Some(ListSource::Url(url.to_owned())),
        android: None,
    }
}

#[tokio::test]
async fn fetch_reads_paths_and_urls() {
    let path = ListSource::parse("tests/data/online_relays_gps.csv");
    assert!(matches!(path, ListSource::Path(_)));
    let (_, dir) = fetch(&path, Platform::Ios).await.unwrap();
    assert_eq!(dir.len(), 326);

    let (url, body) = list_server(Some(list_csv(&["ws://127.0.0.1:1"]))).await;
    assert!(matches!(ListSource::parse(&url), ListSource::Url(_)));
    let (csv, dir) = fetch(&ListSource::parse(&url), Platform::Ios)
        .await
        .unwrap();
    assert_eq!((dir.len(), csv), (1, list_csv(&["ws://127.0.0.1:1"])));

    *body.lock().unwrap() = None;
    assert!(matches!(
        fetch(&ListSource::parse(&url), Platform::Ios).await,
        Err(Error::Http(_))
    ));
    *body.lock().unwrap() = Some("Relay URL,Latitude,Longitude\n".into());
    assert!(matches!(
        fetch(&ListSource::parse(&url), Platform::Ios).await,
        Err(Error::EmptyRelayList(_))
    ));
    assert!(matches!(
        fetch(&ListSource::parse("tests/data/missing.csv"), Platform::Ios).await,
        Err(Error::Io(_))
    ));
}

#[test]
fn official_sources_are_the_apps_urls() {
    let s = ListSources::official();
    assert_eq!(
        s.ios,
        Some(ListSource::Url(
            "https://raw.githubusercontent.com/permissionlesstech/bitchat/refs/heads/main/relays/online_relays_gps.csv".into()
        ))
    );
    assert_eq!(
        s.android,
        Some(ListSource::Url(
            "https://raw.githubusercontent.com/permissionlesstech/georelays/main/nostr_relays.csv"
                .into()
        ))
    );
}

#[tokio::test]
async fn changed_list_moves_the_connections() {
    let (logs, _guard) = Logs::capture();
    let (a, rec_a) = recording_relay(false).await;
    let (b, rec_b) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        reloading(&clock),
        ios_from(&url),
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    eventually("REQ on A", || !rec_a.frames_of("REQ").is_empty()).await;

    *body.lock().unwrap() = Some(list_csv(&[&b]));
    eventually("REQ on B", || !rec_b.frames_of("REQ").is_empty()).await;
    eventually("A disconnected", || rec_a.open() == 0).await;
    assert_eq!(
        monitor.status().await.unwrap().relays,
        std::slice::from_ref(&b)
    );
    assert_eq!(logs.info_lines("relay lists reloaded: new plan"), 1);

    let store = monitor.shutdown().await.unwrap();
    let last = store.last_relay_list(Platform::Ios).unwrap().unwrap();
    assert_eq!((last.csv, last.source), (list_csv(&[&b]), url));
}

#[tokio::test]
async fn unchanged_list_changes_nothing() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("reload-unchanged");
    let (a, rec_a) = recording_relay(false).await;
    let (url, _body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        reloading(&clock),
        ios_from(&url),
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    eventually("REQ on A", || !rec_a.frames_of("REQ").is_empty()).await;
    let reqs = rec_a.frames_of("REQ").len();
    eventually("3 reloads", || {
        logs.info_lines("relay lists reloaded: unchanged") >= 3
    })
    .await;
    assert_eq!(rec_a.frames_of("REQ").len(), reqs, "no new subscription");
    assert_eq!(rec_a.accepted(), 1, "no reconnection");
    drop(monitor.shutdown().await.unwrap());

    // Every load is recorded, the content once.
    let conn = rusqlite::Connection::open(&db.0).unwrap();
    let loads: i64 = conn
        .query_row("SELECT count(*) FROM relay_list_load", [], |r| r.get(0))
        .unwrap();
    let contents: i64 = conn
        .query_row("SELECT count(*) FROM relay_list_csv", [], |r| r.get(0))
        .unwrap();
    assert!(loads >= 4, "{loads} loads");
    assert_eq!(contents, 1);
}

#[tokio::test]
async fn failed_reload_keeps_the_current_list() {
    let (logs, _guard) = Logs::capture();
    let (a, rec_a) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        reloading(&clock),
        ios_from(&url),
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    eventually("REQ on A", || !rec_a.frames_of("REQ").is_empty()).await;
    *body.lock().unwrap() = None;
    eventually("WARN", || {
        logs.text()
            .contains("relay list reload failed, keeping the current one")
    })
    .await;
    assert!(
        logs.text()
            .lines()
            .any(|l| l.contains(" WARN ") && l.contains("reload failed"))
    );
    assert_eq!(
        monitor.status().await.unwrap().relays,
        std::slice::from_ref(&a)
    );
    assert_eq!(rec_a.open(), 1);

    // Back online with a new list: retried at the next cycle.
    let (b, rec_b) = recording_relay(false).await;
    *body.lock().unwrap() = Some(list_csv(&[&b]));
    eventually("REQ on B", || !rec_b.frames_of("REQ").is_empty()).await;
    drop(monitor.shutdown().await.unwrap());
}

#[tokio::test]
async fn start_falls_back_to_the_last_valid_list() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("reload-fallback");
    let (a, _rec_a) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        config("u09tv", &clock),
        ios_from(&url),
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    drop(monitor.shutdown().await.unwrap());

    *body.lock().unwrap() = None;
    // A day later: the list is downloaded again (a recent one would come
    // from the database).
    clock.set(clock.get() + 25 * 3600);
    let monitor = Monitor::start_from_sources(
        config("u09tv", &clock),
        ios_from(&url),
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        monitor.status().await.unwrap().relays,
        std::slice::from_ref(&a)
    );
    assert!(
        logs.text()
            .contains("relay list unavailable, using the last valid one")
    );
    drop(monitor.shutdown().await.unwrap());
}

/// A list with `url` twice, at two places: iOS rejects it (D18).
fn conflicting_csv(url: &str) -> String {
    format!("{}{url},10.0,10.0\n", list_csv(&[url]))
}

#[tokio::test]
async fn start_rejects_a_conflicting_ios_list_for_the_last_valid_one() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("reload-conflict");
    let (a, _rec_a) = recording_relay(false).await;
    let (b, rec_b) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        config("u09tv", &clock),
        ios_from(&url),
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    drop(monitor.shutdown().await.unwrap());

    *body.lock().unwrap() = Some(conflicting_csv(&b));
    // A day later: the list is downloaded again (a recent one would come
    // from the database).
    clock.set(clock.get() + 25 * 3600);
    let monitor = Monitor::start_from_sources(
        config("u09tv", &clock),
        ios_from(&url),
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        monitor.status().await.unwrap().relays,
        std::slice::from_ref(&a)
    );
    assert!(logs.text().lines().any(|l| l.contains(" WARN ")
        && l.contains("relay list rejected as iOS does, using the last valid one")
        && l.contains("iOS rejects this relay list")));
    assert_eq!(rec_b.accepted(), 0);
    // The rejected list is not recorded: it would become the fallback.
    let store = monitor.shutdown().await.unwrap();
    let last = store.last_relay_list(Platform::Ios).unwrap().unwrap();
    assert_eq!(last.csv, list_csv(&[&a]));
}

#[tokio::test]
async fn conflicting_ios_list_is_used_without_a_previous_one() {
    let (logs, _guard) = Logs::capture();
    let (b, rec_b) = recording_relay(false).await;
    let (url, _body) = list_server(Some(conflicting_csv(&b))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        config("u09tv", &clock),
        ios_from(&url),
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    eventually("REQ on B", || !rec_b.frames_of("REQ").is_empty()).await;
    assert!(
        logs.text().lines().any(|l| l.contains(" WARN ")
            && l.contains("iOS would reject it, but there is no previous one"))
    );
    drop(monitor.shutdown().await.unwrap());
}

#[tokio::test]
async fn reload_keeps_the_current_list_over_a_conflicting_one() {
    let (logs, _guard) = Logs::capture();
    let (a, rec_a) = recording_relay(false).await;
    let (b, rec_b) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        reloading(&clock),
        ios_from(&url),
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    eventually("REQ on A", || !rec_a.frames_of("REQ").is_empty()).await;
    *body.lock().unwrap() = Some(conflicting_csv(&b));
    eventually("WARN", || {
        logs.text().lines().any(|l| {
            l.contains("relay list reload failed, keeping the current one")
                && l.contains("iOS rejects this relay list")
        })
    })
    .await;
    assert_eq!(
        monitor.status().await.unwrap().relays,
        std::slice::from_ref(&a)
    );
    assert_eq!(rec_b.accepted(), 0);
    drop(monitor.shutdown().await.unwrap());
}

#[tokio::test]
async fn no_list_at_all_is_an_error() {
    let (url, _body) = list_server(None).await;
    let clock = TestClock::new();
    let result = Monitor::start_from_sources(
        config("u09tv", &clock),
        ios_from(&url),
        Store::open_in_memory().unwrap(),
    )
    .await;
    assert!(matches!(result, Err(Error::NoRelayList)));
}

#[tokio::test]
async fn retirements_survive_a_list_change() {
    let (a, _rec_a) = recording_relay(false).await;
    let (b, rec_b) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let monitor = Monitor::start_from_sources(
        reloading(&clock),
        ios_from(&url),
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    clock.set(clock.get() + inactivity());
    status_until(&monitor, "all retired", |s| s.watched.is_empty()).await;

    *body.lock().unwrap() = Some(list_csv(&[&b]));
    // Give the reload a few cycles: B must not be contacted.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let s = monitor.status().await.unwrap();
    assert_eq!((s.watched.len(), s.retired.len()), (0, 33));
    assert!(s.relays.is_empty());
    assert_eq!(rec_b.accepted(), 0);

    // Reinstating uses the new list.
    monitor.reinstate(vec!["u09tv".into()]).await.unwrap();
    eventually("REQ on B", || !rec_b.frames_of("REQ").is_empty()).await;
    drop(monitor.shutdown().await.unwrap());
}

// --- Lot 20: recent lists from the database ---

#[tokio::test]
async fn a_recent_list_is_not_downloaded_again() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("reload-recent");
    let (a, _rec_a) = recording_relay(false).await;
    let (b, _rec_b) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let start = |config: MonitorConfig, url: &str| {
        let (db, sources) = (db.0.clone(), ios_from(url));
        async move {
            Monitor::start_from_sources(config, sources, Store::open(&db).unwrap())
                .await
                .unwrap()
        }
    };
    let relays = async |m: Monitor| {
        let r = m.status().await.unwrap().relays;
        drop(m.shutdown().await.unwrap());
        r
    };
    // First start: downloaded.
    assert_eq!(
        relays(start(config("u09tv", &clock), &url).await).await,
        std::slice::from_ref(&a)
    );

    // The list changes on the server; within 24 h a start takes the one in
    // the database, without asking for it.
    *body.lock().unwrap() = Some(list_csv(&[&b]));
    clock.set(clock.get() + 3600);
    assert_eq!(
        relays(start(config("u09tv", &clock), &url).await).await,
        std::slice::from_ref(&a)
    );
    assert_eq!(logs.info_lines("relay list from the database (recent)"), 1);

    // Asked for (`--reload-lists`): downloaded.
    let forced = MonitorConfig {
        reload_lists: true,
        ..config("u09tv", &clock)
    };
    assert_eq!(
        relays(start(forced, &url).await).await,
        std::slice::from_ref(&b)
    );

    // After 24 h, or from another source: downloaded.
    *body.lock().unwrap() = Some(list_csv(&[&a]));
    clock.set(clock.get() + 24 * 3600);
    assert_eq!(
        relays(start(config("u09tv", &clock), &url).await).await,
        std::slice::from_ref(&a)
    );
    let (other, _) = list_server(Some(list_csv(&[&b]))).await;
    assert_eq!(
        relays(start(config("u09tv", &clock), &other).await).await,
        [b]
    );
    assert_eq!(logs.info_lines("relay list from the database (recent)"), 1);
}
