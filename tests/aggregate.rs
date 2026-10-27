//! Aggregation: deduplication, checks, window, k threshold, meshes,
//! inactivity, per-minute buckets, privacy. Clock injected (`now`).

mod common;

use std::collections::BTreeMap;

use bitchat_geohash::aggregate::{Aggregator, AggregatorConfig, Observation, Rejection};
use bitchat_geohash::nostr::Event;
use bitchat_geohash::plan::{ListenPlan, PlanConfig};
use bitchat_geohash::probe::{Status, Verdict};
use bitchat_geohash::{KIND_GEO_MESSAGE, KIND_GEO_PRESENCE};
use common::{Identity, Sign, directory_of};

const T0: i64 = 1_700_000_040; // a minute boundary

fn event(who: &Identity, channel: &str, kind: u32, created_at: i64) -> Event {
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        created_at,
        kind,
        tags,
        format!("hi {created_at}"),
    )
    .sign(who)
    .unwrap()
}

fn from(relay: &str, event: &Event) -> Observation {
    Observation {
        relay: relay.to_owned(),
        event: event.clone(),
    }
}

const RELAYS: [&str; 3] = ["ws://a.example", "ws://b.example", "ws://c.example"];

fn plan(channels: &[&str]) -> ListenPlan {
    let urls: Vec<String> = RELAYS.iter().map(|r| r.to_string()).collect();
    let dir = directory_of(&urls);
    let channels: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
    ListenPlan::build(Some(&dir), None, &channels, PlanConfig::default()).unwrap()
}

fn watching(channels: &[&str]) -> Aggregator {
    let mut agg = Aggregator::new(AggregatorConfig::default());
    agg.watch(channels, T0);
    agg
}

#[test]
fn same_event_from_three_relays_counts_once() {
    let alice = Identity::generate();
    let mut agg = watching(&["u09tv"]);
    let e = event(&alice, "u09tv", KIND_GEO_MESSAGE, T0);
    assert_eq!(agg.observe(&from(RELAYS[0], &e), T0), Ok(()));
    assert_eq!(
        agg.observe(&from(RELAYS[1], &e), T0),
        Err(Rejection::Duplicate)
    );
    assert_eq!(
        agg.observe(&from(RELAYS[2], &e), T0),
        Err(Rejection::Duplicate)
    );
    let p = plan(&["u09tv"]);
    let verdicts = BTreeMap::from([(
        RELAYS[0].to_owned(),
        Verdict {
            status: Status::Ok,
            text: String::new(),
        },
    )]);
    let stats = agg.snapshot(T0 + 1, &p, &verdicts);
    assert_eq!(stats.len(), 1);
    let s = &stats[0];
    assert_eq!(
        (s.messages, s.presences, s.participants, s.last_seen),
        (1, 0, Some(1), Some(T0))
    );
    assert!(s.relays.iter().all(|r| r.delivered), "{:?}", s.relays);
    assert_eq!(s.relays[0].status, Some(Status::Ok));
    assert_eq!(s.relays[1].status, None);
    assert_eq!(agg.rejected()[&Rejection::Duplicate], 2);
}

#[test]
fn invalid_events_are_rejected_and_counted() {
    let alice = Identity::generate();
    let mut agg = watching(&["u09tv"]);
    let mut forged = event(&alice, "u09tv", KIND_GEO_MESSAGE, T0);
    forged.content = "changed".into();
    assert_eq!(
        agg.observe(&from(RELAYS[0], &forged), T0),
        Err(Rejection::BadSignature)
    );
    let mut unsigned = event(&alice, "u09tv", KIND_GEO_MESSAGE, T0 + 1);
    unsigned.sig = None;
    assert_eq!(
        agg.observe(&from(RELAYS[0], &unsigned), T0),
        Err(Rejection::BadSignature)
    );
    let note = Event::new_unsigned(
        &alice.public_key_hex(),
        T0,
        1,
        vec![vec!["g".into(), "u09tv".into()]],
        "x".into(),
    )
    .sign(&alice)
    .unwrap();
    assert_eq!(
        agg.observe(&from(RELAYS[0], &note), T0),
        Err(Rejection::WrongKind)
    );
    let untagged = Event::new_unsigned(
        &alice.public_key_hex(),
        T0,
        KIND_GEO_MESSAGE,
        vec![],
        "x".into(),
    )
    .sign(&alice)
    .unwrap();
    assert_eq!(
        agg.observe(&from(RELAYS[0], &untagged), T0),
        Err(Rejection::NoChannel)
    );
    let elsewhere = event(&alice, "u09tu", KIND_GEO_MESSAGE, T0);
    assert_eq!(
        agg.observe(&from(RELAYS[0], &elsewhere), T0),
        Err(Rejection::NotWatched)
    );

    let stats = agg.snapshot(T0, &plan(&["u09tv"]), &BTreeMap::new());
    assert_eq!((stats[0].messages, stats[0].presences), (0, 0));
    assert_eq!(agg.rejected().values().sum::<u64>(), 5);
}

#[test]
fn forged_copy_does_not_block_the_real_event() {
    let alice = Identity::generate();
    let mut agg = watching(&["u09tv"]);
    let real = event(&alice, "u09tv", KIND_GEO_MESSAGE, T0);
    let mut forged = real.clone();
    forged.sig = Some("00".repeat(64));
    assert_eq!(
        agg.observe(&from(RELAYS[0], &forged), T0),
        Err(Rejection::BadSignature)
    );
    assert_eq!(agg.observe(&from(RELAYS[1], &real), T0), Ok(()));
}

#[test]
fn window_slides_on_created_at() {
    let alice = Identity::generate();
    let mut agg = watching(&["u09tv"]);
    let window = AggregatorConfig::default().window_secs;
    agg.observe(
        &from(RELAYS[0], &event(&alice, "u09tv", KIND_GEO_PRESENCE, T0)),
        T0,
    )
    .unwrap();
    let p = plan(&["u09tv"]);
    assert_eq!(
        agg.snapshot(T0 + window - 1, &p, &BTreeMap::new())[0].presences,
        1
    );
    let later = agg.snapshot(T0 + window, &p, &BTreeMap::new());
    assert_eq!((later[0].presences, later[0].participants), (0, Some(0)));
    // The last activity stays known after the window.
    assert_eq!(later[0].last_seen, Some(T0));
    // Too old to count.
    let old = event(&alice, "u09tv", KIND_GEO_MESSAGE, T0);
    assert_eq!(
        agg.observe(&from(RELAYS[0], &old), T0 + window),
        Err(Rejection::OutOfWindow)
    );
}

#[test]
fn small_counts_are_shown_by_default() {
    let alice = Identity::generate();
    let mut agg = watching(&["u09tvq"]);
    agg.observe(
        &from(RELAYS[0], &event(&alice, "u09tvq", KIND_GEO_MESSAGE, T0)),
        T0,
    )
    .unwrap();
    let stats = agg.snapshot(T0 + 10, &plan(&["u09tvq"]), &BTreeMap::new());
    assert_eq!(stats[0].participants, Some(1), "no k threshold (D20)");
}

#[test]
fn small_counts_are_hidden_at_level_6_only_with_a_threshold() {
    let people: Vec<Identity> = (0..3).map(|_| Identity::generate()).collect();
    let mut agg = Aggregator::new(AggregatorConfig {
        k_threshold: 3,
        ..AggregatorConfig::default()
    });
    agg.watch(&["u09tvq", "u09tv"], T0);
    for (i, who) in people.iter().enumerate().take(2) {
        agg.observe(
            &from(
                RELAYS[0],
                &event(who, "u09tvq", KIND_GEO_MESSAGE, T0 + i as i64),
            ),
            T0,
        )
        .unwrap();
    }
    agg.observe(
        &from(RELAYS[0], &event(&people[0], "u09tv", KIND_GEO_MESSAGE, T0)),
        T0,
    )
    .unwrap();
    let p = plan(&["u09tvq", "u09tv"]);
    let stats = agg.snapshot(T0 + 10, &p, &BTreeMap::new());
    let get = |g: &str| stats.iter().find(|s| s.geohash == g).unwrap().clone();
    assert_eq!(get("u09tvq").participants, None, "2 < k at level 6");
    assert_eq!(get("u09tvq").messages, 2);
    assert_eq!(get("u09tv").participants, Some(1), "level 5: shown");
    agg.observe(
        &from(
            RELAYS[0],
            &event(&people[2], "u09tvq", KIND_GEO_MESSAGE, T0 + 5),
        ),
        T0,
    )
    .unwrap();
    let stats = agg.snapshot(T0 + 10, &p, &BTreeMap::new());
    assert_eq!(
        stats
            .iter()
            .find(|s| s.geohash == "u09tvq")
            .unwrap()
            .participants,
        Some(3)
    );
}

#[test]
fn mesh_sums_the_channels_inside() {
    let (alice, bob) = (Identity::generate(), Identity::generate());
    let mut agg = watching(&["u09", "u09t", "u09tv", "u09tu", "u0b"]);
    agg.observe(
        &from(RELAYS[0], &event(&alice, "u09tv", KIND_GEO_MESSAGE, T0)),
        T0,
    )
    .unwrap();
    agg.observe(
        &from(RELAYS[0], &event(&alice, "u09tu", KIND_GEO_PRESENCE, T0)),
        T0,
    )
    .unwrap();
    agg.observe(
        &from(RELAYS[0], &event(&bob, "u09tu", KIND_GEO_MESSAGE, T0)),
        T0,
    )
    .unwrap();
    // The mesh's own channel is not part of its sum.
    agg.observe(
        &from(RELAYS[0], &event(&bob, "u09", KIND_GEO_MESSAGE, T0 + 1)),
        T0,
    )
    .unwrap();
    let meshes = agg.mesh(3, T0 + 10);
    assert_eq!(meshes.len(), 1);
    let m = &meshes[0];
    assert_eq!(
        (
            m.geohash.as_str(),
            m.active_channels,
            m.messages,
            m.presences,
            m.participants
        ),
        ("u09", 2, 2, 1, Some(2))
    );
}

#[test]
fn silent_channel_becomes_inactive() {
    let alice = Identity::generate();
    let delay = AggregatorConfig::default().inactivity_secs;
    let mut agg = watching(&["u09tv", "u09tu"]);
    assert!(agg.inactive(T0 + delay - 1).is_empty());
    agg.observe(
        &from(
            RELAYS[0],
            &event(&alice, "u09tu", KIND_GEO_PRESENCE, T0 + 100),
        ),
        T0 + 100,
    )
    .unwrap();
    assert_eq!(agg.inactive(T0 + delay), ["u09tv"]);
    assert!(
        !agg.inactive(T0 + 100 + delay - 1)
            .contains(&"u09tu".to_owned())
    );
    assert_eq!(agg.inactive(T0 + 100 + delay), ["u09tu", "u09tv"]);
    // Retired channels are no longer reported.
    agg.unwatch(&["u09tv"]);
    assert_eq!(agg.inactive(T0 + 10_000), ["u09tu"]);
    assert_eq!(
        agg.observe(
            &from(
                RELAYS[0],
                &event(&alice, "u09tv", KIND_GEO_MESSAGE, T0 + 10_000)
            ),
            T0 + 10_000
        ),
        Err(Rejection::NotWatched)
    );
}

#[test]
fn reinstated_channel_gets_a_fresh_delay() {
    let delay = AggregatorConfig::default().inactivity_secs;
    let mut agg = watching(&["u09tv"]);
    assert_eq!(agg.inactive(T0 + delay), ["u09tv"]);
    agg.unwatch(&["u09tv"]);
    agg.watch(&["u09tv"], T0 + 1_000);
    assert!(agg.inactive(T0 + 1_000 + delay - 1).is_empty());
    assert_eq!(agg.inactive(T0 + 1_000 + delay), ["u09tv"]);
}

#[test]
fn future_timestamps_do_not_keep_a_channel_alive() {
    let alice = Identity::generate();
    let delay = AggregatorConfig::default().inactivity_secs;
    let mut agg = watching(&["u09tv"]);
    agg.observe(
        &from(
            RELAYS[0],
            &event(&alice, "u09tv", KIND_GEO_MESSAGE, T0 + 86_400),
        ),
        T0,
    )
    .unwrap();
    assert_eq!(agg.inactive(T0 + delay), ["u09tv"]);
}

#[test]
fn complete_minutes_go_to_the_store_once() {
    let (alice, bob) = (Identity::generate(), Identity::generate());
    let mut agg = watching(&["u09tv"]);
    agg.observe(
        &from(RELAYS[0], &event(&alice, "u09tv", KIND_GEO_MESSAGE, T0 + 5)),
        T0 + 5,
    )
    .unwrap();
    agg.observe(
        &from(
            RELAYS[0],
            &event(&alice, "u09tv", KIND_GEO_PRESENCE, T0 + 50),
        ),
        T0 + 50,
    )
    .unwrap();
    assert!(agg.drain_buckets(T0 + 59).is_empty(), "minute not complete");
    let b = agg.drain_buckets(T0 + 60);
    assert_eq!(b.len(), 1);
    assert_eq!(
        (
            b[0].bucket_start,
            b[0].messages,
            b[0].presences,
            b[0].participants
        ),
        (T0, 1, 1, 1)
    );
    assert!(agg.drain_buckets(T0 + 120).is_empty(), "handed once");
    // A late event in that minute hands it again, recounted.
    agg.observe(
        &from(RELAYS[0], &event(&bob, "u09tv", KIND_GEO_MESSAGE, T0 + 30)),
        T0 + 130,
    )
    .unwrap();
    let again = agg.drain_buckets(T0 + 130);
    assert_eq!((again[0].messages, again[0].participants), (2, 2));
}

/// Spec §5: nothing that leaves the aggregator carries a pubkey or content.
#[test]
fn outputs_carry_no_personal_data() {
    let alice = Identity::generate();
    let mut agg = watching(&["u09tvq", "u09tv"]);
    for (i, channel) in ["u09tvq", "u09tv"].iter().enumerate() {
        agg.observe(
            &from(
                RELAYS[0],
                &event(&alice, channel, KIND_GEO_MESSAGE, T0 + i as i64),
            ),
            T0,
        )
        .unwrap();
    }
    let p = plan(&["u09tvq", "u09tv"]);
    let out = format!(
        "{:?} {:?} {:?}",
        agg.snapshot(T0 + 10, &p, &BTreeMap::new()),
        agg.mesh(3, T0 + 10),
        agg.drain_buckets(T0 + 120)
    );
    assert!(!out.contains(&alice.public_key_hex()), "{out}");
    assert!(!out.contains("hi "), "{out}");
}
