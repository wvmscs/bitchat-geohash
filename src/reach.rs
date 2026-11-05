// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a phone following the strategy listened with exchanges messages
//! with the reference apps as released, unchanged (lot 28): bitchat iOS and
//! bitchat Android, each with its list and rule. Two phones exchange in a
//! cell when each writes to a relay the other reads, and that relay works.
//!
//! Under the apps' own strategy, the question is whether an iOS phone and an
//! Android phone exchange with each other.

use geohash_relays::{Strategy, normalize_url};

use crate::GEO_RELAY_COUNT;
use crate::directory::RelayDirectory;
use crate::error::Result;
use crate::select::closest;

/// For one cell: `None` when that app's list is not loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reach {
    pub ios: Option<bool>,
    pub android: Option<bool>,
}

/// The relays of a cell for the strategy and for each reference app, as
/// normalised addresses: computed once per cell, the relays' health applied
/// at each reading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellRelays {
    pub ios: Option<Vec<String>>,
    pub android: Option<Vec<String>>,
    pub read: Vec<String>,
    pub write: Vec<String>,
}

fn normalised(urls: impl IntoIterator<Item = String>) -> Vec<String> {
    urls.into_iter()
        .map(|u| normalize_url(&u).unwrap_or(u))
        .collect()
}

impl CellRelays {
    pub fn of(
        strategy: &dyn Strategy,
        ios: Option<&RelayDirectory>,
        android: Option<&RelayDirectory>,
        cell: &str,
    ) -> Result<CellRelays> {
        let app = |dir: Option<&RelayDirectory>| -> Result<Option<Vec<String>>> {
            dir.map(|d| {
                Ok(normalised(
                    closest(d, cell, GEO_RELAY_COUNT)?
                        .into_iter()
                        .map(|r| r.url),
                ))
            })
            .transpose()
        };
        Ok(CellRelays {
            ios: app(ios)?,
            android: app(android)?,
            read: normalised(strategy.read(cell)?.into_iter().map(|s| s.relay.url)),
            write: normalised(strategy.write(cell)?.into_iter().map(|s| s.relay.url)),
        })
    }

    /// With `apps` (the apps' own strategy), iOS with Android, both fields
    /// alike; otherwise the strategy's phone with each app. `alive`: whether
    /// a relay works (not found dead).
    pub fn reach(&self, apps: bool, alive: impl Fn(&str) -> bool) -> Reach {
        // A writes where B reads, and B where A reads, on working relays.
        let meets =
            |a_write: &[String], a_read: &[String], b_write: &[String], b_read: &[String]| {
                let one_way =
                    |w: &[String], r: &[String]| w.iter().any(|u| alive(u) && r.contains(u));
                one_way(a_write, b_read) && one_way(b_write, a_read)
            };
        if apps {
            let both = match (&self.ios, &self.android) {
                (Some(i), Some(a)) => Some(meets(i, i, a, a)),
                _ => None,
            };
            return Reach {
                ios: both,
                android: both,
            };
        }
        let with =
            |app: &Option<Vec<String>>| app.as_ref().map(|r| meets(&self.write, &self.read, r, r));
        Reach {
            ios: with(&self.ios),
            android: with(&self.android),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::Platform;
    use crate::strategy::StrategyKind;

    fn set(urls: &[&str]) -> Vec<String> {
        urls.iter().map(|u| u.to_string()).collect()
    }

    #[test]
    fn both_ways_on_working_relays() {
        let c = CellRelays {
            ios: Some(set(&["wss://a", "wss://b"])),
            android: Some(set(&["wss://c"])),
            read: set(&["wss://b", "wss://d"]),
            write: set(&["wss://b", "wss://d", "wss://e"]),
        };
        let all = |_: &str| true;
        assert_eq!(
            c.reach(false, all),
            Reach {
                ios: Some(true),
                android: Some(false)
            }
        );
        // The one relay in common is dead: no exchange.
        assert_eq!(c.reach(false, |u: &str| u != "wss://b").ios, Some(false));
        // The apps' strategy: iOS with Android, which share nothing.
        assert_eq!(
            c.reach(true, all),
            Reach {
                ios: Some(false),
                android: Some(false)
            }
        );
        let no_android = CellRelays { android: None, ..c };
        assert_eq!(no_android.reach(false, all).android, None);
        assert_eq!(no_android.reach(true, all).ios, None);
    }

    #[test]
    fn airhop_meets_the_apps_on_their_own_list() {
        let ios = RelayDirectory::parse(
            include_str!("../tests/data/online_relays_gps.csv"),
            Platform::Ios,
        );
        let android = RelayDirectory::parse(
            include_str!("../tests/data/nostr_relays.csv"),
            Platform::Android,
        );
        let airhop = StrategyKind::Airhop.chooser(Some(&ios), Some(&android));
        let c = CellRelays::of(airhop.as_ref(), Some(&ios), Some(&android), "u09tv").unwrap();
        assert_eq!(c.read.len(), 10);
        // Same list as Android, its 5 first are a superset of Android's choice.
        assert_eq!(c.reach(false, |_| true).android, Some(true));
    }
}
