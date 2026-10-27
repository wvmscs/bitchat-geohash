//! Relay lists and the apps' relay choice, on the frozen lists. Expected
//! choices are the output of `bitchat-envelope`'s `geohash_map --no-probe`
//! on the same files, after its relay selection corrections
//! (`Documents/relay-selection-corrections.md`, 27/10/2026, lot 11 bis).

use bitchat_geohash::Error;
use bitchat_geohash::directory::{Platform, RelayDirectory, normalize_url};
use bitchat_geohash::select::{closest, select_both};

const IOS: &str = include_str!("data/online_relays_gps.csv");
const ANDROID: &str = include_str!("data/nostr_relays.csv");

/// Three rows on one point, so only names and order decide.
const FIXTURE: &str = "Relay URL,Latitude,Longitude\n\
                       zeta.example,52.0,5.0\n\
                       alpha.example:443,52.0,5.0\n\
                       alpha.example,52.0,5.0\n\
                       Far.Example,10.0,10.0\n";

fn dirs() -> (RelayDirectory, RelayDirectory) {
    (
        RelayDirectory::parse(IOS, Platform::Ios),
        RelayDirectory::parse(ANDROID, Platform::Android),
    )
}

fn urls(dir: &RelayDirectory, geohash: &str, count: usize) -> Vec<String> {
    closest(dir, geohash, count)
        .unwrap()
        .into_iter()
        .map(|r| r.url)
        .collect()
}

type Choice = Vec<(String, Option<usize>, Vec<usize>)>;

/// `(url without wss://, iOS rank, Android ranks)`, nearest first.
fn choice(geohash: &str) -> Choice {
    let (ios, android) = dirs();
    select_both(Some(&ios), Some(&android), geohash)
        .unwrap()
        .into_iter()
        .map(|s| (s.relay.url[6..].to_owned(), s.ios, s.android))
        .collect()
}

/// `(url, iOS rank or 0, Android ranks)`.
fn expected(rows: &[(&str, usize, &[usize])]) -> Choice {
    rows.iter()
        .map(|&(url, i, a)| (url.to_owned(), (i > 0).then_some(i), a.to_vec()))
        .collect()
}

#[test]
fn frozen_lists_parse_cleanly() {
    let (ios, android) = dirs();
    assert_eq!((ios.len(), ios.duplicates(), ios.rejected()), (326, 115, 0));
    assert_eq!(ios.conflict(), None);
    // Android keeps every line, `host` and `host:443` included.
    assert_eq!(
        (android.len(), android.duplicates(), android.rejected()),
        (381, 0, 0)
    );
    assert!(ios.relays().iter().all(|r| r.url.starts_with("wss://")));
    assert!(android.relays().iter().any(|r| r.url.ends_with(":443")));
}

#[test]
fn normalisation_matches_ios() {
    for raw in [
        "Relay.Example.com",
        "wss://relay.example.com/",
        "relay.example.com:443",
        " relay.example.com ",
        "https://relay.example.com",
    ] {
        assert_eq!(
            normalize_url(raw).as_deref(),
            Some("wss://relay.example.com"),
            "{raw:?}"
        );
    }
    assert_eq!(
        normalize_url("relay.example.com:7777").as_deref(),
        Some("wss://relay.example.com:7777")
    );
    assert_eq!(normalize_url("wss://"), None);
}

#[test]
fn bad_lines_are_rejected_and_counted() {
    let csv = "Relay URL,Latitude,Longitude\n\
               good.example,48.85,2.35\n\
               \n\
               ,48.85,2.35\n\
               nolat.example,,2.35\n\
               text.example,abc,2.35\n\
               out.example,91,2.35\n\
               short.example,48.85\n\
               good.example:443,48.85,2.35\n";
    for platform in [Platform::Ios, Platform::Android] {
        let dir = RelayDirectory::parse(csv, platform);
        // Out-of-range coordinates are rejected for both apps (D19).
        assert_eq!(dir.rejected(), 5, "{platform:?}");
    }
    let ios = RelayDirectory::parse(csv, Platform::Ios);
    assert_eq!((ios.len(), ios.duplicates()), (1, 1));
    let android = RelayDirectory::parse(csv, Platform::Android);
    assert_eq!((android.len(), android.duplicates()), (2, 0));
}

#[test]
fn ios_keeps_the_first_coordinates_and_reports_the_conflict() {
    let csv = format!("{FIXTURE}alpha.example,1.0,1.0\n");
    assert_eq!(
        RelayDirectory::parse(FIXTURE, Platform::Ios).conflict(),
        None
    );
    let ios = RelayDirectory::parse(&csv, Platform::Ios);
    assert_eq!(ios.conflict(), Some("alpha.example"));
    let alpha = &ios.relays()[1];
    assert_eq!(
        (alpha.url.as_str(), alpha.lat),
        ("wss://alpha.example", 52.0)
    );
    // Android has no such check.
    assert_eq!(
        RelayDirectory::parse(&csv, Platform::Android).conflict(),
        None
    );
}

#[test]
fn ios_deduplicates_and_breaks_ties_by_host() {
    let ios = RelayDirectory::parse(FIXTURE, Platform::Ios);
    assert_eq!(
        urls(&ios, "u1", 2),
        ["wss://alpha.example", "wss://zeta.example"]
    );
    let all = urls(&ios, "u1", 5);
    assert_eq!(all.len(), 3);
    assert_eq!(all[2], "wss://far.example");
}

#[test]
fn android_keeps_raw_entries_in_list_order() {
    let android = RelayDirectory::parse(FIXTURE, Platform::Android);
    assert_eq!(
        urls(&android, "u1", 3),
        [
            "wss://zeta.example",
            "wss://alpha.example:443",
            "wss://alpha.example"
        ]
    );
    assert_eq!(urls(&android, "u1", 5)[3], "wss://Far.Example");
}

#[test]
fn union_is_by_server() {
    let ios = RelayDirectory::parse(FIXTURE, Platform::Ios);
    let android = RelayDirectory::parse(FIXTURE, Platform::Android);
    let both = select_both(Some(&ios), Some(&android), "u1").unwrap();
    let got: Choice = both
        .into_iter()
        .map(|s| (s.relay.url, s.ios, s.android))
        .collect();
    assert_eq!(
        got,
        vec![
            ("wss://alpha.example".into(), Some(1), vec![2, 3]),
            ("wss://zeta.example".into(), Some(2), vec![1]),
            ("wss://far.example".into(), Some(3), vec![4]),
        ]
    );
}

#[test]
fn empty_list_selects_nothing() {
    for platform in [Platform::Ios, Platform::Android] {
        let dir = RelayDirectory::parse("", platform);
        assert!(dir.is_empty());
        assert!(closest(&dir, "u09tv", 5).unwrap().is_empty());
        let header_only = RelayDirectory::parse("Relay URL,Latitude,Longitude\n", platform);
        assert!(closest(&header_only, "u09tv", 5).unwrap().is_empty());
    }
}

#[test]
fn invalid_geohash_is_an_error() {
    let (ios, _) = dirs();
    for gh in ["", "u0a", "U09"] {
        assert!(
            matches!(closest(&ios, gh, 5), Err(Error::InvalidGeohash(_))),
            "{gh:?}"
        );
        assert!(
            matches!(select_both(None, None, gh), Err(Error::InvalidGeohash(_))),
            "{gh:?}"
        );
    }
}

#[test]
fn paris_neighbourhood() {
    assert_eq!(
        choice("u09tv"),
        expected(&[
            ("nostr.wild-vibes.ts.net", 1, &[2]),
            ("relay-fra.zombi.cloudrodion.com", 2, &[1]),
            ("relay.cypherflow.ai", 3, &[]),
            ("relay.mmwaves.de", 4, &[]),
            ("treuzkas.branruz.com", 5, &[]),
            ("nostr-relay.xbytez.io", 0, &[4]),
            ("r.senx.io", 0, &[5]),
            ("relay.noeudlibre.fr", 0, &[3]),
        ])
    );
}

#[test]
fn parent_channel_has_its_own_centre() {
    // Same relays as u09tv here, but farther: the distance is to u09's centre.
    let (ios, _) = dirs();
    let near = closest(&ios, "u09", 5).unwrap();
    assert_eq!(near[0].url, "wss://nostr.wild-vibes.ts.net");
    assert_eq!(near[0].km.round(), 42.0);
}

#[test]
fn no_tie_beyond_the_fifth_slot() {
    // iOS: 4 relays on one point, then 3 at 2 km for the 5th slot; only the
    // first by host is taken. Android takes its tied relays in list order.
    assert_eq!(
        choice("9q8yy"),
        expected(&[
            ("bucket.coracle.social", 1, &[2]),
            ("kotukonostr.onrender.com", 2, &[]),
            ("nostr-kyomu-haskell.onrender.com", 3, &[1]),
            ("nostr-rs-relay-qj1h.onrender.com", 4, &[]),
            ("0x-nostr-relay.fly.dev", 5, &[]),
            ("relay.nostrcade.com", 0, &[3]),
            ("relay.aarpia.com", 0, &[4]),
            ("articles.layer3.news", 0, &[5]),
        ])
    );
    assert_eq!(
        choice("gcpvj"),
        expected(&[
            ("relay.nearhood.co.uk", 1, &[1]),
            ("nostr.chaima.info", 2, &[]),
            ("relay.bornheimer.app", 3, &[]),
            ("relay.qstr.app", 4, &[]),
            ("relay.staging.plebeian.market", 5, &[2, 3]),
            ("relay.underorion.se", 0, &[4]),
            ("nostr.88mph.life", 0, &[5]),
        ])
    );
}

#[test]
fn region() {
    assert_eq!(
        choice("u0"),
        expected(&[
            ("relay.artio.inf.unibe.ch", 1, &[1]),
            ("nostr.sathoarder.com", 2, &[2]),
            ("nostr.unkn0wn.world", 0, &[3]),
            ("relay.pyramid.li", 3, &[]),
            ("nostr.hifish.org", 4, &[4]),
            ("nostr.0x7e.xyz", 0, &[5]),
            ("nostr-2.21crypto.ch", 5, &[]),
        ])
    );
}

/// The cells of §4 of the corrections document.
#[test]
fn android_picks_a_server_twice() {
    assert_eq!(
        choice("u1"),
        expected(&[
            ("nostrelay.circum.space", 1, &[]),
            ("relay.cyberguy.fyi", 2, &[5]),
            ("relay.satsmarkt.club", 3, &[]),
            ("nostr.stakey.net", 0, &[1, 2]),
            ("relay.agorist.space", 4, &[]),
            ("relay.mccormick.cx", 5, &[3]),
            ("test.relays.nuts.cash", 0, &[4]),
        ])
    );
    assert_eq!(
        choice("gb"),
        expected(&[
            ("nostr.iskarion.ddns.net", 1, &[]),
            ("nostr.88mph.life", 2, &[1]),
            ("nostr.chaima.info", 3, &[]),
            ("relay.bornheimer.app", 4, &[]),
            ("relay.qstr.app", 5, &[]),
            ("relay.staging.plebeian.market", 0, &[2, 3]),
            ("relay.nearhood.co.uk", 0, &[4]),
            ("relay.underorion.se", 0, &[5]),
        ])
    );
    // relay.earthly.city has other coordinates in the Android list: shown
    // with the iOS ones, ranked 2nd by Android all the same.
    assert_eq!(
        choice("9q"),
        expected(&[
            ("relay.earthly.city", 1, &[2]),
            ("herbstmeister.com", 2, &[]),
            ("nostr-relay.zimage.com", 3, &[1]),
            ("nostr.2b9t.xyz", 4, &[3, 4]),
            ("syb.lol", 5, &[]),
            ("articles.layer3.news", 0, &[5]),
        ])
    );
}

/// The document's regression check: 322 of the 1,024 regions get fewer
/// than 5 distinct servers from Android (307 with 4, 15 with 3), the same
/// as an independent re-implementation of the Kotlin rule.
#[test]
fn android_reaches_fewer_than_five_servers_in_322_regions() {
    const BASE32: &str = "0123456789bcdefghjkmnpqrstuvwxyz";
    let (_, android) = dirs();
    let mut by_count = [0; 6];
    for a in BASE32.chars() {
        for b in BASE32.chars() {
            let mut servers: Vec<String> = closest(&android, &format!("{a}{b}"), 5)
                .unwrap()
                .iter()
                .filter_map(|r| normalize_url(&r.url))
                .collect();
            servers.sort();
            servers.dedup();
            by_count[servers.len()] += 1;
        }
    }
    assert_eq!(by_count, [0, 0, 0, 15, 307, 702]);
}

#[test]
fn one_list_missing() {
    let (ios, _) = dirs();
    let only_ios = select_both(Some(&ios), None, "u09tv").unwrap();
    assert_eq!(only_ios.len(), 5);
    assert!(
        only_ios
            .iter()
            .all(|s| s.ios.is_some() && s.android.is_empty())
    );
    assert!(select_both(None, None, "u09tv").unwrap().is_empty());
}

#[test]
fn explicit_ws_scheme_is_kept_for_local_relays() {
    assert_eq!(
        normalize_url("ws://127.0.0.1:7777/").as_deref(),
        Some("ws://127.0.0.1:7777")
    );
    assert_eq!(normalize_url("ws://"), None);
}
