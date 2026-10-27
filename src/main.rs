//! `bitchat-geohash`: live map of BitChat location-channel activity.
//!
//! Lot 13: watches the channels inside a geohash and serves the live map.
//! The full command line comes with lot 17 (see `PLAN_ET2.md`).
//!
//!   cargo run -- <geohash> [--listen 127.0.0.1:8080] [--allow-remote] \
//!       [--db bitchat-geohash.sqlite]

use std::net::SocketAddr;

use bitchat_geohash::monitor::{ListSources, Monitor, MonitorConfig};
use bitchat_geohash::server::{ServerConfig, serve};
use bitchat_geohash::store::Store;

fn usage() -> ! {
    eprintln!(
        "usage: bitchat-geohash <geohash> [--listen 127.0.0.1:8080] [--allow-remote] [--db bitchat-geohash.sqlite]"
    );
    std::process::exit(2)
}

fn fail(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("{what}: {e}");
    std::process::exit(1)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    bitchat_geohash::install_crypto_provider();

    let mut start = None;
    let mut db = "bitchat-geohash.sqlite".to_owned();
    let mut server = ServerConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                server.listen = args
                    .next()
                    .and_then(|v| v.parse::<SocketAddr>().ok())
                    .unwrap_or_else(|| usage())
            }
            "--allow-remote" => server.allow_remote = true,
            "--db" => db = args.next().unwrap_or_else(|| usage()),
            s if !s.starts_with('-') && start.is_none() => start = Some(s.to_owned()),
            _ => usage(),
        }
    }
    let config = MonitorConfig::new(&start.unwrap_or_else(|| usage()));

    let store = Store::open(&db).unwrap_or_else(|e| fail(&format!("cannot open {db}"), e));
    let monitor = Monitor::start_from_sources(config.clone(), ListSources::official(), store)
        .await
        .unwrap_or_else(|e| fail("cannot start", e));
    let server = serve(monitor, &config, server)
        .await
        .unwrap_or_else(|e| fail("cannot serve", e));

    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("stopping");
    match server.shutdown().await {
        Ok(monitor) => {
            if let Err(e) = monitor.shutdown().await {
                fail("shutdown", e)
            }
        }
        Err(e) => fail("shutdown", e),
    }
}
