// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Relay lists (`host,lat,lon` CSV): where they come from and how they are
//! read (lots 1, 8 and 11 bis). Parsing with each app's rules and
//! normalisation are geohash-relays' (D2 of bitchat-console), re-exported.

pub use geohash_relays::directory::{
    ANDROID_LIST_URL, DirectoryRelay, IOS_LIST_URL, Platform, RelayDirectory, normalize_url,
};

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
/// leave every channel without relay. In the browser, only URLs (and the
/// server must allow the page's origin: CORS).
pub async fn fetch(
    source: &ListSource,
    platform: Platform,
) -> crate::Result<(String, RelayDirectory)> {
    let csv = match source {
        ListSource::Url(url) => {
            #[cfg(not(target_arch = "wasm32"))]
            crate::install_crypto_provider();
            let download = async {
                reqwest::Client::new()
                    .get(url)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await
            };
            crate::rt::timeout(FETCH_TIMEOUT, download)
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "download timed out")
                })??
        }
        ListSource::Path(path) => {
            let path = path.clone();
            crate::rt::blocking(move || std::fs::read_to_string(path))
                .await
                .map_err(|_| std::io::Error::other("file read stopped"))??
        }
    };
    let dir = RelayDirectory::parse(&csv, platform);
    if dir.is_empty() {
        return Err(crate::Error::EmptyRelayList(source.describe()));
    }
    Ok((csv, dir))
}
