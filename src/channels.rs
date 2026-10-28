// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Channel levels and enumeration of the channels inside a geohash (lot 2).
//!
//! A geohash is a channel of its own, not a subset of its parent: an event of
//! `u09tv` carries only `["g","u09tv"]`. Watching a zone therefore means
//! watching every channel of every level inside it.

use crate::error::{Error, Result};

/// Geohash alphabet (base32 without `a`, `i`, `l`, `o`), in geohash order.
pub const BASE32: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// Default cap on the number of channels returned by [`enumerate`].
pub const DEFAULT_MAX_CHANNELS: usize = 2048;

/// Channel levels of the app. Precision 3 is not one: it only serves as an
/// aggregation mesh on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Region = 2,
    Province = 4,
    City = 5,
    Neighbourhood = 6,
    Block = 7,
}

impl Level {
    /// All channel levels, coarsest first.
    pub const ALL: [Level; 5] = [
        Level::Region,
        Level::Province,
        Level::City,
        Level::Neighbourhood,
        Level::Block,
    ];

    /// Levels observed unless asked otherwise: [`Level::Block`] is left out.
    pub const DEFAULT: [Level; 4] = [
        Level::Region,
        Level::Province,
        Level::City,
        Level::Neighbourhood,
    ];

    /// Geohash length of this level.
    pub fn precision(self) -> usize {
        self as usize
    }

    pub fn from_precision(precision: usize) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.precision() == precision)
    }

    /// Level of a channel, `None` for a length that is not a channel level.
    pub fn of(geohash: &str) -> Option<Level> {
        Level::from_precision(geohash.len())
    }
}

/// Validates a geohash usable as a start or a channel (2 to 7 characters of
/// the geohash alphabet) and returns it in lower case.
pub fn normalize(geohash: &str) -> Result<String> {
    let gh = geohash.trim().to_ascii_lowercase();
    if !(2..=Level::Block.precision()).contains(&gh.len())
        || !gh.bytes().all(|b| BASE32.contains(&b))
    {
        return Err(Error::InvalidGeohash(geohash.to_owned()));
    }
    Ok(gh)
}

/// Precisions of the channels [`enumerate`] returns for `start`: `start`'s
/// own if it is a channel level, then the next `depth` channel levels
/// (counting every app level, observed or not), kept if in `levels`.
fn target_levels(start: &str, depth: usize, levels: &[Level]) -> Vec<Level> {
    let own = Level::of(start).into_iter();
    let deeper = Level::ALL
        .into_iter()
        .filter(|l| l.precision() > start.len())
        .take(depth);
    own.chain(deeper).filter(|l| levels.contains(l)).collect()
}

/// Number of channels [`enumerate`] would return, without building them.
pub fn count(start: &str, depth: usize, levels: &[Level]) -> Result<u64> {
    let start = normalize(start)?;
    Ok(target_levels(&start, depth, levels)
        .into_iter()
        .map(|l| 32u64.pow((l.precision() - start.len()) as u32))
        .sum())
}

/// The channels of the requested levels inside `start`, `start` included if
/// it is a channel level. `depth` counts channel levels below `start`: from
/// `u09` (precision 3), depth 2 gives the precision 4 and 5 channels.
/// Coarsest level first, geohash order within a level.
///
/// More than `max` channels is an error that gives the actual count.
pub fn enumerate(start: &str, depth: usize, levels: &[Level], max: usize) -> Result<Vec<String>> {
    let total = count(start, depth, levels)?;
    if total > max as u64 {
        return Err(Error::TooManyChannels { count: total, max });
    }
    let start = normalize(start)?;
    let mut out = Vec::with_capacity(total as usize);
    for level in target_levels(&start, depth, levels) {
        // Breadth-first expansion from `start` down to this level.
        let mut cells = vec![start.clone()];
        for _ in start.len()..level.precision() {
            cells = cells.iter().flat_map(|c| children(c)).collect();
        }
        out.extend(cells);
    }
    Ok(out)
}

/// The 32 cells one character longer than `geohash`, in geohash order.
pub fn children(geohash: &str) -> impl Iterator<Item = String> + '_ {
    BASE32.iter().map(move |&c| {
        let mut child = String::with_capacity(geohash.len() + 1);
        child.push_str(geohash);
        child.push(c as char);
        child
    })
}
