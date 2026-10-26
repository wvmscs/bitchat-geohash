//! The monitor end to end, against local relays, with an injected clock and
//! captured logs.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitchat_envelope::envelope::unix_now;
use bitchat_envelope::mock::MockRelay;
use bitchat_envelope::mock::ws::WsRelay;
use bitchat_envelope::{Event, Identity};
use bitchat_geohash::KIND_GEO_MESSAGE;
use bitchat_geohash::listener::{ConnState, ListenerConfig};
use bitchat_geohash::monitor::{Monitor, MonitorConfig, MonitorStatus};
use bitchat_geohash::probe::Status;
use bitchat_geohash::store::Store;
use common::*;

/// Captured log output (current-thread runtime: the monitor task logs on
/// the test thread).
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    fn info_lines(&self, needle: &str) -> usize {
        self.text()
            .lines()
            .filter(|l| l.contains(" INFO ") && l.contains(needle))
            .count()
    }
}

struct TestClock(Arc<AtomicI64>);

impl TestClock {
    fn new() -> Self {
        TestClock(Arc::new(AtomicI64::new(unix_now())))
    }
    fn set(&self, t: i64) {
        self.0.store(t, Ordering::SeqCst);
    }
    fn get(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn config(start: &str, clock: &TestClock) -> MonitorConfig {
    let c = clock.0.clone();
    MonitorConfig {
        depth: 1,
        probe: false,
        tick: Duration::from_millis(30),
        listener: ListenerConfig {
            backoff_min: Duration::from_millis(50),
            backoff_max: Duration::from_millis(200),
            clock: Arc::new(move || c.load(Ordering::SeqCst)),
            ..ListenerConfig::default()
        },
        ..MonitorConfig::new(start)
    }
}

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

async fn status_until(
    monitor: &Monitor,
    what: &str,
    check: impl Fn(&MonitorStatus) -> bool,
) -> MonitorStatus {
    for _ in 0..150 {
        let s = monitor.status().await.unwrap();
        if check(&s) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "timed out waiting for: {what}: {:?}",
        monitor.status().await.unwrap()
    );
}

fn connected(s: &MonitorStatus) -> bool {
    !s.connections.is_empty()
        && s.connections
            .iter()
            .all(|c| c.state == ConnState::Connected)
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        TempDb(std::env::temp_dir().join(format!(
            "bitchat-geohash-monitor-{name}-{}.sqlite",
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

#[tokio::test]
async fn inactive_channels_are_retired_and_logged() {
    let (logs, _guard) = Logs::capture();
    let relay = WsRelay::spawn("127.0.0.1:0", MockRelay::new())
        .await
        .unwrap();
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
    relay.relay.publish(message(&alice, "u09tvq", base));
    clock.set(base + 200);
    relay.relay.publish(message(&alice, "u09tvq", base + 200));
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

    clock.set(base + 300);
    let s = status_until(&monitor, "32 retired", |s| s.retired.len() == 32).await;
    assert_eq!(s.watched, ["u09tvq"]);
    assert_eq!(s.relays, [relay.url()], "still needed by u09tvq");
    assert_eq!(
        logs.info_lines("retired from monitoring: inactive for 5 min"),
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
    let relay = WsRelay::spawn("127.0.0.1:0", MockRelay::new())
        .await
        .unwrap();
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
    clock.set(base + 300);
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
    clock.set(base + 300 + 299);
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
    let relay = WsRelay::spawn("127.0.0.1:0", MockRelay::new())
        .await
        .unwrap();
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
    clock.set(clock.get() + 300);
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
