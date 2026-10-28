// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The relay lists in `tests/data/` are frozen: other tests rely on their
//! exact content. If this fails, the files were changed (see AGENTS.md).

const IOS: &str = include_str!("data/online_relays_gps.csv");
const ANDROID: &str = include_str!("data/nostr_relays.csv");

#[test]
fn relay_lists_are_unchanged() {
    for (csv, relays) in [(IOS, 441), (ANDROID, 381)] {
        assert_eq!(csv.lines().next(), Some("Relay URL,Latitude,Longitude"));
        assert_eq!(csv.lines().skip(1).count(), relays);
    }
}
