// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Live monitor of BitChat location channels (geohash).
//!
//! Enumerates the channels inside a geohash, computes the relays the iOS and
//! Android apps use for each one, listens to kinds 20000/20001 there and
//! aggregates activity per cell, without keeping personal data. See
//! `SPECIFICATIONS.md`, `PLAN_ET1.md` and `PLAN_ET2.md`.
//!
//! Feature `server` (on by default): the web server of the map and the
//! `bitchat-geohash` binary.
//!
//! Browser build (`wasm32-unknown-unknown`, without default features): the
//! the monitor on the browser's tasks, timers and WebSockets (`rt`, `ws`),
//! with an in-memory store saved as a snapshot (`store::memory`).

pub mod aggregate;
pub mod channels;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod directory;
pub mod error;
#[cfg(feature = "geocode")]
pub mod geocode;
pub mod listener;
pub mod monitor;
pub mod nostr;
pub mod plan;
pub mod probe;
pub mod reach;
pub mod rt;
pub mod select;
#[cfg(feature = "server")]
pub mod server;
pub mod store;
pub mod strategy;
pub mod ws;

pub use error::{Error, Result};

/// The version of this build: its `v*` tag, else the first 7 characters of
/// its commit (see `build.rs`).
pub const VERSION: &str = env!("BITCHAT_VERSION");

/// Location-channel message (ephemeral).
pub const KIND_GEO_MESSAGE: u32 = 20000;
/// Location-channel presence heartbeat (ephemeral).
pub const KIND_GEO_PRESENCE: u32 = 20001;
/// Relays each app picks per channel (5).
pub use geohash_relays::GEO_RELAY_COUNT;

/// Installs ring as the process-wide rustls crypto provider, needed before
/// any `wss://` connection or HTTPS download. Calling it again is harmless.
#[cfg(not(target_arch = "wasm32"))]
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
