// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional TLS (lot 13 bis): HTTPS, ALPN `h2` / `http/1.1`, names of the
//! certificate, errors. Test certificate authority and keys in
//! `tests/data/tls/` (test only).
#![cfg(feature = "server")]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bitchat_geohash::Error;
use bitchat_geohash::monitor::{Monitor, MonitorConfig};
use bitchat_geohash::server::{Server, ServerConfig, TlsFiles, serve};
use bitchat_geohash::store::Store;
use common::*;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/tls")
        .join(name)
}

fn files(key: &str) -> TlsFiles {
    TlsFiles {
        cert: data("cert.pem"),
        key: data(key),
    }
}

async fn monitor(clock: &TestClock) -> (Monitor, MonitorConfig) {
    let config = config("u09t", clock);
    let monitor = Monitor::start(
        config.clone(),
        Some(directory_of(&["ws://127.0.0.1:1".into()])),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .unwrap();
    (monitor, config)
}

async fn start(tls: Option<TlsFiles>) -> Result<Server, Error> {
    bitchat_geohash::install_crypto_provider();
    let (monitor, config) = monitor(&TestClock::new()).await;
    let server = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls,
        push_every: Duration::from_millis(50),
        ..ServerConfig::default()
    };
    serve(monitor, &config, server).await
}

/// An HTTPS client that trusts the test authority only, checked by
/// webpki: the platform verifier of macOS refuses certificates valid for
/// more than 825 days, and the test one is valid for 100 years.
fn client() -> reqwest::Client {
    let ca = reqwest::Certificate::from_pem(&std::fs::read(data("ca.pem")).unwrap()).unwrap();
    reqwest::Client::builder()
        .tls_certs_only([ca])
        .build()
        .unwrap()
}

async fn stop(server: Server) {
    let monitor = server.shutdown().await.unwrap();
    drop(monitor.shutdown().await.unwrap());
}

#[tokio::test]
async fn page_and_api_over_https() {
    let server = start(Some(files("key.pem"))).await.unwrap();
    let url = server.url();
    assert!(url.starts_with("https://127.0.0.1:"), "{url}");
    let http = client();
    let page = http.get(format!("{url}/")).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert_eq!(page.version(), reqwest::Version::HTTP_11);
    assert!(page.headers().contains_key("content-security-policy"));
    let state = http.get(format!("{url}/api/state")).send().await.unwrap();
    assert_eq!(state.status(), 200);

    // SSE over TLS: the first message is `hello`.
    let mut sse = http.get(format!("{url}/api/events")).send().await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(3), sse.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).starts_with("event: hello"));

    // Commands: the origin is the HTTPS one.
    let post = |origin: String| {
        http.post(format!("{url}/api/reinstate"))
            .header("content-type", "application/json")
            .header("origin", origin)
            .body(r#"{"all":true}"#)
            .send()
    };
    assert_eq!(post(url.clone()).await.unwrap().status(), 200);
    let clear = url.replacen("https://", "http://", 1);
    assert_eq!(post(clear).await.unwrap().status(), 403);

    // At shutdown the stream ends (after what was already sent).
    stop(server).await;
    let end = tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(Some(_)) = sse.chunk().await {}
    })
    .await;
    assert!(end.is_ok(), "stream still open");
}

#[tokio::test]
async fn host_must_be_a_name_of_the_certificate() {
    let server = start(Some(files("key.pem"))).await.unwrap();
    let port = server.local_addr().port();
    let http = client();
    let get = |host: String| {
        http.get(format!("{}/api/state", server.url()))
            .header("host", host)
            .send()
    };
    for ok in [format!("localhost:{port}"), format!("127.0.0.1:{port}")] {
        assert_eq!(get(ok.clone()).await.unwrap().status(), 200, "{ok}");
    }
    for bad in [
        format!("evil.example:{port}"),
        format!("localhost:{}", port.wrapping_add(1)),
        "localhost".to_owned(),
    ] {
        assert_eq!(get(bad.clone()).await.unwrap().status(), 421, "{bad}");
    }
    stop(server).await;
}

/// A TLS connection offering `alpn`, as a browser does; returns the
/// protocol the server picked.
async fn handshake(
    addr: std::net::SocketAddr,
    alpn: &[&[u8]],
) -> (
    String,
    tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
) {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(data("ca.pem")).unwrap())
        .unwrap();
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let tls = connector
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    let picked = String::from_utf8_lossy(tls.get_ref().1.alpn_protocol().unwrap()).into_owned();
    (picked, tls)
}

#[tokio::test]
async fn http2_is_negotiated_and_served() {
    let server = start(Some(files("key.pem"))).await.unwrap();
    let addr = server.local_addr();
    let (picked, _) = handshake(addr, &[b"http/1.1"]).await;
    assert_eq!(picked, "http/1.1");

    let (picked, mut tls) = handshake(addr, &[b"h2", b"http/1.1"]).await;
    assert_eq!(picked, "h2");
    // HTTP/2 connection preface and an empty SETTINGS frame: the server
    // answers with its own SETTINGS frame (type 4).
    tls.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    tls.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap();
    let mut header = [0u8; 9];
    tokio::time::timeout(Duration::from_secs(3), tls.read_exact(&mut header))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header[3], 4, "SETTINGS frame expected: {header:?}");
    drop(tls);
    stop(server).await;
}

#[tokio::test]
async fn bad_tls_files_are_clear_errors() {
    let missing = TlsFiles {
        cert: data("missing.pem"),
        key: data("key.pem"),
    };
    let not_a_cert = TlsFiles {
        cert: data("README.md"),
        key: data("key.pem"),
    };
    let not_a_key = TlsFiles {
        cert: data("cert.pem"),
        key: data("ca.pem"),
    };
    for (tls, want) in [
        (missing, "missing.pem"),
        (not_a_cert, "no certificate"),
        (not_a_key, "ca.pem"),
        (files("other-key.pem"), "other-key.pem"),
    ] {
        match start(Some(tls)).await {
            Err(Error::Tls(msg)) => assert!(msg.contains(want), "{msg}"),
            Err(e) => panic!("{want}: {e}"),
            Ok(_) => panic!("{want}: started"),
        }
    }
}

#[tokio::test]
async fn remote_needs_tls_or_insecure_remote() {
    let (monitor, config) = monitor(&TestClock::new()).await;
    let server = ServerConfig {
        listen: "192.0.2.1:8443".parse().unwrap(),
        allow_remote: true,
        ..ServerConfig::default()
    };
    assert!(matches!(
        serve(monitor, &config, server).await,
        Err(Error::RemoteWithoutTls(_))
    ));
}
