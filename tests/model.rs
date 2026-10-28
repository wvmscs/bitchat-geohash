// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the browser receives: views, SSE messages, deltas, and privacy.
#![cfg(feature = "server")]

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use bitchat_geohash::aggregate::{CellRelay, CellStats, MeshStats, Rejection};
use bitchat_geohash::listener::ConnState;
use bitchat_geohash::monitor::{ConnectionStatus, Monitor, MonitorStatus, RelayInfo};
use bitchat_geohash::nostr::Event;
use bitchat_geohash::probe::Status;
use bitchat_geohash::server::model::*;
use bitchat_geohash::store::Store;
use bitchat_geohash::{KIND_GEO_MESSAGE, KIND_GEO_PRESENCE};
use common::*;
use serde_json::json;

fn cell(geohash: &str, messages: u32, participants: Option<u32>) -> CellStats {
    CellStats {
        geohash: geohash.into(),
        level: geohash.len() as u8,
        watched: true,
        preview: false,
        messages,
        presences: 2,
        participants,
        last_seen: Some(1_800_000_000),
        relays: vec![
            CellRelay {
                url: "wss://a.example".into(),
                ios: Some(1),
                android: vec![2, 3],
                status: Some(Status::Ok),
                delivered: true,
            },
            CellRelay {
                url: "wss://b.example".into(),
                ios: None,
                android: vec![1],
                status: None,
                delivered: false,
            },
        ],
        divergence: vec!["wss://b.example".into()],
    }
}

#[test]
fn cell_reference_json() {
    let view = CellView::from(&cell("u09tv", 4, Some(3)));
    assert_eq!(
        serde_json::to_value(&view).unwrap(),
        json!({
            "geohash": "u09tv",
            "level": 5,
            "bbox": [[48.8232421875, 2.3291015625], [48.8671875, 2.373046875]],
            "watched": true,
            "preview": false,
            "messages": 4,
            "presences": 2,
            "participants": 3,
            "last_seen": 1_800_000_000,
            "relays": [
                {"url": "wss://a.example", "ios": 1, "android": [2, 3], "status": "ok", "delivered": true},
                {"url": "wss://b.example", "ios": null, "android": [1], "status": null, "delivered": false}
            ],
            "divergence": ["wss://b.example"]
        })
    );
}

#[test]
fn hidden_participants_are_low() {
    // Only with a k threshold set: none by default (D20).
    let view = serde_json::to_value(CellView::from(&cell("u09tvw", 5, None))).unwrap();
    assert_eq!(view["participants"], "low");
    assert_eq!(
        (&view["messages"], &view["presences"]),
        (&json!(5), &json!(2))
    );
    let mesh = MeshView::from(&MeshStats {
        geohash: "u09tvw".into(),
        active_channels: 1,
        messages: 5,
        presences: 0,
        participants: None,
    });
    let mesh = serde_json::to_value(mesh).unwrap();
    assert_eq!(mesh["participants"], "low");
    assert_eq!(mesh["messages"], 5);
    assert_eq!(mesh["active_channels"], 1);
    assert_eq!(mesh["level"], 6);
}

#[test]
fn relay_status_and_connections() {
    let info = RelayInfo {
        url: "wss://a.example".into(),
        lat: 48.8566,
        lon: 2.3522,
        ios: true,
        android: false,
        status: Some(Status::Auth),
        listened: false,
        channels: 12,
        connections: vec![
            ConnState::Waiting {
                retry_in: Duration::from_millis(2500),
            },
            ConnState::Abandoned {
                auth: true,
                reason: "auth-required: sign in".into(),
            },
        ],
    };
    assert_eq!(
        serde_json::to_value(RelayView::from(&info)).unwrap(),
        json!({
            "url": "wss://a.example", "lat": 48.8566, "lon": 2.3522,
            "ios": true, "android": false, "status": "auth", "listened": false,
            "channels": 12,
            "connections": [
                {"state": "waiting", "retry_in_secs": 2},
                {"state": "abandoned", "auth": true, "reason": "auth-required: sign in"}
            ]
        })
    );
    let status = MonitorStatus {
        watched: vec!["u09tv".into(), "u09tw".into()],
        retired: vec!["u09ty".into()],
        relays: vec!["wss://a.example".into()],
        dropped_relays: vec!["wss://dead.example".into()],
        connections: vec![
            ConnectionStatus {
                relay: "wss://a.example".into(),
                connection: 0,
                state: ConnState::Connected,
            },
            ConnectionStatus {
                relay: "wss://a.example".into(),
                connection: 1,
                state: ConnState::Connecting,
            },
        ],
        rejected: BTreeMap::from([(Rejection::Duplicate, 7), (Rejection::OutOfWindow, 1)]),
    };
    assert_eq!(
        serde_json::to_value(StatusView::from(&status)).unwrap(),
        json!({
            "watched": 2, "retired": 1, "relays": 1,
            "dropped_relays": ["wss://dead.example"],
            "connections_up": 1, "connections": 2,
            "rejected": {"duplicate": 7, "out_of_window": 1}
        })
    );
}

fn params() -> Params {
    Params::new(
        &bitchat_geohash::monitor::MonitorConfig::new("u09t"),
        DEFAULT_TILES,
        DEFAULT_ATTRIBUTION,
    )
}

#[test]
fn hello_then_only_what_changed() {
    let mut p = Published::new(params());
    let a = CellView::from(&cell("u09tv", 1, Some(3)));
    let b = CellView::from(&cell("u09tw", 0, Some(0)));
    let status = StatusView::default();

    let first = p.update(vec![a.clone(), b.clone()], vec![], status.clone());
    assert_eq!(
        first.iter().map(Push::event).collect::<Vec<_>>(),
        ["cells", "status"]
    );
    assert_eq!(first.iter().map(Push::seq).collect::<Vec<_>>(), [1, 2]);

    // Same state: nothing to send.
    assert!(
        p.update(vec![a.clone(), b.clone()], vec![], status.clone())
            .is_empty()
    );

    // One cell changes: only that one, next seq.
    let a2 = CellView::from(&cell("u09tv", 2, Some(3)));
    let delta = p.update(vec![a2.clone(), b.clone()], vec![], status.clone());
    assert_eq!(
        delta,
        [Push::Cells {
            seq: 3,
            cells: vec![a2.clone()]
        }]
    );

    // A cell disappears.
    let delta = p.update(vec![a2.clone()], vec![], status);
    assert_eq!(
        delta,
        [Push::Removed {
            seq: 4,
            cells: vec!["u09tw".into()]
        }]
    );

    // A new stream starts from the full state and the last seq.
    let hello = serde_json::to_value(p.hello()).unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["seq"], 4);
    assert_eq!(hello["cells"].as_array().unwrap().len(), 1);
    assert_eq!(hello["params"]["start"], "u09t");
    assert_eq!(hello["params"]["levels"], json!([2, 4, 5, 6]));
    assert_eq!(hello["params"]["k"], 0);
    assert_eq!(hello["params"]["window_secs"], 900);
    assert_eq!(hello["params"]["tiles"], DEFAULT_TILES);
    let push = Push::Status {
        seq: 9,
        status: StatusView::default(),
    };
    assert_eq!(push.event(), "status");
    assert!(push.to_json().starts_with(r#"{"type":"status","seq":9,"#));
}

/// Longest run of hexadecimal characters: 64 is a pubkey or an event id,
/// 128 a signature.
fn longest_hex_run(s: &str) -> usize {
    let (mut best, mut run) = (0, 0);
    for c in s.chars() {
        run = if c.is_ascii_hexdigit() { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

fn event(who: &Identity, kind: u32, channel: &str, at: i64, marker: &str) -> Event {
    let tags = vec![
        vec!["g".to_owned(), channel.to_owned()],
        vec!["n".to_owned(), format!("nick-{marker}")],
    ];
    Event::new_unsigned(
        &who.public_key_hex(),
        at,
        kind,
        tags,
        format!("content-{marker}"),
    )
    .sign(who)
    .unwrap()
}

#[tokio::test]
async fn nothing_personal_reaches_the_browser() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let clock = TestClock::new();
    let cfg = config("u09t", &clock);
    let mut published = Published::new(Params::new(&cfg, DEFAULT_TILES, DEFAULT_ATTRIBUTION));
    let monitor = Monitor::start(
        cfg,
        Some(directory_of(&[relay.url()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    monitor.descend("u09tv").await.unwrap();
    status_until(&monitor, "connected", connected).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Three people in u09tv (level 5), one alone in u09tvw (level 6).
    let people: Vec<Identity> = (0..4).map(|_| Identity::generate()).collect();
    let mut sent = Vec::new();
    for (i, who) in people.iter().enumerate() {
        let channel = if i < 3 { "u09tv" } else { "u09tvw" };
        for kind in [KIND_GEO_MESSAGE, KIND_GEO_PRESENCE] {
            let e = event(who, kind, channel, clock.get(), "SECRET-MARKER");
            sent.push(e.clone());
            relay.publish(e);
        }
    }
    let mut snap = Vec::new();
    for _ in 0..150 {
        snap = monitor.snapshot().await.unwrap();
        let counted: u32 = snap.iter().map(|c| c.messages + c.presences).sum();
        if counted == 8 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let cells: Vec<CellView> = snap.iter().map(CellView::from).collect();
    let relays: Vec<RelayView> = monitor
        .relays()
        .await
        .unwrap()
        .iter()
        .map(RelayView::from)
        .collect();
    let status = StatusView::from(&monitor.status().await.unwrap());
    let mut deltas = published.update(cells, relays, status);
    let mut meshes = Vec::new();
    for precision in MESH_LEVELS {
        meshes.extend(
            monitor
                .mesh(precision)
                .await
                .unwrap()
                .iter()
                .map(MeshView::from),
        );
    }
    deltas.extend(published.update_meshes(meshes));
    let hello = published.hello();
    // The u09tv mesh sums its level 6 channel: one person.
    let mesh = serde_json::to_value(&hello).unwrap()["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["geohash"] == "u09tv")
        .cloned()
        .unwrap();
    assert_eq!(
        (&mesh["active_channels"], &mesh["participants"]),
        (&json!(1), &json!(1))
    );

    let tv = serde_json::to_value(&hello).unwrap()["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["geohash"] == "u09tv")
        .cloned()
        .unwrap();
    assert_eq!(
        (tv["messages"].clone(), tv["participants"].clone()),
        (json!(3), json!(3))
    );
    let tvw = serde_json::to_value(&hello).unwrap()["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["geohash"] == "u09tvw")
        .cloned()
        .unwrap();
    // No k threshold by default: a lone participant is counted (D20).
    assert_eq!(
        (tvw["messages"].clone(), tvw["participants"].clone()),
        (json!(1), json!(1))
    );

    let mut text = hello.to_json();
    for d in &deltas {
        text.push_str(&d.to_json());
    }
    assert!(text.contains("u09tvw"), "the stream is not empty");
    assert!(
        !text.contains("SECRET-MARKER"),
        "content or nickname leaked"
    );
    assert!(!text.contains("nick-"));
    for e in &sent {
        assert!(!text.contains(&e.pubkey), "pubkey leaked");
        assert!(!text.contains(&e.id), "event id leaked");
        assert!(
            !text.contains(e.sig.as_deref().unwrap()),
            "signature leaked"
        );
    }
    assert!(!text.contains("npub"));
    assert!(
        longest_hex_run(&text) < 64,
        "a 64-hex value is in the stream"
    );

    drop(monitor.shutdown().await.unwrap());
    relay.shutdown().await;
}

#[test]
fn meshes_are_sent_whole_when_they_change() {
    let mut p = Published::new(params());
    let mesh = |messages| {
        MeshView::from(&MeshStats {
            geohash: "u09".into(),
            active_channels: 2,
            messages,
            presences: 1,
            participants: Some(2),
        })
    };
    assert_eq!(p.update_meshes(vec![]), None);
    let first = p.update_meshes(vec![mesh(3)]).unwrap();
    assert_eq!((first.event(), first.seq()), ("meshes", 1));
    assert_eq!(p.update_meshes(vec![mesh(3)]), None);
    assert_eq!(
        p.update_meshes(vec![mesh(4)]),
        Some(Push::Meshes {
            seq: 2,
            meshes: vec![mesh(4)]
        })
    );
    let hello = serde_json::to_value(p.hello()).unwrap();
    assert_eq!(hello["seq"], 2);
    assert_eq!(hello["meshes"][0]["level"], 3);
    assert_eq!(
        hello["meshes"][0]["bbox"],
        json!([[47.8125, 1.40625], [49.21875, 2.8125]])
    );
    assert_eq!(hello["params"]["mesh_levels"], json!([2, 3, 4, 5, 6]));
}
