//! Static files of the page, built into the binary (D10): the map works
//! offline and makes no third-party request except for the tiles.

/// One built-in file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// Path under `/assets/` (`index.html` is also served at `/`).
    pub path: &'static str,
    pub mime: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! asset {
    ($path:literal, $mime:expr) => {
        Asset {
            path: $path,
            mime: $mime,
            bytes: include_bytes!(concat!("assets/", $path)),
        }
    };
}

const HTML: &str = "text/html; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const PNG: &str = "image/png";

/// Leaflet 1.9.4, unmodified from the npm package (BSD-2-Clause, see
/// `assets/leaflet/LICENSE`); SHA-256 recorded in `PLAN_ET2.md`.
const ALL: &[Asset] = &[
    asset!("index.html", HTML),
    asset!("app.js", JS),
    asset!("view.js", JS),
    asset!("app.css", CSS),
    asset!("leaflet/leaflet.js", JS),
    asset!("leaflet/leaflet.css", CSS),
    asset!("leaflet/LICENSE", TEXT),
    asset!("leaflet/images/layers.png", PNG),
    asset!("leaflet/images/layers-2x.png", PNG),
    asset!("leaflet/images/marker-icon.png", PNG),
    asset!("leaflet/images/marker-icon-2x.png", PNG),
    asset!("leaflet/images/marker-shadow.png", PNG),
];

impl Asset {
    pub fn get(path: &str) -> Option<Asset> {
        ALL.iter().find(|a| a.path == path).copied()
    }

    pub fn all() -> &'static [Asset] {
        ALL
    }
}
