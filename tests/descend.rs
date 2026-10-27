//! Channels added while running (descent into a cell), relay information
//! and change notifications, against local relays.

mod common;

use std::time::Duration;

use bitchat_geohash::directory::{Platform, RelayDirectory};
use bitchat_geohash::monitor::{Monitor, MonitorEvent};
use bitchat_geohash::nostr::Event;
use bitchat_geohash::probe::Status;
use bitchat_geohash::select::geohash_center;
use bitchat_geohash::store::Store;
use bitchat_geohash::{Error, KIND_GEO_MESSAGE};
use common::*;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

fn message(who: &Identity, channel: &str, created_at: i64) -> Event {
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        created_at,
        KIND_GEO_MESSAGE,
        tags,
        "hello".into(),
    )
    .sign(who)
    .unwrap()
}

/// Waits for the first event matching `want`, skipping the others.
async fn next_event(
    rx: &mut tokio::sync::broadcast::Receiver<MonitorEvent>,
    want: impl Fn(&MonitorEvent) -> bool,
) -> MonitorEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let e = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("timed out waiting for a monitor event")
            .unwrap();
        if want(&e) {
            return e;
        }
    }
}

#[tokio::test]
async fn descent_listens_to_the_children_at_once() {
    let (logs, _guard) = Logs::capture();
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let cfg = config("u09t", &clock);
    let monitor = Monitor::start(
        cfg,
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    let s = status_until(&monitor, "connected", connected).await;
    assert_eq!(s.watched.len(), 33);
    let mut events = monitor.subscribe();

    let added = monitor.descend("U09TV").await.unwrap();
    assert_eq!(added.len(), 32);
    assert!(added.iter().all(|g| g.len() == 6 && g.starts_with("u09tv")));
    assert_eq!(
        next_event(&mut events, |e| matches!(e, MonitorEvent::Added(_))).await,
        MonitorEvent::Added(added.clone())
    );
    assert_eq!(monitor.status().await.unwrap().watched.len(), 65);
    assert_eq!(logs.info_lines("descended into u09tv: 32 channels"), 1);

    // A second descent into the same cell changes nothing.
    assert!(monitor.descend("u09tv").await.unwrap().is_empty());

    // The new channels are subscribed: an event there is counted.
    status_until(&monitor, "connected", connected).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    relay.publish(message(&Identity::generate(), "u09tvq", clock.get()));
    for _ in 0..150 {
        let snap = monitor.snapshot().await.unwrap();
        if snap
            .iter()
            .any(|c| c.geohash == "u09tvq" && c.messages == 1)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let snap = monitor.snapshot().await.unwrap();
    let cell = snap.iter().find(|c| c.geohash == "u09tvq").unwrap();
    assert_eq!(cell.messages, 1);

    let store = monitor.shutdown().await.unwrap();
    assert_eq!(store.descents().unwrap(), added);
    relay.shutdown().await;
}

#[tokio::test]
async fn refused_descents_change_nothing() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let mut cfg = config("u09t", &clock);
    cfg.max_channels = 40;
    let monitor = Monitor::start(
        cfg,
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    match monitor.descend("u09tv").await {
        Err(Error::TooManyChannels { count, max }) => assert_eq!((count, max), (65, 40)),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        monitor.descend("u09ty").await,
        Err(Error::TooManyChannels { .. })
    ));
    assert!(matches!(
        monitor.descend("u0bc").await,
        Err(Error::OutsideArea { .. })
    ));
    assert!(matches!(
        monitor.descend("u09").await,
        Err(Error::OutsideArea { .. })
    ));
    assert!(matches!(
        monitor.descend("u09ta!").await,
        Err(Error::InvalidGeohash(_))
    ));
    assert!(matches!(
        monitor
            .watch_channels(vec!["u09tvq".into(), "u0bcd".into()])
            .await,
        Err(Error::OutsideArea { .. })
    ));
    assert!(matches!(
        monitor.watch_channels(vec!["u09tvqp1".into()]).await,
        Err(Error::InvalidGeohash(_))
    ));
    assert_eq!(monitor.status().await.unwrap().watched.len(), 33);

    // Level 7 is reachable on request; nothing below it.
    let added = monitor
        .watch_channels(vec!["u09tvqp".into()])
        .await
        .unwrap();
    assert_eq!(added, ["u09tvqp"]);
    assert!(matches!(
        monitor.descend("u09tvqp").await,
        Err(Error::NoFinerLevel(_))
    ));
    monitor.shutdown().await.unwrap();
    relay.shutdown().await;
}

#[tokio::test]
async fn descents_are_restored_at_start_unless_forgotten() {
    let (logs, _guard) = Logs::capture();
    let db = TempDb::new("descents");
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let dir = directory_of(&[relay.url()]);
    let start = |cfg| Monitor::start(cfg, Some(dir.clone()), None, Store::open(&db.0).unwrap());

    let monitor = start(config("u09t", &clock)).await.unwrap();
    monitor.descend("u09tv").await.unwrap();
    monitor.shutdown().await.unwrap();

    let monitor = start(config("u09t", &clock)).await.unwrap();
    assert_eq!(monitor.status().await.unwrap().watched.len(), 65);
    assert_eq!(logs.info_lines("channels of previous descents restored"), 1);
    monitor.shutdown().await.unwrap();

    // Another area does not get them.
    let monitor = start(config("u09w", &clock)).await.unwrap();
    assert_eq!(monitor.status().await.unwrap().watched.len(), 33);
    monitor.shutdown().await.unwrap();

    let mut cfg = config("u09t", &clock);
    cfg.forget_descents = true;
    let monitor = start(cfg).await.unwrap();
    assert_eq!(monitor.status().await.unwrap().watched.len(), 33);
    let store = monitor.shutdown().await.unwrap();
    assert!(store.descents().unwrap().is_empty());
    relay.shutdown().await;
}

#[tokio::test]
async fn retired_descents_stay_retired_after_a_restart() {
    let db = TempDb::new("retired-descents");
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let base = clock.get();
    let dir = directory_of(&[relay.url()]);
    let start = |cfg| Monitor::start(cfg, Some(dir.clone()), None, Store::open(&db.0).unwrap());

    let monitor = start(config("u09t", &clock)).await.unwrap();
    let mut events = monitor.subscribe();
    monitor.descend("u09tv").await.unwrap();
    clock.set(base + inactivity());
    let retired = next_event(&mut events, |e| matches!(e, MonitorEvent::Retired(_))).await;
    let MonitorEvent::Retired(retired) = retired else {
        unreachable!()
    };
    assert_eq!(retired.len(), 65);
    monitor.shutdown().await.unwrap();

    let monitor = start(config("u09t", &clock)).await.unwrap();
    let s = monitor.status().await.unwrap();
    assert_eq!((s.watched.len(), s.retired.len()), (0, 65));
    // Descending again reinstates them.
    let back = monitor.descend("u09tv").await.unwrap();
    assert_eq!(back.len(), 32);
    assert_eq!(monitor.status().await.unwrap().watched.len(), 32);
    monitor.shutdown().await.unwrap();
    relay.shutdown().await;
}

/// A relay list: `live` relays at the centre of `u09t`, one dead relay at
/// the centre of `u09t0`. Only channels near `u09t0`, from a descent,
/// choose the dead one.
fn directory_around(live: &[String], dead: &str) -> RelayDirectory {
    let (lat, lon) = geohash_center("u09t").unwrap();
    let (dlat, dlon) = geohash_center("u09t0").unwrap();
    let mut csv = String::from("Relay URL,Latitude,Longitude\n");
    for url in live {
        csv.push_str(&format!("{url},{lat},{lon}\n"));
    }
    csv.push_str(&format!("{dead},{dlat},{dlon}\n"));
    RelayDirectory::parse(&csv, Platform::Ios)
}

#[tokio::test]
async fn new_relays_of_a_descent_are_probed() {
    let mut live = Vec::new();
    for _ in 0..5 {
        live.push(TestRelay::spawn("127.0.0.1:0").await.unwrap());
    }
    let urls: Vec<String> = live.iter().map(TestRelay::url).collect();
    let dead = closed_port().await;
    let clock = TestClock::new();
    let mut cfg = config("u09t", &clock);
    cfg.depth = 0;
    cfg.probe = true;
    let monitor = Monitor::start(
        cfg,
        Some(directory_around(&urls, &dead)),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    let relays = monitor.relays().await.unwrap();
    assert_eq!(relays.len(), 5, "the dead relay is not chosen yet");
    assert!(relays.iter().all(|r| r.status == Some(Status::Ok)));

    let mut events = monitor.subscribe();
    monitor.descend("u09t").await.unwrap();
    next_event(&mut events, |e| *e == MonitorEvent::RelaysChanged).await;
    let mut found = None;
    for _ in 0..150 {
        let relays = monitor.relays().await.unwrap();
        found = relays
            .into_iter()
            .find(|r| r.url.contains(dead.trim_start_matches("ws://")));
        if found.as_ref().is_some_and(|r| r.status.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let r = found.expect("the dead relay is in the plan after the descent");
    assert_eq!(r.status, Some(Status::Dead));
    assert!(!r.listened);
    assert!(
        (1..32).contains(&r.channels),
        "chosen by u09t0 and some neighbours only"
    );
    let s = monitor.status().await.unwrap();
    assert!(s.dropped_relays.contains(&r.url));
    monitor.shutdown().await.unwrap();
    for relay in live {
        relay.shutdown().await;
    }
}

#[tokio::test]
async fn relay_information() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let monitor = Monitor::start(
        config("u09tv", &clock),
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    status_until(&monitor, "connected", connected).await;
    let relays = monitor.relays().await.unwrap();
    assert_eq!(relays.len(), 1);
    let r = &relays[0];
    assert_eq!((r.lat, r.lon), (48.8566, 2.3522));
    assert_eq!((r.ios, r.android), (true, false));
    assert_eq!((r.status, r.listened, r.channels), (None, true, 33));
    assert_eq!(r.connections.len(), 1);
    monitor.shutdown().await.unwrap();
    relay.shutdown().await;
}

#[tokio::test]
async fn subscribers_get_ticks_and_lag_when_slow() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let mut cfg = config("u09tv", &clock);
    cfg.tick = Duration::from_millis(1);
    let monitor = Monitor::start(
        cfg,
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    let mut fast = monitor.subscribe();
    next_event(&mut fast, |e| matches!(e, MonitorEvent::Tick { .. })).await;

    let mut slow = monitor.subscribe();
    assert_eq!(slow.try_recv(), Err(TryRecvError::Empty));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(matches!(slow.recv().await, Err(RecvError::Lagged(_))));
    // After the lag, it reads on.
    assert!(slow.recv().await.is_ok());
    monitor.shutdown().await.unwrap();
    relay.shutdown().await;
}

#[tokio::test]
async fn descents_survive_a_relay_list_reload() {
    let (a, _rec_a) = recording_relay(false).await;
    let (b, rec_b) = recording_relay(false).await;
    let (url, body) = list_server(Some(list_csv(&[&a]))).await;
    let clock = TestClock::new();
    let cfg = bitchat_geohash::monitor::MonitorConfig {
        reload_every: Duration::from_millis(150),
        ..config("u09t", &clock)
    };
    let sources = bitchat_geohash::monitor::ListSources {
        ios: Some(bitchat_geohash::directory::ListSource::Url(url)),
        android: None,
    };
    let monitor = Monitor::start_from_sources(cfg, sources, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    let mut events = monitor.subscribe();
    monitor.descend("u09tv").await.unwrap();

    *body.lock().unwrap() = Some(list_csv(&[&b]));
    next_event(&mut events, |e| *e == MonitorEvent::PlanChanged).await;
    assert_eq!(monitor.status().await.unwrap().watched.len(), 65);
    // B gets the descended channels too.
    eventually("u09tvq subscribed on B", || {
        rec_b
            .frames_of("REQ")
            .iter()
            .any(|(_, f)| f.to_string().contains("u09tvq"))
    })
    .await;
    monitor.shutdown().await.unwrap();
}

/// Waits until `count` channels are watched.
async fn watched_until(monitor: &Monitor, count: usize) {
    for _ in 0..150 {
        if monitor.status().await.unwrap().watched.len() == count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never {count} watched channels");
}

#[tokio::test]
async fn previews_listen_for_a_while_and_save_nothing() {
    let (logs, _guard) = Logs::capture();
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let mut cfg = config("u09t", &clock);
    cfg.preview_for = Duration::from_secs(300);
    let monitor = Monitor::start(
        cfg,
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    status_until(&monitor, "connected", connected).await;
    let mut events = monitor.subscribe();

    let added = monitor.preview("u09tv").await.unwrap();
    assert_eq!(added.len(), 32);
    next_event(&mut events, |e| matches!(e, MonitorEvent::Added(_))).await;
    let snap = monitor.snapshot().await.unwrap();
    assert_eq!(snap.iter().filter(|c| c.preview).count(), 32);
    assert!(
        !snap
            .iter()
            .any(|c| c.preview && !c.geohash.starts_with("u09tv"))
    );
    // Already previewed: the delay is extended, nothing added. Channels
    // already watched are not previewed.
    assert!(monitor.preview("u09tv").await.unwrap().is_empty());
    assert!(monitor.preview("u09t").await.unwrap().is_empty());
    assert_eq!(logs.info_lines("preview of u09tv: 32 channels"), 1);

    // An event there is counted.
    status_until(&monitor, "connected", connected).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    relay.publish(message(&Identity::generate(), "u09tvq", clock.get()));
    for _ in 0..150 {
        let snap = monitor.snapshot().await.unwrap();
        if snap
            .iter()
            .any(|c| c.geohash == "u09tvq" && c.messages == 1)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Past the delay, the preview ends: the channels leave the plan, they
    // are not retired.
    clock.set(clock.get() + 300);
    match next_event(&mut events, |e| matches!(e, MonitorEvent::Removed(_))).await {
        MonitorEvent::Removed(gone) => assert_eq!(gone, added),
        _ => unreachable!(),
    }
    let s = monitor.status().await.unwrap();
    assert_eq!((s.watched.len(), s.retired.len()), (33, 0));
    assert_eq!(monitor.snapshot().await.unwrap().len(), 33);
    assert_eq!(logs.info_lines("preview of u09tv ended (delay over)"), 1);

    let store = monitor.shutdown().await.unwrap();
    assert!(store.descents().unwrap().is_empty());
    relay.shutdown().await;
}

#[tokio::test]
async fn a_descent_keeps_the_previewed_channels() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let monitor = Monitor::start(
        config("u09t", &clock),
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    let previewed = monitor.preview("u09tv").await.unwrap();
    let kept = monitor.descend("u09tv").await.unwrap();
    assert_eq!(kept, previewed);
    assert!(!monitor.snapshot().await.unwrap().iter().any(|c| c.preview));
    // No longer a preview: nothing ends after the delay.
    clock.set(clock.get() + 300);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(monitor.status().await.unwrap().watched.len(), 65);
    let store = monitor.shutdown().await.unwrap();
    assert_eq!(store.descents().unwrap(), kept);
    relay.shutdown().await;
}

#[tokio::test]
async fn previews_give_way_when_room_is_needed() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let mut cfg = config("u09t", &clock);
    cfg.max_channels = 33 + 32 + 32;
    let monitor = Monitor::start(
        cfg,
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(monitor.preview("u09tu").await.unwrap().len(), 32);
    clock.set(clock.get() + 10);
    assert_eq!(monitor.preview("u09tv").await.unwrap().len(), 32);
    // A third preview ends the oldest one.
    assert_eq!(monitor.preview("u09tw").await.unwrap().len(), 32);
    let previewed: Vec<String> = monitor
        .snapshot()
        .await
        .unwrap()
        .into_iter()
        .filter(|c| c.preview)
        .map(|c| c.geohash[..5].to_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(previewed, ["u09tv", "u09tw"]);
    // A descent too, and it fails only if previews cannot make room.
    assert_eq!(monitor.descend("u09ty").await.unwrap().len(), 32);
    watched_until(&monitor, 33 + 32 + 32).await;
    assert_eq!(monitor.descend("u09tz").await.unwrap().len(), 32);
    assert!(!monitor.snapshot().await.unwrap().iter().any(|c| c.preview));
    // No preview left to end: refused, and nothing changed.
    assert!(matches!(
        monitor.descend("u09tx").await,
        Err(Error::TooManyChannels { .. })
    ));
    assert!(matches!(
        monitor.preview("u09tx").await,
        Err(Error::TooManyChannels { .. })
    ));
    assert_eq!(monitor.status().await.unwrap().watched.len(), 97);
    monitor.shutdown().await.unwrap();
    relay.shutdown().await;
}
