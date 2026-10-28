// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The apps' relay choice for a channel: closest relays, iOS and Android (lots
//! 1 and 11 bis). geohash-relays' code (D2 of bitchat-console), with this
//! crate's error type.

pub use geohash_relays::select::{GeoRelay, Selected};

use crate::directory::RelayDirectory;
use crate::error::Result;

/// Centre `(lat, lon)` of a geohash cell: where the apps measure the
/// distance to the relays from.
pub fn geohash_center(geohash: &str) -> Result<(f64, f64)> {
    Ok(geohash_relays::select::geohash_center(geohash)?)
}

/// The `count` relays of `dir` closest to the centre of `geohash`, with the
/// rule of the list's app.
pub fn closest(dir: &RelayDirectory, geohash: &str, count: usize) -> Result<Vec<GeoRelay>> {
    Ok(geohash_relays::select::closest(dir, geohash, count)?)
}

/// Union of the iOS and Android choices for a channel, one entry per server,
/// nearest first. Either list may be missing.
pub fn select_both(
    ios: Option<&RelayDirectory>,
    android: Option<&RelayDirectory>,
    geohash: &str,
) -> Result<Vec<Selected>> {
    Ok(geohash_relays::select::select_both(ios, android, geohash)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The rules themselves are tested in geohash-relays.
    #[test]
    fn errors_are_this_crates() {
        assert!(matches!(
            select_both(None, None, "u0a"),
            Err(crate::Error::InvalidGeohash(g)) if g == "u0a"
        ));
        let (lat, lon) = geohash_center("u09tv").unwrap();
        assert!((48.8..48.9).contains(&lat) && (2.3..2.4).contains(&lon));
    }
}
