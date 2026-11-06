// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the map keeps (lot 4): the records, and the store that holds them.
//! Native build: SQLite with versioned schema migrations (`sqlite`).
//!
//! Times are Unix seconds, always given by the caller (injected clock). The
//! schema holds no pubkey, nickname or message content (spec §5).

#[cfg(not(target_arch = "wasm32"))]
mod migrations;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;

use sha2::{Digest, Sha256};

use crate::directory::Platform;

#[cfg(not(target_arch = "wasm32"))]
pub use migrations::SCHEMA_VERSION;
#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::{Store, StoreWriter};

/// Whether a channel is listened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelState {
    Watched,
    Retired,
}

impl ChannelState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelState::Watched => "watched",
            ChannelState::Retired => "retired",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "watched" => Some(ChannelState::Watched),
            "retired" => Some(ChannelState::Retired),
            _ => None,
        }
    }
}

/// Counters of one channel over one minute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellBucket {
    pub geohash: String,
    /// Start of the minute, Unix seconds (a multiple of 60).
    pub bucket_start: i64,
    pub messages: u32,
    pub presences: u32,
    pub participants: u32,
}

/// A relay list load, with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayListLoad {
    pub platform: Platform,
    pub source: String,
    pub loaded_at: i64,
    pub sha256: String,
    pub relays: u32,
    pub rejected: u32,
    pub csv: String,
}

/// One write, for [`StoreWriter::send`].
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    RelayList(RelayListLoad),
    Probe {
        relay: String,
        probed_at: i64,
        status: String,
        detail: String,
    },
    ChannelState {
        geohash: String,
        state: ChannelState,
        changed_at: i64,
        reason: Option<String>,
    },
    Buckets(Vec<CellBucket>),
    /// A channel added by a descent: watched, restored at the next start.
    Descent {
        geohash: String,
        added_at: i64,
    },
    /// Drops every channel added by a descent (`--forget-descents`).
    ForgetDescents,
}

/// SHA-256 of a relay list, hex: tells whether a reload changed anything.
pub fn sha256_hex(csv: &str) -> String {
    crate::nostr::hex_encode(&Sha256::digest(csv.as_bytes()))
}
