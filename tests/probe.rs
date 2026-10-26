//! Relay probe against local servers.

mod common;

use std::time::Duration;

use bitchat_geohash::plan::{ListenPlan, PlanConfig};
use bitchat_geohash::probe::{Status, probe_relays, probe_with_timeout};
use bitchat_geohash::store::{Store, Write};
use common::*;

const SHORT: Duration = Duration::from_secs(2);

async fn status_of(url: &str) -> Status {
    probe_with_timeout(url, "u09tv", SHORT).await.status
}

#[tokio::test]
async fn mock_relay_is_usable() {
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let v = probe_with_timeout(&relay.url(), "u09tv", SHORT).await;
    assert_eq!(v.status, Status::Ok, "{}", v.text);
    assert!(v.text.contains("0 recent event(s)"), "{}", v.text);
    relay.shutdown().await;
}

#[tokio::test]
async fn closed_port_is_dead() {
    assert_eq!(status_of(&closed_port().await).await, Status::Dead);
}

#[tokio::test]
async fn unknown_host_is_dead() {
    // `.invalid` never resolves (RFC 2606).
    assert_eq!(
        status_of("ws://relay.bitchat-geohash.invalid").await,
        Status::Dead
    );
}

#[tokio::test]
async fn http_4xx_is_dead_5xx_is_down() {
    assert_eq!(
        status_of(&http_server("404 Not Found").await).await,
        Status::Dead
    );
    assert_eq!(
        status_of(&http_server("503 Service Unavailable").await).await,
        Status::Down
    );
}

#[tokio::test]
async fn no_answer_is_down() {
    let url = silent_server().await;
    let v = probe_with_timeout(&url, "u09tv", Duration::from_millis(300)).await;
    assert_eq!(v.status, Status::Down, "{}", v.text);
}

#[tokio::test]
async fn auth_required_is_unusable_by_the_app() {
    let url = scripted_relay(&[
        r#"["AUTH","challenge"]"#,
        r#"["CLOSED","{sub}","auth-required: we only serve authenticated users"]"#,
    ])
    .await;
    let v = probe_with_timeout(&url, "u09tv", SHORT).await;
    assert_eq!(v.status, Status::Auth, "{}", v.text);
    assert!(!v.status.worth_listening());
}

#[tokio::test]
async fn other_closed_is_refused_but_kept() {
    let url = scripted_relay(&[r#"["CLOSED","{sub}","blocked: kind not allowed"]"#]).await;
    let v = probe_with_timeout(&url, "u09tv", SHORT).await;
    assert_eq!(v.status, Status::Refused, "{}", v.text);
    assert!(v.status.worth_listening());
}

#[tokio::test]
async fn auth_challenge_alone_is_still_usable() {
    let url = scripted_relay(&[r#"["AUTH","challenge"]"#, r#"["EOSE","{sub}"]"#]).await;
    let v = probe_with_timeout(&url, "u09tv", SHORT).await;
    assert_eq!(v.status, Status::Ok, "{}", v.text);
    assert!(v.text.contains("AUTH challenges"), "{}", v.text);
}

#[tokio::test]
async fn one_probe_per_relay_of_the_plan() {
    let alive = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let dead = closed_port().await;
    let dir = directory_of(&[alive.url(), dead.clone()]);
    let channels = vec!["u09tv".to_owned(), "u09tu".to_owned()];
    let mut plan = ListenPlan::build(Some(&dir), None, &channels, PlanConfig::default()).unwrap();
    let verdicts = probe_relays(&plan, 4, SHORT).await;
    assert_eq!(verdicts.len(), 2);
    assert_eq!(verdicts[&alive.url()].status, Status::Ok);
    assert_eq!(verdicts[&dead].status, Status::Dead);

    // Relays not worth listening to leave the plan; verdicts go to the store.
    let drop: Vec<&str> = verdicts
        .iter()
        .filter(|(_, v)| !v.status.worth_listening())
        .map(|(r, _)| r.as_str())
        .collect();
    plan.drop_relays(&drop);
    assert_eq!(plan.relays(), [alive.url()]);
    let mut store = Store::open_in_memory().unwrap();
    let writes: Vec<Write> = verdicts.iter().map(|(r, v)| v.to_write(r, 1_000)).collect();
    store.apply(&writes).unwrap();
    assert_eq!(store.last_probe(&dead).unwrap().unwrap().1, "dead");
    alive.shutdown().await;
}
