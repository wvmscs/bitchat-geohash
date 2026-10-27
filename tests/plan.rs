//! Listen plan on the frozen relay lists.

use std::collections::BTreeSet;
use std::time::Instant;

use bitchat_geohash::channels::{DEFAULT_MAX_CHANNELS, Level, enumerate};
use bitchat_geohash::directory::{Platform, RelayDirectory};
use bitchat_geohash::plan::{ListenPlan, PlanConfig, RelayChange};
use bitchat_geohash::select::select_both;
use serde_json::json;

const IOS: &str = include_str!("data/online_relays_gps.csv");
const ANDROID: &str = include_str!("data/nostr_relays.csv");

fn plan_with(channels: &[&str], config: PlanConfig) -> ListenPlan {
    let ios = RelayDirectory::parse(IOS, Platform::Ios);
    let android = RelayDirectory::parse(ANDROID, Platform::Android);
    let channels: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
    ListenPlan::build(Some(&ios), Some(&android), &channels, config).unwrap()
}

fn plan(channels: &[&str]) -> ListenPlan {
    plan_with(channels, PlanConfig::default())
}

fn small() -> PlanConfig {
    PlanConfig {
        max_tags_per_filter: 3,
        max_subs_per_conn: 2,
        ..PlanConfig::default()
    }
}

#[test]
fn one_channel_gets_its_relays() {
    let p = plan(&["u09tv"]);
    // 5 iOS + 5 Android, 2 in common (see tests/select.rs).
    assert_eq!(p.relays().len(), 8);
    for relay in p.relays() {
        let subs = p.subscriptions(&relay);
        assert_eq!(subs.len(), 1);
        assert_eq!(
            (subs[0].id.as_str(), subs[0].channels.as_slice()),
            ("geo0", &["u09tv".to_owned()][..])
        );
    }
    // 3 relays chosen by iOS only, 3 by Android only.
    assert_eq!(p.divergence("u09tv").len(), 6);
}

#[test]
fn neighbours_share_relays() {
    let p = plan(&["u09tv", "u09tu", "u09ty"]);
    let mut sum = 0;
    for c in ["u09tv", "u09tu", "u09ty"] {
        sum += p.selection(c).unwrap().len();
    }
    let relays = p.relays();
    assert!(
        relays.len() < sum,
        "{} relays for {sum} choices",
        relays.len()
    );
    // Each relay appears once, with the channels that chose it.
    assert_eq!(relays.iter().collect::<BTreeSet<_>>().len(), relays.len());
    let paris = p.subscriptions("wss://nostr.wild-vibes.ts.net");
    assert_eq!(paris.len(), 1);
    assert_eq!(paris[0].channels, ["u09tu", "u09tv", "u09ty"]);
}

#[test]
fn every_channel_is_listened_to_on_each_chosen_relay() {
    let channels = enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    let refs: Vec<&str> = channels.iter().map(String::as_str).collect();
    let p = plan_with(&refs, small());
    let ios = RelayDirectory::parse(IOS, Platform::Ios);
    let android = RelayDirectory::parse(ANDROID, Platform::Android);
    for c in &channels {
        for s in select_both(Some(&ios), Some(&android), c).unwrap() {
            let subs = p.subscriptions(&s.relay.url);
            assert_eq!(
                subs.iter().filter(|sub| sub.channels.contains(c)).count(),
                1,
                "{c} on {}",
                s.relay.url
            );
        }
    }
}

#[test]
fn groups_and_connections_respect_the_limits() {
    // 33 channels; Paris relays are chosen by all of them.
    let channels = enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    let refs: Vec<&str> = channels.iter().map(String::as_str).collect();
    let p = plan_with(&refs, small());
    let relay = "wss://nostr.wild-vibes.ts.net";
    let subs = p.subscriptions(relay);
    assert_eq!(subs.len(), 11);
    assert!(subs.iter().all(|s| s.channels.len() <= 3));
    let conns = p.connections(relay);
    assert_eq!(conns.len(), 6);
    assert!(conns.values().all(|c| c.len() <= 2));
    assert_eq!(conns[&5].len(), 1);
}

#[test]
fn filter_is_the_app_channel_filter() {
    let p = plan(&["u09tv", "u09tu"]);
    let sub = &p.subscriptions("wss://nostr.wild-vibes.ts.net")[0];
    assert_eq!(
        sub.filter(Some(1_700_000_000)).to_json(),
        json!({"kinds": [20000, 20001], "#g": ["u09tu", "u09tv"], "since": 1_700_000_000})
    );
}

#[test]
fn duplicates_count_once() {
    let p = plan(&["u09tv", "u09tv"]);
    assert_eq!(p.watched().len(), 1);
    assert_eq!(
        p.subscriptions("wss://nostr.wild-vibes.ts.net")[0].channels,
        ["u09tv"]
    );
}

#[test]
fn retiring_one_channel_only_touches_its_group() {
    let channels = enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    let refs: Vec<&str> = channels.iter().map(String::as_str).collect();
    let before = plan_with(&refs, small());
    let relay = "wss://nostr.wild-vibes.ts.net";
    let mut after = before.clone();
    // Groups of 3 in geohash order: u09t, u09t0, u09t1 | u09t2, ...
    assert_eq!(after.retire(&["u09t2"]), ["u09t2"]);
    assert!(after.is_retired("u09t2"));
    match &before.diff(&after).relays[relay] {
        RelayChange::Update { send, close } => {
            assert!(close.is_empty());
            assert_eq!(send.len(), 1);
            assert_eq!((send[0].id.as_str(), send[0].connection), ("geo1", 0));
            assert_eq!(send[0].channels, ["u09t3", "u09t4"]);
        }
        other => panic!("{other:?}"),
    }
    // Other subscriptions keep their id and connection.
    let kept: Vec<_> = after
        .subscriptions(relay)
        .into_iter()
        .filter(|s| s.id != "geo1")
        .collect();
    let orig: Vec<_> = before
        .subscriptions(relay)
        .into_iter()
        .filter(|s| s.id != "geo1")
        .collect();
    assert_eq!(kept, orig);
}

#[test]
fn emptied_group_is_closed_and_comes_back() {
    let channels = enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    let refs: Vec<&str> = channels.iter().map(String::as_str).collect();
    let before = plan_with(&refs, small());
    let relay = "wss://nostr.wild-vibes.ts.net";
    let mut after = before.clone();
    after.retire(&["u09t2", "u09t3", "u09t4"]);
    assert_eq!(
        before.diff(&after).relays[relay],
        RelayChange::Update {
            send: vec![],
            close: vec!["geo1".to_owned()]
        }
    );
    assert_eq!(
        after.reinstate(&["u09t3", "u09t4", "unknown"]),
        ["u09t3", "u09t4"]
    );
    // Retiring twice or reinstating a watched channel changes nothing.
    assert!(after.reinstate(&["u09t3"]).is_empty());
    assert_eq!(after.reinstate_all(), ["u09t2"]);
    assert!(before.diff(&after).is_empty());
}

#[test]
fn relay_without_channel_is_disconnected() {
    let before = plan(&["u09tv"]);
    let mut after = before.clone();
    after.retire(&["u09tv"]);
    assert_eq!(after.relays().len(), 0);
    let diff = before.diff(&after);
    assert_eq!(diff.relays.len(), 8);
    assert!(diff.relays.values().all(|c| *c == RelayChange::Disconnect));
    // And reconnected on reinstatement.
    let back = after.diff(&before);
    assert!(
        back.relays
            .values()
            .all(|c| matches!(c, RelayChange::Connect(subs) if subs.len() == 1))
    );
}

#[test]
fn dead_relays_are_dropped_not_replaced() {
    let before = plan(&["u09tv"]);
    let mut after = before.clone();
    after.drop_relays(&["wss://relay.cypherflow.ai"]);
    assert_eq!(after.relays().len(), 7);
    assert_eq!(after.all_relays().len(), 8);
    assert!(after.subscriptions("wss://relay.cypherflow.ai").is_empty());
    assert_eq!(after.dead_relays(), ["wss://relay.cypherflow.ai"]);
    let diff = before.diff(&after);
    assert_eq!(diff.relays.len(), 1);
    assert_eq!(
        diff.relays["wss://relay.cypherflow.ai"],
        RelayChange::Disconnect
    );
}

#[test]
fn new_plan_connects_new_relays() {
    let paris = plan(&["u09tv"]);
    let both = plan(&["u09tv", "9q8yy"]);
    let diff = paris.diff(&both);
    let connects = diff
        .relays
        .values()
        .filter(|c| matches!(c, RelayChange::Connect(_)))
        .count();
    // The 8 servers of 9q8yy (see tests/select.rs).
    assert_eq!(connects, 8);
    assert_eq!(diff.relays.len(), 8);
}

/// Spec acceptance criterion: plan of `u09`, depth 2, in under a second.
#[test]
#[ignore = "timing: run with --release -- --ignored"]
fn plan_of_u09_depth_2_under_a_second() {
    let start = Instant::now();
    let ios = RelayDirectory::parse(IOS, Platform::Ios);
    let android = RelayDirectory::parse(ANDROID, Platform::Android);
    let channels = enumerate("u09", 2, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    let p =
        ListenPlan::build(Some(&ios), Some(&android), &channels, PlanConfig::default()).unwrap();
    let relays = p.relays().len();
    let elapsed = start.elapsed();
    eprintln!("{} channels, {relays} relays, {elapsed:?}", channels.len());
    assert_eq!(channels.len(), 1_056);
    assert!(elapsed.as_secs_f64() < 1.0, "{elapsed:?}");
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|c| c.to_string()).collect()
}

#[test]
fn extend_keeps_the_subscriptions_already_sent() {
    let ios = RelayDirectory::parse(IOS, Platform::Ios);
    let android = RelayDirectory::parse(ANDROID, Platform::Android);
    let start = enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    let mut p = ListenPlan::build(Some(&ios), Some(&android), &start, small()).unwrap();
    let before = p.clone();
    let children = enumerate("u09tv", 1, &[Level::Neighbourhood], DEFAULT_MAX_CHANNELS).unwrap();
    let added = p.extend(Some(&ios), Some(&android), &children).unwrap();
    assert_eq!(added.len(), 32);
    assert_eq!(p.len(), 33 + 32);
    for relay in before.relays() {
        let old = before.subscriptions(&relay);
        let new = p.subscriptions(&relay);
        // Every old group keeps its id and channels, except the last one,
        // which may have been completed with new channels.
        for (i, sub) in old.iter().enumerate() {
            let same = new.iter().find(|s| s.id == sub.id).unwrap();
            if i + 1 < old.len() {
                assert_eq!(same, sub, "{relay} {}", sub.id);
            } else {
                assert!(
                    same.channels.starts_with(&sub.channels),
                    "{relay} {}",
                    sub.id
                );
            }
        }
    }
    let diff = before.diff(&p);
    assert!(!diff.is_empty());
    for change in diff.relays.values() {
        if let RelayChange::Update { close, .. } = change {
            assert!(close.is_empty(), "extending closes nothing");
        }
    }
    for c in &children {
        assert!(p.contains(c) && !p.is_retired(c));
    }
}

#[test]
fn extend_skips_known_channels_and_fills_the_last_group() {
    let dir = RelayDirectory::parse(
        "Relay URL,Latitude,Longitude\nwss://a.example,48.85,2.35\n",
        Platform::Ios,
    );
    let mut p =
        ListenPlan::build(Some(&dir), None, &strings(&["u09tv", "u09tu"]), small()).unwrap();
    let added = p
        .extend(
            Some(&dir),
            None,
            &strings(&["u09tu", "u09ty", "u09tz", "u09ty"]),
        )
        .unwrap();
    assert_eq!(added, ["u09ty", "u09tz"]);
    assert_eq!(p.relays().len(), 1);
    let subs = p.subscriptions(&p.relays()[0]);
    let groups: Vec<&[String]> = subs.iter().map(|s| s.channels.as_slice()).collect();
    assert_eq!(
        groups,
        [
            &strings(&["u09tu", "u09tv", "u09ty"])[..],
            &strings(&["u09tz"])[..]
        ]
    );
}

#[test]
fn extend_with_an_invalid_channel_changes_nothing() {
    let mut p = plan(&["u09tv"]);
    let before = p.clone();
    assert!(p.extend(None, None, &strings(&["u09tu", "u09ta"])).is_err());
    assert_eq!(p.len(), 1);
    assert!(before.diff(&p).is_empty());
}

#[test]
fn removing_channels_only_touches_their_groups() {
    let dir = RelayDirectory::parse(
        "Relay URL,Latitude,Longitude\nwss://a.example,48.85,2.35\nwss://b.example,-33.9,151.2\n",
        Platform::Ios,
    );
    let mut p =
        ListenPlan::build(Some(&dir), None, &strings(&["u09tu", "u09tv"]), small()).unwrap();
    let before = p.clone();
    p.extend(Some(&dir), None, &strings(&["u09ty", "u09tz", "u09tw"]))
        .unwrap();
    // Groups: [u09tu u09tv u09ty] [u09tz u09tw].
    let removed = p.remove(&["u09ty", "u09tz", "u09tw", "u09tx"]);
    assert_eq!(removed, ["u09tw", "u09ty", "u09tz"]);
    assert_eq!(p.len(), 2);
    assert!(!p.contains("u09ty"));
    // Back to the subscriptions before the preview; the emptied last
    // group is gone, so a later extend reuses its index.
    assert!(before.diff(&p).is_empty());
    p.extend(Some(&dir), None, &strings(&["u09tz"])).unwrap();
    let ids: Vec<String> = p
        .subscriptions("wss://a.example")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["geo0"]);
    // A relay left without any channel leaves the plan.
    assert!(p.remove(&["u09tu", "u09tv", "u09tz"]).len() == 3);
    assert!(p.all_relays().is_empty() && p.is_empty());
}
