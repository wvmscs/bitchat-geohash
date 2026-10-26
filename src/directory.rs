//! Relay lists (`host,lat,lon` CSV): parsing, normalisation and sources (lots 1 and 8).

use std::collections::HashSet;

/// Which app reads a relay list: iOS reads bitchat's reviewed
/// `relays/online_relays_gps.csv`, Android georelays' daily `nostr_relays.csv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    Ios,
    Android,
}

impl Platform {
    /// `"ios"` or `"android"`, as stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
        }
    }
}

/// A relay of a list, with the coordinates the list gives it (estimated from
/// its IPv4 address upstream, hence many relays on the same point).
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryRelay {
    /// `wss://host`, normalised like the app.
    pub url: String,
    pub lat: f64,
    pub lon: f64,
}

/// A parsed relay list. Parse once, then use it for every channel.
#[derive(Debug, Clone, Default)]
pub struct RelayDirectory {
    relays: Vec<DirectoryRelay>,
    rejected: usize,
    duplicates: usize,
}

impl RelayDirectory {
    /// Parses a `host,lat,lon` list. The first line is a header and is
    /// skipped, as the app does. Blank lines are ignored; lines without a
    /// host or with invalid coordinates are rejected and counted. Entries
    /// identical after normalisation (`host` and `host:443`, say) are kept
    /// once and counted. The same host with other coordinates is kept: the
    /// relay choice then uses its closest entry, as the app does.
    pub fn parse(csv: &str) -> Self {
        let mut dir = RelayDirectory::default();
        let mut seen = HashSet::new();
        for line in csv.lines().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            let Some(relay) = parse_line(line) else {
                dir.rejected += 1;
                continue;
            };
            if seen.insert((relay.url.clone(), relay.lat.to_bits(), relay.lon.to_bits())) {
                dir.relays.push(relay);
            } else {
                dir.duplicates += 1;
            }
        }
        dir
    }

    /// Relays in list order.
    pub fn relays(&self) -> &[DirectoryRelay] {
        &self.relays
    }

    /// Lines that could not be read.
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// Lines dropped as duplicates of an earlier one.
    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    pub fn len(&self) -> usize {
        self.relays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.relays.is_empty()
    }
}

/// Normalises a relay host like the app: `host`, `wss://host`, `host/` and
/// `host:443` are one relay, case-insensitive. Returns `wss://host`.
///
/// An explicit `ws://` is kept, for local test relays without TLS: the
/// published lists never contain one.
pub fn normalize_url(raw: &str) -> Option<String> {
    if let Some(rest) = raw.trim().strip_prefix("ws://") {
        let host = rest.trim_end_matches('/').to_lowercase();
        return (!host.is_empty()).then(|| format!("ws://{host}"));
    }
    let host = raw
        .trim()
        .trim_start_matches("wss://")
        .trim_end_matches('/');
    let host = host.strip_suffix(":443").unwrap_or(host).to_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(format!("wss://{host}"))
    }
}

fn parse_line(line: &str) -> Option<DirectoryRelay> {
    let mut p = line.split(',').map(str::trim);
    let url = normalize_url(p.next()?)?;
    let lat: f64 = p.next()?.parse().ok()?;
    let lon: f64 = p.next()?.parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some(DirectoryRelay { url, lat, lon })
}

/// iOS list, as the app downloads it (`GeoRelayDirectory.swift`).
pub const IOS_LIST_URL: &str = "https://raw.githubusercontent.com/permissionlesstech/bitchat/refs/heads/main/relays/online_relays_gps.csv";
/// Android list: georelays' daily output, unreviewed.
pub const ANDROID_LIST_URL: &str =
    "https://raw.githubusercontent.com/permissionlesstech/georelays/main/nostr_relays.csv";

/// Where a relay list comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListSource {
    Url(String),
    Path(std::path::PathBuf),
}

impl ListSource {
    /// The official list of a platform (D5).
    pub fn official(platform: Platform) -> Self {
        ListSource::Url(
            match platform {
                Platform::Ios => IOS_LIST_URL,
                Platform::Android => ANDROID_LIST_URL,
            }
            .to_owned(),
        )
    }

    /// `http://` or `https://` is a URL, anything else a local path.
    pub fn parse(s: &str) -> Self {
        if s.starts_with("https://") || s.starts_with("http://") {
            ListSource::Url(s.to_owned())
        } else {
            ListSource::Path(s.into())
        }
    }

    /// As recorded in the database.
    pub fn describe(&self) -> String {
        match self {
            ListSource::Url(u) => u.clone(),
            ListSource::Path(p) => p.display().to_string(),
        }
    }
}

/// Download or read timeout.
pub const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Reads a list. A list without any usable relay is an error: it would
/// leave every channel without relay.
pub async fn fetch(source: &ListSource) -> crate::Result<(String, RelayDirectory)> {
    let csv = match source {
        ListSource::Url(url) => {
            crate::install_crypto_provider();
            let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build()?;
            client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?
        }
        ListSource::Path(path) => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))??
        }
    };
    let dir = RelayDirectory::parse(&csv);
    if dir.is_empty() {
        return Err(crate::Error::EmptyRelayList(source.describe()));
    }
    Ok((csv, dir))
}
