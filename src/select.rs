//! The apps' relay choice for a channel: closest relays, iOS and Android (lots 1 and 11 bis).

use crate::GEO_RELAY_COUNT;
use crate::directory::{Platform, RelayDirectory, normalize_url};
use crate::error::{Error, Result};

/// A relay with its distance to a channel centre.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoRelay {
    pub url: String,
    pub lat: f64,
    pub lon: f64,
    pub km: f64,
}

/// A server chosen for a channel by iOS, Android or both.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    /// The server (`url` normalised, see [`normalize_url`]); coordinates and
    /// distance from the iOS list when both apps choose it.
    pub relay: GeoRelay,
    /// 1-based rank in the iOS choice (1 to `GEO_RELAY_COUNT`).
    pub ios: Option<usize>,
    /// 1-based ranks in the Android choice: empty, one, or two when the list
    /// has the server twice (`host` and `host:443`).
    pub android: Vec<usize>,
}

/// Centre (lat, lon) of a geohash cell.
pub fn geohash_center(geohash: &str) -> Result<(f64, f64)> {
    if geohash.is_empty() {
        return Err(Error::InvalidGeohash(geohash.to_owned()));
    }
    let (c, _, _) =
        geohash::decode(geohash).map_err(|_| Error::InvalidGeohash(geohash.to_owned()))?;
    Ok((c.y, c.x))
}

/// The app's relay choice: the `count` relays nearest to the channel centre,
/// with the rule of the app that reads `dir` (no tie beyond `count`).
///
/// iOS sorts its one-entry-per-host list by distance, then host. Android
/// sorts its raw list by distance only, with a stable sort, so ties keep the
/// list order, and it can pick the same server twice. The URLs are those of
/// the list ([`DirectoryRelay::url`](crate::directory::DirectoryRelay::url)).
pub fn closest(dir: &RelayDirectory, geohash: &str, count: usize) -> Result<Vec<GeoRelay>> {
    let (lat, lon) = geohash_center(geohash)?;
    let mut entries: Vec<GeoRelay> = dir
        .relays()
        .iter()
        .map(|r| GeoRelay {
            url: r.url.clone(),
            lat: r.lat,
            lon: r.lon,
            km: haversine_km(lat, lon, r.lat, r.lon),
        })
        .collect();
    match dir.platform() {
        Platform::Ios => {
            entries.sort_by(|a, b| a.km.total_cmp(&b.km).then_with(|| a.url.cmp(&b.url)))
        }
        // `sort_by` is stable, like Kotlin's `sortedBy`.
        Platform::Android => entries.sort_by(|a, b| a.km.total_cmp(&b.km)),
    }
    entries.truncate(count);
    Ok(entries)
}

/// Union of the iOS and Android choices for a channel, one entry per server,
/// nearest first. The two lists drift apart between reviews, and the apps
/// apply different rules, so they can pick different relays for the same
/// channel. Either list may be missing.
pub fn select_both(
    ios: Option<&RelayDirectory>,
    android: Option<&RelayDirectory>,
    geohash: &str,
) -> Result<Vec<Selected>> {
    // Rejects an invalid geohash even when both lists are missing.
    geohash_center(geohash)?;
    let mut out: Vec<Selected> = Vec::new();
    for (dir, is_ios) in [(ios, true), (android, false)] {
        let Some(dir) = dir else { continue };
        for (i, r) in closest(dir, geohash, GEO_RELAY_COUNT)?
            .into_iter()
            .enumerate()
        {
            // List entries are valid addresses: normalisation cannot fail.
            let url = normalize_url(&r.url).unwrap_or_else(|| r.url.clone());
            let at = match out.iter().position(|s| s.relay.url == url) {
                Some(at) => at,
                None => {
                    out.push(Selected {
                        relay: GeoRelay { url, ..r },
                        ios: None,
                        android: Vec::new(),
                    });
                    out.len() - 1
                }
            };
            if is_ios {
                out[at].ios = Some(i + 1);
            } else {
                out[at].android.push(i + 1);
            }
        }
    }
    out.sort_by(|a, b| {
        a.relay
            .km
            .total_cmp(&b.relay.km)
            .then_with(|| a.relay.url.cmp(&b.relay.url))
    });
    Ok(out)
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().asin()
}
