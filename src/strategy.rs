// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which relays the map listens to for a channel (lot 27, D28): what the
//! apps do today, by default, or one of geohash-relays' strategies, to see
//! how many of the people in a cell a phone following it would reach.
//!
//! The phones themselves follow [`StrategyKind::Apps`]: under another
//! strategy, the map sees only what reaches the relays that strategy picks.
//! The counts then say how well that strategy would connect the phones of
//! today. See geohash-relays' `docs/clients.md` and `docs/evaluation.md`.

use geohash_relays::a0::SharedStrategy;
use geohash_relays::a1::HrwStrategy;
use geohash_relays::{AirhopStrategy, AppStrategy, Apps, Strategy};

use crate::directory::RelayDirectory;
use crate::error::{Error, Result};

/// A relay choice the map can listen with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StrategyKind {
    /// bitchat iOS and Android, each with its list and rule (the union):
    /// where the phones are today.
    #[default]
    Apps,
    /// AirHop: the 10 nearest relays of georelays' list, ties by URL.
    Airhop,
    /// A0: one list, the iOS rule, write 8 and read 5.
    A0,
    /// A1: rendezvous hashing (HRW) over the list, no salt.
    A1,
}

impl StrategyKind {
    pub const ALL: [StrategyKind; 4] = [
        StrategyKind::Apps,
        StrategyKind::Airhop,
        StrategyKind::A0,
        StrategyKind::A1,
    ];

    /// The name on the command line and in the API.
    pub fn name(self) -> &'static str {
        match self {
            StrategyKind::Apps => "apps",
            StrategyKind::Airhop => "airhop",
            StrategyKind::A0 => "a0",
            StrategyKind::A1 => "a1",
        }
    }

    /// What the page shows.
    pub fn label(self) -> &'static str {
        match self {
            StrategyKind::Apps => "Today (AS): bitchat iOS + Android",
            StrategyKind::Airhop => "AirHop (A8): 10 nearest",
            StrategyKind::A0 => "A0: one list, write 8, read 5",
            StrategyKind::A1 => "A1: rendezvous hashing",
        }
    }

    pub fn parse(name: &str) -> Result<StrategyKind> {
        StrategyKind::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| Error::UnknownStrategy(name.to_owned()))
    }

    /// The choice itself, on the lists in use. AirHop, A0 and A1 read
    /// georelays' list (Android's source, updated daily), else the iOS one;
    /// with neither, they choose nothing.
    pub fn chooser(
        self,
        ios: Option<&RelayDirectory>,
        android: Option<&RelayDirectory>,
    ) -> Box<dyn Strategy> {
        let shared = android.or(ios);
        match (self, shared) {
            (StrategyKind::Apps, _) => {
                Box::new(AppStrategy::new(ios.cloned(), android.cloned(), Apps::Both))
            }
            (StrategyKind::Airhop, Some(list)) => Box::new(AirhopStrategy::from_directory(list)),
            (StrategyKind::A0, Some(list)) => {
                Box::new(SharedStrategy::from_directory(list.clone()))
            }
            (StrategyKind::A1, Some(list)) => Box::new(HrwStrategy::new(list)),
            (_, None) => Box::new(AppStrategy::new(None, None, Apps::Both)),
        }
    }
}

/// The other strategies of geohash-relays, in the page's list but not
/// selectable: what each needs that the map does not have. Name, label, why.
pub const NOT_AVAILABLE: [(&str, &str, &str); 5] = [
    (
        "a2",
        "A2: directory from NIP-66 monitors",
        "needs the reports of trusted NIP-66 monitors that test writes through Tor; few exist",
    ),
    (
        "a3",
        "A3: relays announced in presences",
        "needs presences that announce their relays: a change of the apps, none sends them yet",
    ),
    (
        "a4",
        "A4: republishers",
        "not a relay choice: peers that copy events between the apps' sets (bitchat-console --republish)",
    ),
    (
        "a5",
        "A5: federation of synchronised relays",
        "needs relays that synchronise with each other (NIP-77); no federation runs yet",
    ),
    (
        "a6",
        "A6: qualified relays only",
        "a filter added to another strategy: the map already sets aside the relays its probes find dead",
    ),
];

impl std::fmt::Display for StrategyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::Platform;

    const IOS: &str = include_str!("../tests/data/online_relays_gps.csv");
    const ANDROID: &str = include_str!("../tests/data/nostr_relays.csv");

    #[test]
    fn names_go_both_ways() {
        for k in StrategyKind::ALL {
            assert_eq!(StrategyKind::parse(k.name()).unwrap(), k);
        }
        assert_eq!(StrategyKind::parse(" A1 ").unwrap(), StrategyKind::A1);
        assert!(matches!(
            StrategyKind::parse("a9"),
            Err(Error::UnknownStrategy(n)) if n == "a9"
        ));
        assert_eq!(StrategyKind::default(), StrategyKind::Apps);
    }

    #[test]
    fn each_strategy_reads_its_own_number_of_relays() {
        let ios = RelayDirectory::parse(IOS, Platform::Ios);
        let android = RelayDirectory::parse(ANDROID, Platform::Android);
        let read = |k: StrategyKind| {
            k.chooser(Some(&ios), Some(&android))
                .read("u09tv")
                .unwrap()
                .len()
        };
        // The union of two choices of 5.
        assert!((5..=10).contains(&read(StrategyKind::Apps)));
        assert_eq!(read(StrategyKind::Airhop), 10);
        assert_eq!(read(StrategyKind::A0), 5);
        assert_eq!(read(StrategyKind::A1), 5);
        // No list: nothing chosen, nothing listened to.
        assert!(
            StrategyKind::A1
                .chooser(None, None)
                .read("u09tv")
                .unwrap()
                .is_empty()
        );
    }
}
