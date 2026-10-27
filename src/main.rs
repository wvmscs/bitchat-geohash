//! `bitchat-geohash`: live map of BitChat location-channel activity.
//!
//! Lot 10 skeleton: serves the map page only. The monitor, the live state
//! and the full command line come with lots 13 to 17 (see `PLAN_ET2.md`).
//!
//!   cargo run -- [--listen 127.0.0.1:8080]

use std::net::SocketAddr;

fn usage() -> ! {
    eprintln!("usage: bitchat-geohash [--listen 127.0.0.1:8080]");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let mut listen: SocketAddr = "127.0.0.1:8080".parse().expect("valid default");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                listen = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            _ => usage(),
        }
    }

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .unwrap_or_else(|e| {
            eprintln!("cannot listen on {listen}: {e}");
            std::process::exit(1)
        });
    tracing::info!("map on http://{}/", listener.local_addr().unwrap_or(listen));
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(e) = axum::serve(listener, bitchat_geohash::server::router())
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("server error: {e}");
        std::process::exit(1)
    }
}
