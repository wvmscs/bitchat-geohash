//! Listen plan: relay to channels, filters, retire/reinstate, diff (lot 3).
//!
//! Relays are geolocated by IP and cluster in city centres, so thousands of
//! neighbouring channels share the same few relays. The plan inverts the
//! per-channel choice into `relay -> channels`, so that each relay gets one
//! connection carrying all its channels, in a few `#g` filters.

use std::collections::{BTreeMap, BTreeSet};

use bitchat_envelope::Filter;

use crate::directory::RelayDirectory;
use crate::error::Result;
use crate::select::{Selected, select_both};
use crate::{KIND_GEO_MESSAGE, KIND_GEO_PRESENCE};

/// Subscription sizing. Relays limit filter sizes and subscriptions per
/// connection without documenting it; the defaults are guesses (D3), to be
/// adjusted after measurements.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanConfig {
    /// Channels per `#g` filter, hence per subscription.
    pub max_tags_per_filter: usize,
    /// Subscriptions per connection. A relay needing more gets more than one
    /// connection (see [`ListenPlan::connections`]).
    pub max_subs_per_conn: usize,
    /// How far back the first REQ asks (`since = now - lookback`).
    pub lookback_secs: i64,
}

impl Default for PlanConfig {
    fn default() -> Self {
        PlanConfig {
            max_tags_per_filter: 100,
            max_subs_per_conn: 10,
            lookback_secs: 3600,
        }
    }
}

/// One subscription to a relay: a `#g` filter over some channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// Stable for a given plan: replacing a subscription reuses its id, which
    /// makes the relay drop the old filter.
    pub id: String,
    /// Which of the relay's connections carries it (see
    /// [`ListenPlan::connections`]); fixed for a given plan, like `id`.
    pub connection: usize,
    pub channels: Vec<String>,
}

impl Subscription {
    /// `{"kinds":[20000,20001], "#g":[...], "since": since}`.
    pub fn filter(&self, since: Option<i64>) -> Filter {
        Filter {
            kinds: vec![KIND_GEO_MESSAGE, KIND_GEO_PRESENCE],
            since,
            tags: BTreeMap::from([('g', self.channels.clone())]),
            ..Default::default()
        }
    }
}

/// What to change on the connections to move from one plan to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayChange {
    /// New relay: connect and send these subscriptions.
    Connect(Vec<Subscription>),
    /// No channel left on this relay: disconnect.
    Disconnect,
    /// Send these subscriptions (new, or replacing the one with the same id)
    /// and close these subscription ids.
    Update {
        send: Vec<Subscription>,
        close: Vec<String>,
    },
}

/// Changes per relay URL; relays without change are absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanDiff {
    pub relays: BTreeMap<String, RelayChange>,
}

impl PlanDiff {
    pub fn is_empty(&self) -> bool {
        self.relays.is_empty()
    }
}

#[derive(Debug, Clone)]
struct ChannelEntry {
    selected: Vec<Selected>,
    retired: bool,
}

/// Which relays to listen to, for which channels.
///
/// Each relay's channels (retired ones included) are split once, at build
/// time, into fixed groups of `max_tags_per_filter`, one subscription per
/// group. Retiring or reinstating a channel then only changes its own
/// group's subscription.
#[derive(Debug, Clone)]
pub struct ListenPlan {
    config: PlanConfig,
    channels: BTreeMap<String, ChannelEntry>,
    /// Relay -> its channel groups, retired channels included.
    groups: BTreeMap<String, Vec<Vec<String>>>,
    dead: BTreeSet<String>,
}

impl ListenPlan {
    /// Computes the relay choice of each channel (iOS and Android) and
    /// inverts it. Channels must be valid geohashes; duplicates count once.
    pub fn build(
        ios: Option<&RelayDirectory>,
        android: Option<&RelayDirectory>,
        channels: &[String],
        config: PlanConfig,
    ) -> Result<Self> {
        let mut entries = BTreeMap::new();
        let mut by_relay: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for channel in channels {
            if entries.contains_key(channel) {
                continue;
            }
            let selected = select_both(ios, android, channel)?;
            for s in &selected {
                by_relay
                    .entry(s.relay.url.clone())
                    .or_default()
                    .push(channel.clone());
            }
            entries.insert(
                channel.clone(),
                ChannelEntry {
                    selected,
                    retired: false,
                },
            );
        }
        let size = config.max_tags_per_filter.max(1);
        let groups = by_relay
            .into_iter()
            .map(|(relay, mut chans)| {
                chans.sort();
                (relay, chans.chunks(size).map(<[String]>::to_vec).collect())
            })
            .collect();
        Ok(ListenPlan {
            config,
            channels: entries,
            groups,
            dead: BTreeSet::new(),
        })
    }

    pub fn config(&self) -> &PlanConfig {
        &self.config
    }

    /// Watched channels (not retired), in geohash order.
    pub fn watched(&self) -> Vec<String> {
        self.channels_where(false)
    }

    /// Retired channels, in geohash order.
    pub fn retired(&self) -> Vec<String> {
        self.channels_where(true)
    }

    fn channels_where(&self, retired: bool) -> Vec<String> {
        self.channels
            .iter()
            .filter(|(_, e)| e.retired == retired)
            .map(|(c, _)| c.clone())
            .collect()
    }

    pub fn contains(&self, channel: &str) -> bool {
        self.channels.contains_key(channel)
    }

    pub fn is_retired(&self, channel: &str) -> bool {
        self.channels.get(channel).is_some_and(|e| e.retired)
    }

    /// The relays the apps choose for a channel, dead ones included.
    pub fn selection(&self, channel: &str) -> Option<&[Selected]> {
        self.channels.get(channel).map(|e| e.selected.as_slice())
    }

    /// Relays chosen for a channel by only one of the two apps: a message
    /// posted there from one app may never reach users of the other.
    pub fn divergence(&self, channel: &str) -> Vec<Selected> {
        self.selection(channel)
            .unwrap_or_default()
            .iter()
            .filter(|s| s.ios.is_some() != s.android.is_some())
            .cloned()
            .collect()
    }

    /// Every relay chosen for at least one channel, dead ones included.
    pub fn all_relays(&self) -> Vec<String> {
        self.groups.keys().cloned().collect()
    }

    /// Relays to listen to: alive and with at least one watched channel.
    pub fn relays(&self) -> Vec<String> {
        self.groups
            .keys()
            .filter(|r| !self.subscriptions(r).is_empty())
            .cloned()
            .collect()
    }

    /// Subscriptions for a relay: one per channel group that still has a
    /// watched channel. Empty for a dead relay or an unknown one.
    pub fn subscriptions(&self, relay: &str) -> Vec<Subscription> {
        if self.dead.contains(relay) {
            return Vec::new();
        }
        let Some(groups) = self.groups.get(relay) else {
            return Vec::new();
        };
        groups
            .iter()
            .enumerate()
            .filter_map(|(i, group)| {
                let channels: Vec<String> = group
                    .iter()
                    .filter(|c| !self.is_retired(c))
                    .cloned()
                    .collect();
                (!channels.is_empty()).then(|| Subscription {
                    id: format!("geo{i}"),
                    connection: i / self.config.max_subs_per_conn.max(1),
                    channels,
                })
            })
            .collect()
    }

    /// The subscriptions of a relay by connection, at most
    /// `max_subs_per_conn` each. A relay has more than one connection only
    /// with more than `max_tags_per_filter * max_subs_per_conn` channels.
    /// The assignment follows the channel groups, so retiring or
    /// reinstating never moves a subscription to another connection.
    pub fn connections(&self, relay: &str) -> BTreeMap<usize, Vec<Subscription>> {
        let mut out: BTreeMap<usize, Vec<Subscription>> = BTreeMap::new();
        for sub in self.subscriptions(relay) {
            out.entry(sub.connection).or_default().push(sub);
        }
        out
    }

    /// Sets relays aside (probe said they won't recover). They are not
    /// replaced: the apps would not use the next relay either.
    pub fn drop_relays(&mut self, relays: &[&str]) {
        self.dead.extend(relays.iter().map(|r| r.to_string()));
    }

    pub fn dead_relays(&self) -> Vec<String> {
        self.dead.iter().cloned().collect()
    }

    /// Stops listening to these channels. Returns those actually retired
    /// (known and watched until now).
    pub fn retire(&mut self, channels: &[&str]) -> Vec<String> {
        self.set_retired(channels, true)
    }

    /// Listens to these channels again. Returns those actually reinstated.
    pub fn reinstate(&mut self, channels: &[&str]) -> Vec<String> {
        self.set_retired(channels, false)
    }

    pub fn reinstate_all(&mut self) -> Vec<String> {
        let retired = self.retired();
        self.reinstate(&retired.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn set_retired(&mut self, channels: &[&str], retired: bool) -> Vec<String> {
        let mut changed = Vec::new();
        for &channel in channels {
            if let Some(e) = self.channels.get_mut(channel)
                && e.retired != retired
            {
                e.retired = retired;
                changed.push(channel.to_owned());
            }
        }
        changed
    }

    /// Changes to apply to the connections to go from `self` to `next`.
    pub fn diff(&self, next: &ListenPlan) -> PlanDiff {
        let old: BTreeSet<String> = self.relays().into_iter().collect();
        let new: BTreeSet<String> = next.relays().into_iter().collect();
        let mut relays = BTreeMap::new();
        for relay in old.difference(&new) {
            relays.insert(relay.clone(), RelayChange::Disconnect);
        }
        for relay in new.difference(&old) {
            relays.insert(
                relay.clone(),
                RelayChange::Connect(next.subscriptions(relay)),
            );
        }
        for relay in old.intersection(&new) {
            let before = self.subscriptions(relay);
            let after = next.subscriptions(relay);
            let send: Vec<Subscription> = after
                .iter()
                .filter(|s| !before.contains(s))
                .cloned()
                .collect();
            let close: Vec<String> = before
                .iter()
                .filter(|b| !after.iter().any(|a| a.id == b.id))
                .map(|b| b.id.clone())
                .collect();
            if !send.is_empty() || !close.is_empty() {
                relays.insert(relay.clone(), RelayChange::Update { send, close });
            }
        }
        PlanDiff { relays }
    }
}
