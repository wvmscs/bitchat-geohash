//! The apps' relay choice for a channel: closest relays, iOS and Android (lot 1).

use std::collections::HashSet;

use crate::GEO_RELAY_COUNT;
use crate::directory::RelayDirectory;
use crate::error::{Error, Result};

/// A relay with its distance to a channel centre.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoRelay {
    pub url: String,
    pub lat: f64,
    pub lon: f64,
    pub km: f64,
}

/// A relay chosen for a channel by iOS, Android or both.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    pub relay: GeoRelay,
    /// 1-based rank in each platform's choice; above `GEO_RELAY_COUNT` means
    /// tied for the last slot.
    pub ios: Option<usize>,
    pub android: Option<usize>,
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

/// The app's relay choice: the `count` relays nearest to the channel centre.
/// iOS breaks distance ties by host, Android keeps list order, and lists
/// have relays with identical coordinates, so the two can disagree on the
/// last slot. To reach both, the result also includes every relay tied with
/// the last one.
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
    entries.sort_by(|a, b| a.km.total_cmp(&b.km).then_with(|| a.url.cmp(&b.url)));
    // A host listed with several coordinates counts once, at its closest.
    let mut seen = HashSet::new();
    entries.retain(|r| seen.insert(r.url.clone()));
    let Some(last) = entries
        .get(count.saturating_sub(1))
        .or(entries.last())
        .map(|r| r.km)
    else {
        return Ok(Vec::new());
    };
    Ok(entries
        .into_iter()
        .enumerate()
        .take_while(|(i, r)| *i < count || r.km == last)
        .map(|(_, r)| r)
        .collect())
}

/// Union of the iOS and Android choices for a channel, nearest first. The
/// two lists drift apart between reviews, so the apps can pick different
/// relays for the same channel. Either list may be missing.
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
            let at = match out.iter().position(|s| s.relay.url == r.url) {
                Some(at) => at,
                None => {
                    out.push(Selected {
                        relay: r,
                        ios: None,
                        android: None,
                    });
                    out.len() - 1
                }
            };
            if is_ios {
                out[at].ios = Some(i + 1);
            } else {
                out[at].android = Some(i + 1);
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
