//! Live monitor of BitChat location channels (geohash).
//!
//! Enumerates the channels inside a geohash, computes the relays the iOS and
//! Android apps use for each one, listens to kinds 20000/20001 there and
//! aggregates activity per cell, without keeping personal data. See
//! `SPECIFICATIONS.md`, `PLAN_ET1.md` and `PLAN_ET2.md`.
//!
//! Feature `server` (on by default): the web server of the map and the
//! `bitchat-geohash` binary.

pub mod aggregate;
pub mod channels;
pub mod cli;
pub mod directory;
pub mod error;
pub mod listener;
pub mod monitor;
pub mod nostr;
pub mod plan;
pub mod probe;
pub mod select;
#[cfg(feature = "server")]
pub mod server;
pub mod store;

pub use error::{Error, Result};

/// Location-channel message (ephemeral).
pub const KIND_GEO_MESSAGE: u32 = 20000;
/// Location-channel presence heartbeat (ephemeral).
pub const KIND_GEO_PRESENCE: u32 = 20001;
/// Relays the app sends location-channel traffic to: the closest to the
/// channel centre, per app (Android may count one server twice).
pub const GEO_RELAY_COUNT: usize = 5;

/// Installs ring as the process-wide rustls crypto provider, needed before
/// any `wss://` connection or HTTPS download. Calling it again is harmless.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
