//! Relay connections against local relays.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use bitchat_geohash::KIND_GEO_MESSAGE;
use bitchat_geohash::channels::{DEFAULT_MAX_CHANNELS, Level, enumerate};
use bitchat_geohash::listener::{ConnState, Listener, ListenerConfig, ListenerEvent};
use bitchat_geohash::nostr::Event;
use bitchat_geohash::nostr::unix_now;
use bitchat_geohash::plan::{ListenPlan, PlanConfig};
use common::*;
use tokio::sync::mpsc;

fn fast() -> ListenerConfig {
    ListenerConfig {
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        connect_timeout: Duration::from_secs(2),
        ..ListenerConfig::default()
    }
}

fn plan_on(url: &str, channels: &[String], config: PlanConfig) -> ListenPlan {
    let dir = directory_of(&[url.to_owned()]);
    ListenPlan::build(Some(&dir), None, channels, config).unwrap()
}

fn small() -> PlanConfig {
    PlanConfig {
        max_tags_per_filter: 3,
        max_subs_per_conn: 2,
        ..PlanConfig::default()
    }
}

fn u09t() -> Vec<String> {
    enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap()
}

fn message(who: &Identity, channel: &str) -> Event {
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        unix_now(),
        KIND_GEO_MESSAGE,
        tags,
        "salut".into(),
    )
    .sign(who)
    .unwrap()
}

async fn next_observation(rx: &mut mpsc::UnboundedReceiver<ListenerEvent>) -> Event {
    let wait = async {
        loop {
            if let Some(ListenerEvent::Observation(o)) = rx.recv().await {
                return o.event;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(3), wait)
        .await
        .expect("no observation")
}

#[tokio::test]
async fn delivered_event_becomes_an_observation() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let plan = plan_on(&relay.url(), &["u09tv".to_owned()], PlanConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    // Kinds 20000/20001 are ephemeral: the relay only forwards them live.
    wait_connected(&mut rx).await;
    let alice = Identity::generate();
    let live = message(&alice, "u09tv");
    relay.publish(live.clone());
    assert_eq!(next_observation(&mut rx).await.id, live.id);
    listener.shutdown().await;
    relay.shutdown().await;
}

/// Waits for a connection, plus a moment for the REQ to reach the relay.
async fn wait_connected(rx: &mut mpsc::UnboundedReceiver<ListenerEvent>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(ListenerEvent::State {
                state: ConnState::Connected,
                ..
            }) = rx.recv().await
            {
                return;
            }
        }
    })
    .await
    .expect("not connected");
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn reconnects_after_the_relay_goes_away() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let addr = relay.addr();
    let plan = plan_on(&relay.url(), &["u09tv".to_owned()], PlanConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    wait_connected(&mut rx).await;
    let alice = Identity::generate();
    let first = message(&alice, "u09tv");
    relay.publish(first.clone());
    assert_eq!(next_observation(&mut rx).await.id, first.id);

    relay.shutdown().await;
    let back = TestRelay::spawn(&addr.to_string()).await.unwrap();
    wait_connected(&mut rx).await;
    let second = message(&alice, "u09tv");
    back.publish(second.clone());
    assert_eq!(next_observation(&mut rx).await.id, second.id);
    listener.shutdown().await;
    back.shutdown().await;
}

#[tokio::test]
async fn subscriptions_follow_the_plan() {
    let (url, rec) = recording_relay(false).await;
    let channels = u09t();
    let plan = plan_on(&url, &channels, small());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    // 33 channels by 3: 11 subscriptions, by 2 per connection: 6 connections.
    eventually("11 REQ", || rec.frames_of("REQ").len() == 11).await;
    assert_eq!(rec.open(), 6);
    let reqs = rec.frames_of("REQ");
    let mut per_conn = std::collections::BTreeMap::<usize, usize>::new();
    let mut ids = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for (conn, v) in &reqs {
        *per_conn.entry(*conn).or_default() += 1;
        ids.insert(v[1].as_str().unwrap().to_owned());
        let filter = &v[2];
        assert_eq!(filter["kinds"], serde_json::json!([20000, 20001]));
        assert!(filter["since"].is_i64());
        let g = filter["#g"].as_array().unwrap();
        assert!(g.len() <= 3);
        seen.extend(g.iter().map(|c| c.as_str().unwrap().to_owned()));
    }
    assert!(per_conn.values().all(|&n| n <= 2), "{per_conn:?}");
    assert_eq!(ids.len(), 11);
    assert_eq!(seen, channels.iter().cloned().collect());
    listener.shutdown().await;
}

#[tokio::test]
async fn retiring_updates_or_closes_subscriptions() {
    let (url, rec) = recording_relay(false).await;
    let mut plan = plan_on(&url, &u09t(), small());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    eventually("11 REQ", || rec.frames_of("REQ").len() == 11).await;

    // geo1 = u09t2, u09t3, u09t4: one retired, the REQ is replaced.
    plan.retire(&["u09t2"]);
    listener.sync(&plan);
    eventually("replaced REQ", || rec.frames_of("REQ").len() == 12).await;
    let (_, last) = rec.frames_of("REQ").pop().unwrap();
    assert_eq!(last[1], "geo1");
    assert_eq!(last[2]["#g"], serde_json::json!(["u09t3", "u09t4"]));

    // Emptied: closed.
    plan.retire(&["u09t3", "u09t4"]);
    listener.sync(&plan);
    eventually("CLOSE geo1", || {
        rec.frames_of("CLOSE").iter().any(|(_, v)| v[1] == "geo1")
    })
    .await;

    // geo10 (u09tx, u09ty, u09tz) is alone on connection 5: retiring its
    // channels disconnects it.
    plan.retire(&["u09tx", "u09ty", "u09tz"]);
    listener.sync(&plan);
    eventually("connection 5 closed", || rec.open() == 5).await;
    assert_eq!(listener.connection_count(), 5);

    // Reinstating brings everything back.
    plan.reinstate_all();
    listener.sync(&plan);
    eventually("6 connections", || rec.open() == 6).await;
    listener.shutdown().await;
    eventually("all closed", || rec.open() == 0).await;
}

#[tokio::test]
async fn auth_challenges_are_ignored() {
    let (url, rec) = recording_relay(true).await;
    let plan = plan_on(&url, &["u09tv".to_owned()], PlanConfig::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    eventually("REQ", || rec.frames_of("REQ").len() == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rec.frames_of("AUTH").is_empty());
    assert!(
        rec.frames
            .lock()
            .unwrap()
            .iter()
            .all(|(_, v)| v[0] != "EVENT")
    );
    listener.shutdown().await;
}

#[tokio::test]
async fn auth_required_relay_is_abandoned_without_retry() {
    let url = scripted_relay(&[r#"["CLOSED","{sub}","auth-required: members only"]"#]).await;
    let plan = plan_on(&url, &["u09tv".to_owned()], PlanConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    let abandoned = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(ListenerEvent::State {
                state: ConnState::Abandoned { auth, .. },
                ..
            }) = rx.recv().await
            {
                return auth;
            }
        }
    })
    .await
    .unwrap();
    assert!(abandoned);
    // No reconnection attempt afterwards.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(e) = rx.try_recv() {
        assert!(
            !matches!(
                e,
                ListenerEvent::State {
                    state: ConnState::Connecting,
                    ..
                }
            ),
            "{e:?}"
        );
    }
    listener.shutdown().await;
}

#[tokio::test]
async fn closed_port_is_abandoned_and_unreachable_retried() {
    let dead = closed_port().await;
    let plan = plan_on(&dead, &["u09tv".to_owned()], PlanConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut listener = Listener::new(fast(), tx);
    listener.sync(&plan);
    let state = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(ListenerEvent::State {
                state: s @ ConnState::Abandoned { .. },
                ..
            }) = rx.recv().await
            {
                return s;
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(state, ConnState::Abandoned { auth: false, .. }));
    listener.shutdown().await;

    // A silent server is only "down": waiting, then retried.
    let silent = silent_server().await;
    let plan = plan_on(&silent, &["u09tv".to_owned()], PlanConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let config = ListenerConfig {
        connect_timeout: Duration::from_millis(100),
        ..fast()
    };
    let mut listener = Listener::new(config, tx);
    listener.sync(&plan);
    let mut connecting = 0;
    tokio::time::timeout(Duration::from_secs(3), async {
        while connecting < 2 {
            if let Some(ListenerEvent::State {
                state: ConnState::Connecting,
                ..
            }) = rx.recv().await
            {
                connecting += 1;
            }
        }
    })
    .await
    .expect("retried");
    listener.shutdown().await;
}
