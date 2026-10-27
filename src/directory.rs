//! Relay lists (`host,lat,lon` CSV): parsing with each app's rules,
//! normalisation and sources (lots 1, 8 and 11 bis).

use std::collections::HashMap;

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
    /// The address as the app uses it: `wss://host` normalised for iOS, the
    /// raw text (with `wss://` added when there is no scheme) for Android.
    /// [`normalize_url`] gives the server it designates.
    pub url: String,
    pub lat: f64,
    pub lon: f64,
}

/// A parsed relay list, read with its app's rules. Parse once, then use it
/// for every channel.
#[derive(Debug, Clone)]
pub struct RelayDirectory {
    platform: Platform,
    relays: Vec<DirectoryRelay>,
    rejected: usize,
    duplicates: usize,
    conflict: Option<String>,
}

impl RelayDirectory {
    /// Parses a `host,lat,lon` list like `platform`'s app.
    ///
    /// Both: blank lines and header lines (starting with `relay url`, any
    /// case) are skipped; lines without a host, with fewer than 3 fields or
    /// with invalid coordinates are rejected and counted. Out-of-range
    /// coordinates are rejected too, although Android accepts them (D19).
    ///
    /// iOS keeps one entry per normalised host (`host`, `HOST:443` and
    /// `wss://host/` are one), with the first coordinates seen; later lines
    /// are counted as duplicates. A host listed at other coordinates is a
    /// [`conflict`](Self::conflict): iOS rejects such a list.
    ///
    /// Android keeps every line as is: no lowercasing, no `:443` removal, no
    /// deduplication, so `host` and `host:443` are two entries.
    pub fn parse(csv: &str, platform: Platform) -> Self {
        let mut dir = RelayDirectory {
            platform,
            relays: Vec::new(),
            rejected: 0,
            duplicates: 0,
            conflict: None,
        };
        let mut seen: HashMap<String, (f64, f64)> = HashMap::new();
        for line in csv.lines().map(str::trim) {
            if line.is_empty() || line.to_lowercase().starts_with("relay url") {
                continue;
            }
            let Some(relay) = parse_line(line, platform) else {
                dir.rejected += 1;
                continue;
            };
            if platform == Platform::Ios {
                match seen.get(&relay.url) {
                    Some(&at) => {
                        if at != (relay.lat, relay.lon) && dir.conflict.is_none() {
                            let host = relay.url.split_once("://").map_or("", |(_, h)| h);
                            dir.conflict = Some(host.to_owned());
                        }
                        dir.duplicates += 1;
                        continue;
                    }
                    None => {
                        seen.insert(relay.url.clone(), (relay.lat, relay.lon));
                    }
                }
            }
            dir.relays.push(relay);
        }
        dir
    }

    /// The app whose rules this list was read with.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Relays in list order.
    pub fn relays(&self) -> &[DirectoryRelay] {
        &self.relays
    }

    /// Lines that could not be read.
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// iOS: lines dropped because their host was already listed. Always 0
    /// for Android.
    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    /// iOS: the first host listed at two different coordinates. The app
    /// rejects such a list and keeps its previous one (D18). Only this cause
    /// of rejection is checked. Always `None` for Android.
    pub fn conflict(&self) -> Option<&str> {
        self.conflict.as_deref()
    }

    pub fn len(&self) -> usize {
        self.relays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.relays.is_empty()
    }
}

/// The server a relay address designates, as iOS keys it: `host`,
/// `wss://host`, `host/` and `host:443` are one server, case-insensitive.
/// Returns `wss://host` (`wss://host:port` for another port).
///
/// An explicit `ws://` is kept, for local test relays without TLS: the
/// published lists never contain one.
pub fn normalize_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("ws://") {
        let host = rest.trim_end_matches('/').to_lowercase();
        return (!host.is_empty()).then(|| format!("ws://{host}"));
    }
    let host = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let host = host.trim_end_matches('/');
    let host = host.strip_suffix(":443").unwrap_or(host).to_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(format!("wss://{host}"))
    }
}

fn parse_line(line: &str, platform: Platform) -> Option<DirectoryRelay> {
    let mut p = line.split(',').map(str::trim);
    let raw = p.next()?;
    let url = match platform {
        Platform::Ios => normalize_url(raw)?,
        // Android only adds a missing scheme. An empty host is still
        // rejected: nothing to connect to.
        Platform::Android => {
            normalize_url(raw)?;
            if raw.contains("://") {
                raw.to_owned()
            } else {
                format!("wss://{raw}")
            }
        }
    };
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
pub async fn fetch(
    source: &ListSource,
    platform: Platform,
) -> crate::Result<(String, RelayDirectory)> {
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
    let dir = RelayDirectory::parse(&csv, platform);
    if dir.is_empty() {
        return Err(crate::Error::EmptyRelayList(source.describe()));
    }
    Ok((csv, dir))
}
