//! Web server: page and built-in files.
#![cfg(feature = "server")]

use bitchat_geohash::server::{Asset, router};
use sha2::{Digest, Sha256};

/// Leaflet 1.9.4 from the npm package, unmodified (D10).
const LEAFLET_JS_SHA256: &str = "db49d009c841f5ca34a888c96511ae936fd9f5533e90d8b2c4d57596f4e5641a";
const LEAFLET_CSS_SHA256: &str = "a7837102824184820dfa198d1ebcd109ff6d0ff9a2672a074b9a1b4d147d04c6";

async fn serve() -> String {
    bitchat_geohash::install_crypto_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router()).await });
    format!("http://{addr}")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn page_and_assets_are_served_with_their_type() {
    let base = serve().await;
    let page = reqwest::get(format!("{base}/")).await.unwrap();
    assert_eq!(page.status(), 200);
    assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(page.headers()["x-content-type-options"], "nosniff");
    let html = page.text().await.unwrap();

    // Everything the page references is built in, and nothing is external.
    let refs: Vec<&str> = html
        .split('"')
        .filter(|s| s.starts_with("/assets/"))
        .collect();
    assert_eq!(refs.len(), 4, "{refs:?}");
    for r in refs {
        let path = r.strip_prefix("/assets/").unwrap();
        let a = Asset::get(path).unwrap_or_else(|| panic!("{path} not built in"));
        let resp = reqwest::get(format!("{base}{r}")).await.unwrap();
        assert_eq!(resp.status(), 200, "{r}");
        assert_eq!(resp.headers()["content-type"], a.mime, "{r}");
        assert_eq!(resp.bytes().await.unwrap().as_ref(), a.bytes, "{r}");
    }
    assert!(
        !html.contains("https://"),
        "no external resource in the page"
    );
}

#[tokio::test]
async fn unknown_paths_are_not_found() {
    let base = serve().await;
    for path in ["/assets/nope.js", "/assets/../Cargo.toml", "/nope"] {
        let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(resp.status(), 404, "{path}");
    }
}

#[test]
fn leaflet_is_the_unmodified_release() {
    let js = Asset::get("leaflet/leaflet.js").unwrap();
    let css = Asset::get("leaflet/leaflet.css").unwrap();
    assert_eq!(sha256(js.bytes), LEAFLET_JS_SHA256);
    assert_eq!(sha256(css.bytes), LEAFLET_CSS_SHA256);
    let license = Asset::get("leaflet/LICENSE").unwrap();
    assert!(license.bytes.starts_with(b"BSD 2-Clause License"));
}

#[test]
fn every_asset_has_content_and_a_type() {
    for a in Asset::all() {
        assert!(!a.bytes.is_empty(), "{}", a.path);
        assert!(!a.mime.is_empty(), "{}", a.path);
    }
}
