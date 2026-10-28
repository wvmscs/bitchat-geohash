//! The monitor end to end, against local relays, with an injected clock and
//! captured logs.

mod common;

use std::time::Duration;

use bitchat_geohash::KIND_GEO_MESSAGE;
use bitchat_geohash::monitor::Monitor;
use bitchat_geohash::nostr::Event;
use bitchat_geohash::probe::Status;
use bitchat_geohash::store::Store;
use common::*;

fn message(who: &Identity, channel: &str, created_at: i64) -> Event {
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        created_at,
        KIND_GEO_MESSAGE,
        tags,
        "salut".into(),
    )
    .sign(who)
    .unwrap()
}

#[tokio::test]
async fn inactive_channels_are_retired_and_logged() {
    let (logs, _guard) = Logs::capture();
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let base = clock.get();
    let dir = directory_of(&[relay.url()]);
    let mut cfg = config("u09tv", &clock);
    cfg.probe = true;
    let monitor = Monitor::start(cfg, Some(dir), None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    let s = status_until(&monitor, "connected", connected).await;
    assert_eq!(s.watched.len(), 33);
    // Let the REQ reach the relay: it only forwards ephemeral kinds live.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // u09tvq speaks at base and at base + 200; the others never do.
    let alice = Identity::generate();
    relay.publish(message(&alice, "u09tvq", base));
    clock.set(base + 200);
    relay.publish(message(&alice, "u09tvq", base + 200));
    for _ in 0..150 {
        let snap = monitor.snapshot().await.unwrap();
        if snap
            .iter()
            .any(|c| c.geohash == "u09tvq" && c.messages == 2)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(monitor.status().await.unwrap().retired.is_empty());

    clock.set(base + inactivity());
    let s = status_until(&monitor, "32 retired", |s| s.retired.len() == 32).await;
    assert_eq!(s.watched, ["u09tvq"]);
    assert_eq!(s.relays, [relay.url()], "still needed by u09tvq");
    assert_eq!(
        logs.info_lines(&format!(
            "retired from monitoring: inactive for {} min",
            inactivity() / 60
        )),
        32
    );
    assert_eq!(logs.info_lines("channel u09tv retired"), 1);

    let store = monitor.shutdown().await.unwrap();
    assert_eq!(store.retired_channels().unwrap().len(), 32);
    assert_eq!(store.last_probe(&relay.url()).unwrap().unwrap().1, "ok");
    relay.shutdown().await;
}

#[tokio::test]
async fn retired_channels_survive_a_restart_until_reinstated() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("restart");
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let base = clock.get();
    let dir = || Some(directory_of(&[relay.url()]));

    // Run 1: nobody speaks, everything is retired, the relay is let go.
    let monitor = Monitor::start(
        config("u09tv", &clock),
        dir(),
        None,
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    status_until(&monitor, "connected", connected).await;
    clock.set(base + inactivity());
    let s = status_until(&monitor, "all retired", |s| s.watched.is_empty()).await;
    assert!(s.relays.is_empty());
    drop(monitor.shutdown().await.unwrap());

    // Run 2: still retired; reinstating one channel reconnects.
    let monitor = Monitor::start(
        config("u09tv", &clock),
        dir(),
        None,
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    let s = monitor.status().await.unwrap();
    assert_eq!((s.watched.len(), s.retired.len()), (0, 33));
    assert!(s.relays.is_empty() && s.connections.is_empty());
    let back = monitor
        .reinstate(vec!["u09tv".into(), "u09".into(), "u09tvq".into()])
        .await
        .unwrap();
    assert_eq!(back, ["u09tv", "u09tvq"], "unknown channels are ignored");
    assert_eq!(logs.info_lines("channel u09tv reinstated on request"), 1);
    let s = status_until(&monitor, "reconnected", connected).await;
    assert_eq!(s.watched, ["u09tv", "u09tvq"]);
    // The delay starts again from the reinstatement.
    clock.set(base + 2 * inactivity() - 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(monitor.status().await.unwrap().watched.len(), 2);
    assert_eq!(monitor.reinstate_all().await.unwrap().len(), 31);
    drop(monitor.shutdown().await.unwrap());

    // Run 3: nothing retired any more.
    let monitor = Monitor::start(
        config("u09tv", &clock),
        dir(),
        None,
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    assert!(monitor.status().await.unwrap().retired.is_empty());
    drop(monitor.shutdown().await.unwrap());
    relay.shutdown().await;
}

#[tokio::test]
async fn reinstate_all_at_start() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("reinstate-all");
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let dir = || Some(directory_of(&[relay.url()]));
    let monitor = Monitor::start(
        config("u09tv", &clock),
        dir(),
        None,
        Store::open(&db.0).unwrap(),
    )
    .await
    .unwrap();
    clock.set(clock.get() + inactivity());
    status_until(&monitor, "all retired", |s| s.watched.is_empty()).await;
    drop(monitor.shutdown().await.unwrap());

    let mut cfg = config("u09tv", &clock);
    cfg.reinstate_all = true;
    let monitor = Monitor::start(cfg, dir(), None, Store::open(&db.0).unwrap())
        .await
        .unwrap();
    let s = monitor.status().await.unwrap();
    assert_eq!((s.watched.len(), s.retired.len()), (33, 0));
    assert_eq!(logs.info_lines("reinstated (--reinstate-all)"), 33);
    let store = monitor.shutdown().await.unwrap();
    assert!(store.retired_channels().unwrap().is_empty());
    relay.shutdown().await;
}

#[tokio::test]
async fn relay_requiring_auth_is_set_aside() {
    let url = scripted_relay(&[r#"["CLOSED","{sub}","auth-required: members only"]"#]).await;
    let clock = TestClock::new();
    let monitor = Monitor::start(
        config("u09tv", &clock),
        Some(directory_of(std::slice::from_ref(&url))),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    let s = status_until(&monitor, "dropped", |s| !s.dropped_relays.is_empty()).await;
    assert_eq!(s.dropped_relays, std::slice::from_ref(&url));
    assert!(s.relays.is_empty());
    let snap = monitor.snapshot().await.unwrap();
    assert_eq!(snap[0].relays[0].status, Some(Status::Auth));
    let store = monitor.shutdown().await.unwrap();
    assert_eq!(store.last_probe(&url).unwrap().unwrap().1, "auth");
}

// --- Lot 20: the area's relays first, the others in the background ---

/// Probing on, the area's own channel left out (level 5 only under `u09t`):
/// nothing is probed before listening.
fn probing(clock: &TestClock, background: bool) -> bitchat_geohash::monitor::MonitorConfig {
    bitchat_geohash::monitor::MonitorConfig {
        probe: true,
        probe_in_background: background,
        probe_timeout: Duration::from_secs(2),
        levels: vec![bitchat_geohash::channels::Level::City],
        ..config("u09t", clock)
    }
}

#[tokio::test]
async fn other_relays_are_probed_after_the_start() {
    let (silent, dead) = (silent_server().await, closed_port().await);
    let clock = TestClock::new();
    let started = std::time::Instant::now();
    let monitor = Monitor::start(
        probing(&clock, true),
        Some(directory_of(&[silent.clone(), dead.clone()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    // Not held by the silent relay's 2 s.
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    // The dead one is set aside when its verdict comes.
    let s = status_until(&monitor, "dropped", |s| !s.dropped_relays.is_empty()).await;
    assert_eq!(s.dropped_relays, std::slice::from_ref(&dead));
    drop(monitor.shutdown().await.unwrap());

    // All first, as before: the start waits for the silent one.
    let started = std::time::Instant::now();
    let monitor = Monitor::start(
        probing(&clock, false),
        Some(directory_of(&[silent, dead.clone()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(monitor.status().await.unwrap().dropped_relays, [dead]);
    drop(monitor.shutdown().await.unwrap());
}

#[tokio::test]
async fn the_areas_own_relays_are_probed_first() {
    let silent = silent_server().await;
    let clock = TestClock::new();
    let started = std::time::Instant::now();
    // `u09t` is a channel here (levels 4 and 5): its relays come first.
    let config = bitchat_geohash::monitor::MonitorConfig {
        levels: vec![
            bitchat_geohash::channels::Level::Province,
            bitchat_geohash::channels::Level::City,
        ],
        ..probing(&clock, true)
    };
    let monitor = Monitor::start(
        config,
        Some(directory_of(&[silent])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2));
    drop(monitor.shutdown().await.unwrap());
}

#[tokio::test]
async fn recent_verdicts_are_not_probed_again() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("recent-verdicts");
    let dead = closed_port().await;
    let clock = TestClock::new();
    let start = || {
        Monitor::start(
            probing(&clock, true),
            Some(directory_of(std::slice::from_ref(&dead))),
            None,
            Store::open(&db.0).unwrap(),
        )
    };
    let rows = || -> i64 {
        rusqlite::Connection::open(&db.0)
            .unwrap()
            .query_row("SELECT count(*) FROM probe_result", [], |r| r.get(0))
            .unwrap()
    };
    let monitor = start().await.unwrap();
    status_until(&monitor, "dropped", |s| !s.dropped_relays.is_empty()).await;
    drop(monitor.shutdown().await.unwrap());
    let after_first = rows();
    assert!(after_first >= 1);

    // Within the hour: set aside at once, from the database, without a
    // probe nor a connection.
    clock.set(clock.get() + 600);
    let monitor = start().await.unwrap();
    assert_eq!(
        monitor.status().await.unwrap().dropped_relays,
        std::slice::from_ref(&dead)
    );
    assert_eq!(logs.info_lines("recent relay verdicts reused"), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(monitor.shutdown().await.unwrap());
    assert_eq!(rows(), after_first, "nothing probed again");

    // An hour later: probed again.
    clock.set(clock.get() + 3600);
    let monitor = start().await.unwrap();
    status_until(&monitor, "dropped", |s| !s.dropped_relays.is_empty()).await;
    drop(monitor.shutdown().await.unwrap());
    assert!(rows() > after_first);
}
