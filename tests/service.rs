// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The map's service without a web server (the browser build's): the same
//! answers and event stream as the server, around a monitor on a local
//! relay.
#![cfg(feature = "server")]

mod common;

use std::time::Duration;

use bitchat_geohash::KIND_GEO_MESSAGE;
use bitchat_geohash::monitor::Monitor;
use bitchat_geohash::nostr::Event;
use bitchat_geohash::service::{Events, Relaunch, Service, ServiceConfig};
use bitchat_geohash::store::Store;
use common::*;
use serde_json::{Value, json};

struct Running {
    relay: TestRelay,
    clock: TestClock,
    service: Service,
}

impl Running {
    async fn start(relaunch: bool, strategy_change: bool) -> Self {
        bitchat_geohash::install_crypto_provider();
        let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
        let clock = TestClock::new();
        let config = config("u09t", &clock);
        let monitor = Monitor::start(
            config.clone(),
            Some(directory_of(&[relay.url()])),
            None,
            Store::open_in_memory().unwrap(),
        )
        .await
        .unwrap();
        status_until(&monitor, "connected", connected).await;
        let relaunch = relaunch.then(|| Relaunch::Fixed {
            ios: Some(directory_of(&[relay.url()])),
            android: None,
            db: std::env::temp_dir()
                .join(format!(
                    "bitchat-geohash-service-{}.sqlite",
                    std::process::id()
                ))
                .display()
                .to_string(),
        });
        let service = Service::start(
            monitor,
            &config,
            relaunch,
            ServiceConfig {
                push_every: Duration::from_millis(50),
                strategy_change,
                ..ServiceConfig::default()
            },
        )
        .await
        .unwrap();
        Running {
            relay,
            clock,
            service,
        }
    }

    async fn ask(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
        let (status, body) = self.service.handle(method, path, body).await;
        (status, serde_json::from_str(&body).unwrap())
    }

    async fn stop(self) {
        let monitor = self.service.shutdown().await.unwrap();
        drop(monitor.shutdown().await.unwrap());
        self.relay.shutdown().await;
    }
}

/// Skips messages until one of `event` matching `want`.
async fn until(events: &mut Events, event: &str, want: impl Fn(&Value) -> bool) -> (u64, Value) {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(3), events.next())
            .await
            .expect("event stream silent")
            .expect("stream ended");
        let data: Value = serde_json::from_str(&m.data).unwrap();
        if m.event == event && want(&data) {
            return (m.id, data);
        }
    }
}

fn message(channel: &str, at: i64) -> Event {
    let who = Identity::generate();
    let tags = vec![vec!["g".to_owned(), channel.to_owned()]];
    Event::new_unsigned(
        &who.public_key_hex(),
        at,
        KIND_GEO_MESSAGE,
        tags,
        "hi".into(),
    )
    .sign(&who)
    .unwrap()
}

fn cell<'a>(cells: &'a Value, geohash: &str) -> Option<&'a Value> {
    cells.as_array()?.iter().find(|c| c["geohash"] == geohash)
}

#[tokio::test]
async fn hello_then_a_delta_after_an_event() {
    let r = Running::start(false, true).await;
    let mut events = r.service.events();
    let (hello_seq, hello) = until(&mut events, "hello", |_| true).await;
    assert_eq!(hello["cells"].as_array().unwrap().len(), 33);
    assert_eq!(hello["params"]["start"], "u09t");
    r.relay.publish(message("u09tv", r.clock.get()));
    let (seq, delta) = until(&mut events, "cells", |d| {
        cell(&d["cells"], "u09tv").is_some_and(|c| c["messages"] == 1)
    })
    .await;
    assert!(seq > hello_seq);
    assert!(cell(&delta["cells"], "u09tw").is_none());
    r.stop().await;
}

#[tokio::test]
async fn commands_answer_as_the_server_does() {
    let r = Running::start(false, true).await;
    let (status, state) = r.ask("GET", "/api/state", "").await;
    assert_eq!((status, state["type"].as_str()), (200, Some("hello")));
    let mut events = r.service.events();
    let (status, body) = r
        .ask("POST", "/api/descend", r#"{"geohash":"u09tv"}"#)
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["added"].as_array().unwrap().len(), 32);
    until(&mut events, "cells", |d| {
        cell(&d["cells"], "u09tvw").is_some()
    })
    .await;
    let (_, state) = r.ask("GET", "/api/state?x=1", "").await;
    assert_eq!(state["cells"].as_array().unwrap().len(), 65);
    let (status, body) = r.ask("POST", "/api/preview", r#"{"geohash":"u0b"}"#).await;
    assert_eq!((status, &body["error"]), (422, &json!("outside_area")));
    r.stop().await;
}

#[tokio::test]
async fn refused_requests_say_why() {
    let r = Running::start(false, false).await;
    for (method, path, body, status, error) in [
        (
            "POST",
            "/api/descend",
            r#"{"geohash":"u0a"}"#,
            400,
            "invalid_geohash",
        ),
        (
            "POST",
            "/api/descend",
            r#"{"geo":"u09tv"}"#,
            422,
            "bad_request",
        ),
        ("POST", "/api/descend", "{not json", 400, "bad_request"),
        ("POST", "/api/reinstate", "{}", 400, "bad_request"),
        (
            "POST",
            "/api/area",
            r#"{"geohash":"u0b"}"#,
            409,
            "area_fixed",
        ),
        (
            "POST",
            "/api/strategy",
            r#"{"strategy":"a1"}"#,
            403,
            "strategy_fixed",
        ),
        ("GET", "/api/descend", "", 405, "method_not_allowed"),
        ("GET", "/api/nope", "", 404, "not_found"),
    ] {
        let (got, answer) = r.ask(method, path, body).await;
        assert_eq!(
            (got, &answer["error"]),
            (status, &json!(error)),
            "{path} {body}"
        );
    }
    r.stop().await;
}

#[tokio::test]
async fn reinstate_after_retirement() {
    let r = Running::start(false, true).await;
    r.clock.set(r.clock.get() + inactivity());
    for _ in 0..150 {
        if r.ask("GET", "/api/state", "").await.1["status"]["retired"] == 33 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (status, body) = r
        .ask("POST", "/api/reinstate", r#"{"channels":["u09tv"]}"#)
        .await;
    assert_eq!((status, &body["reinstated"]), (200, &json!(["u09tv"])));
    r.stop().await;
}

#[tokio::test]
async fn the_area_changes_and_the_stream_starts_over() {
    let r = Running::start(true, true).await;
    let mut events = r.service.events();
    until(&mut events, "hello", |_| true).await;
    let (status, body) = r.ask("POST", "/api/area", r#"{"geohash":"u09w"}"#).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (&body["start"], &body["place"]),
        (&json!("u09w"), &Value::Null)
    );
    let (_, hello) = until(&mut events, "hello", |d| d["params"]["start"] == "u09w").await;
    assert!(cell(&hello["cells"], "u09wb").is_some());
    let (status, body) = r.ask("POST", "/api/strategy", r#"{"strategy":"a1"}"#).await;
    assert_eq!((status, &body["strategy"]), (200, &json!("a1")));
    r.stop().await;
}
