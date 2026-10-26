//! Live monitor of BitChat location channels (geohash).
//!
//! Enumerates the channels inside a geohash, computes the relays the iOS and
//! Android apps use for each one, listens to kinds 20000/20001 there and
//! aggregates activity per cell, without keeping personal data. See
//! `SPECIFICATIONS.md` and `PLAN_ET1.md`.

pub mod aggregate;
pub mod channels;
pub mod directory;
pub mod error;
pub mod listener;
pub mod monitor;
pub mod plan;
pub mod probe;
pub mod select;
pub mod store;

pub use error::{Error, Result};

/// Location-channel message (ephemeral).
pub const KIND_GEO_MESSAGE: u32 = 20000;
/// Location-channel presence heartbeat (ephemeral).
pub const KIND_GEO_PRESENCE: u32 = 20001;
/// Relays the app sends location-channel traffic to: the closest to the
/// channel centre (plus ties for the last slot).
pub const GEO_RELAY_COUNT: usize = 5;
