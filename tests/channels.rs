// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Channel levels and enumeration.

use std::collections::HashSet;

use bitchat_geohash::Error;
use bitchat_geohash::channels::{
    BASE32, DEFAULT_MAX_CHANNELS, Level, children, count, enumerate, normalize,
};

fn lengths(channels: &[String]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for c in channels {
        match out.last_mut() {
            Some((len, n)) if *len == c.len() => *n += 1,
            _ => out.push((c.len(), 1)),
        }
    }
    out
}

#[test]
fn levels() {
    assert_eq!(Level::of("u0"), Some(Level::Region));
    assert_eq!(Level::of("u09"), None);
    assert_eq!(Level::of("u09t"), Some(Level::Province));
    assert_eq!(Level::of("u09tvqr"), Some(Level::Block));
    assert!(!Level::DEFAULT.contains(&Level::Block));
}

#[test]
fn counts_of_the_spec_table() {
    let only = |l: Level| [l];
    // From a region (2c), one level at a time.
    assert_eq!(count("u0", 1, &only(Level::Province)).unwrap(), 1_024);
    assert_eq!(count("u0", 2, &only(Level::City)).unwrap(), 32_768);
    assert_eq!(
        count("u0", 3, &only(Level::Neighbourhood)).unwrap(),
        1_048_576
    );
    // From a province (4c).
    assert_eq!(count("u09t", 1, &only(Level::City)).unwrap(), 32);
    assert_eq!(
        count("u09t", 2, &only(Level::Neighbourhood)).unwrap(),
        1_024
    );
    // From a city (5c).
    assert_eq!(count("u09tv", 1, &only(Level::Neighbourhood)).unwrap(), 32);
}

#[test]
fn start_of_precision_3_depth_2() {
    // u09 is not a channel: its precision 4 and 5 channels, 32 + 1 024.
    let channels = enumerate("u09", 2, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    assert_eq!(lengths(&channels), vec![(4, 32), (5, 1_024)]);
    assert_eq!(
        channels.len() as u64,
        count("u09", 2, &Level::DEFAULT).unwrap()
    );
    assert!(channels.iter().all(|c| c.starts_with("u09")));
    assert_eq!(
        channels.iter().collect::<HashSet<_>>().len(),
        channels.len()
    );
    assert_eq!(
        (channels[0].as_str(), channels[31].as_str()),
        ("u090", "u09z")
    );
}

#[test]
fn start_is_included_when_it_is_a_channel() {
    let channels = enumerate("u09t", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    assert_eq!(lengths(&channels), vec![(4, 1), (5, 32)]);
    assert_eq!(channels[0], "u09t");
    // Depth 0: the start alone.
    assert_eq!(
        enumerate("u09tv", 0, &Level::DEFAULT, 1).unwrap(),
        vec!["u09tv"]
    );
    assert!(enumerate("u09", 0, &Level::DEFAULT, 1).unwrap().is_empty());
}

#[test]
fn depth_counts_channel_levels_not_characters() {
    // From a region, depth 1 is precision 4 (precision 3 is not a level).
    let channels = enumerate("u0", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    assert_eq!(lengths(&channels), vec![(2, 1), (4, 1_024)]);
}

#[test]
fn levels_filter() {
    let channels = enumerate("u09t", 2, &[Level::Neighbourhood], DEFAULT_MAX_CHANNELS).unwrap();
    assert_eq!(lengths(&channels), vec![(6, 1_024)]);
    assert!(
        enumerate("u09t", 2, &[], DEFAULT_MAX_CHANNELS)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn block_level_only_on_request() {
    let default = enumerate("u09tvq", 1, &Level::DEFAULT, DEFAULT_MAX_CHANNELS).unwrap();
    assert_eq!(default, vec!["u09tvq"]);
    let with_block = enumerate("u09tvq", 1, &Level::ALL, DEFAULT_MAX_CHANNELS).unwrap();
    assert_eq!(lengths(&with_block), vec![(6, 1), (7, 32)]);
}

#[test]
fn too_many_channels_gives_the_count() {
    match enumerate("u0", 2, &Level::DEFAULT, DEFAULT_MAX_CHANNELS) {
        Err(Error::TooManyChannels { count, max }) => {
            assert_eq!((count, max), (1 + 1_024 + 32_768, DEFAULT_MAX_CHANNELS));
        }
        other => panic!("expected TooManyChannels, got {other:?}"),
    }
    // The cap itself is allowed.
    assert_eq!(enumerate("u09t", 1, &Level::DEFAULT, 33).unwrap().len(), 33);
}

#[test]
fn normalisation_and_invalid_input() {
    assert_eq!(normalize(" U09TV ").unwrap(), "u09tv");
    for bad in ["", "u", "u09tvqrs", "u0a", "u0i", "u0l", "u0o", "u0-", "ü0"] {
        assert!(
            matches!(normalize(bad), Err(Error::InvalidGeohash(_))),
            "{bad:?}"
        );
        assert!(
            matches!(
                enumerate(bad, 1, &Level::DEFAULT, 10),
                Err(Error::InvalidGeohash(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn children_are_the_32_cells_in_order() {
    let kids: Vec<String> = children("u0").collect();
    assert_eq!(kids.len(), 32);
    assert!(
        kids.iter()
            .zip(BASE32)
            .all(|(k, &c)| k.as_bytes() == [b'u', b'0', c])
    );
}
