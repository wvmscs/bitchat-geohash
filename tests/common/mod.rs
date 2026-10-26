//! Local relays and servers for the network tests. Each test binary uses a
//! different subset.
#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use bitchat_geohash::directory::RelayDirectory;

/// A relay list with these URLs, all in the centre of Paris (so they are
/// the closest relays to every `u09` channel).
pub fn directory_of(urls: &[String]) -> RelayDirectory {
    let mut csv = String::from("Relay URL,Latitude,Longitude\n");
    for url in urls {
        csv.push_str(&format!("{url},48.8566,2.3522\n"));
    }
    RelayDirectory::parse(&csv)
}

async fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    (l, format!("ws://{addr}"))
}

/// A WebSocket server that answers each REQ with `replies`, where `{sub}`
/// stands for the subscription id.
pub async fn scripted_relay(replies: &[&str]) -> String {
    let (l, url) = listener().await;
    let replies: Vec<String> = replies.iter().map(|r| r.to_string()).collect();
    tokio::spawn(async move {
        while let Ok((stream, _)) = l.accept().await {
            let replies = replies.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(Value::Array(v)) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
                    };
                    if v.first().and_then(Value::as_str) != Some("REQ") {
                        continue;
                    }
                    let sub = v.get(1).and_then(Value::as_str).unwrap_or("");
                    for r in &replies {
                        let _ = ws.send(Message::text(r.replace("{sub}", sub))).await;
                    }
                }
            });
        }
    });
    url
}

/// Accepts TCP connections and never answers: the handshake hangs.
pub async fn silent_server() -> String {
    let (l, url) = listener().await;
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = l.accept().await {
            held.push(stream);
        }
    });
    url
}

/// Answers every HTTP request with this status line, e.g. `404 Not Found`.
pub async fn http_server(status: &'static str) -> String {
    let (l, url) = listener().await;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let reply =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    url
}

/// A local port nobody listens on.
pub async fn closed_port() -> String {
    let (l, url) = listener().await;
    drop(l);
    url
}
