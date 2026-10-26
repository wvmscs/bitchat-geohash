//! Relay lists and the apps' relay choice, on the frozen lists. Expected
//! choices are the output of `bitchat-envelope`'s `geohash_map --no-probe`
//! on the same files.

use bitchat_geohash::Error;
use bitchat_geohash::directory::{RelayDirectory, normalize_url};
use bitchat_geohash::select::{closest, select_both};

const IOS: &str = include_str!("data/online_relays_gps.csv");
const ANDROID: &str = include_str!("data/nostr_relays.csv");

fn dirs() -> (RelayDirectory, RelayDirectory) {
    (RelayDirectory::parse(IOS), RelayDirectory::parse(ANDROID))
}

/// `(url without wss://, iOS rank, Android rank)`, nearest first.
fn choice(geohash: &str) -> Vec<(String, Option<usize>, Option<usize>)> {
    let (ios, android) = dirs();
    select_both(Some(&ios), Some(&android), geohash)
        .unwrap()
        .into_iter()
        .map(|s| (s.relay.url[6..].to_owned(), s.ios, s.android))
        .collect()
}

fn expected(rows: &[(&str, usize, usize)]) -> Vec<(String, Option<usize>, Option<usize>)> {
    let rank = |r: usize| (r > 0).then_some(r);
    rows.iter()
        .map(|&(url, i, a)| (url.to_owned(), rank(i), rank(a)))
        .collect()
}

#[test]
fn frozen_lists_parse_cleanly() {
    let (ios, android) = dirs();
    assert_eq!((ios.len(), ios.duplicates(), ios.rejected()), (326, 115, 0));
    assert_eq!(
        (android.len(), android.duplicates(), android.rejected()),
        (332, 49, 0)
    );
    assert!(ios.relays().iter().all(|r| r.url.starts_with("wss://")));
}

#[test]
fn normalisation_matches_the_app() {
    for raw in [
        "Relay.Example.com",
        "wss://relay.example.com/",
        "relay.example.com:443",
        " relay.example.com ",
    ] {
        assert_eq!(
            normalize_url(raw).as_deref(),
            Some("wss://relay.example.com")
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
               good.example:443,48.85,2.35\n\
               good.example,10,10\n";
    let dir = RelayDirectory::parse(csv);
    assert_eq!(dir.rejected(), 5);
    assert_eq!(dir.duplicates(), 1);
    // Same host, other coordinates: kept, the choice uses the closest.
    assert_eq!(dir.len(), 2);
    let near = closest(&dir, "s00", 5).unwrap();
    assert_eq!(near.len(), 1);
    assert_eq!((near[0].lat, near[0].lon), (10.0, 10.0));
}

#[test]
fn empty_list_selects_nothing() {
    let dir = RelayDirectory::parse("");
    assert!(dir.is_empty());
    assert!(closest(&dir, "u09tv", 5).unwrap().is_empty());
    let header_only = RelayDirectory::parse("Relay URL,Latitude,Longitude\n");
    assert!(closest(&header_only, "u09tv", 5).unwrap().is_empty());
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
            ("nostr.wild-vibes.ts.net", 1, 1),
            ("relay-fra.zombi.cloudrodion.com", 2, 2),
            ("relay.cypherflow.ai", 3, 0),
            ("relay.mmwaves.de", 4, 0),
            ("treuzkas.branruz.com", 5, 0),
            ("nostr-relay.xbytez.io", 0, 3),
            ("r.senx.io", 0, 4),
            ("relay.noeudlibre.fr", 0, 5),
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
fn ties_for_the_last_slot_are_included() {
    // iOS: 3 relays at 2 km on the same point compete for the 5th slot.
    assert_eq!(
        choice("9q8yy"),
        expected(&[
            ("bucket.coracle.social", 1, 1),
            ("kotukonostr.onrender.com", 2, 0),
            ("nostr-kyomu-haskell.onrender.com", 3, 2),
            ("nostr-rs-relay-qj1h.onrender.com", 4, 0),
            ("0x-nostr-relay.fly.dev", 5, 0),
            ("conduitl2.fly.dev", 6, 0),
            ("nostrcity-club.fly.dev", 7, 0),
            ("relay.nostrcade.com", 0, 3),
            ("relay.aarpia.com", 0, 4),
            ("articles.layer3.news", 0, 5),
        ])
    );
}

#[test]
fn ties_on_both_platforms() {
    assert_eq!(
        choice("gcpvj"),
        expected(&[
            ("relay.nearhood.co.uk", 1, 1),
            ("nostr.chaima.info", 2, 0),
            ("relay.bornheimer.app", 3, 0),
            ("relay.qstr.app", 4, 0),
            ("relay.staging.plebeian.market", 5, 2),
            ("relay.typedcypher.com", 6, 0),
            ("rele.speyhard.fi", 7, 0),
            ("relay.underorion.se", 0, 3),
            ("nostr.88mph.life", 0, 4),
            ("nostr-01.uid.ovh", 0, 5),
            ("nostr-02.uid.ovh", 0, 6),
            ("relay.mwaters.net", 0, 7),
            ("relay.nmail.li", 0, 8),
            ("relay.openspecs.uid.ovh", 0, 9),
        ])
    );
}

#[test]
fn region() {
    assert_eq!(
        choice("u0"),
        expected(&[
            ("relay.artio.inf.unibe.ch", 1, 1),
            ("nostr.sathoarder.com", 2, 2),
            ("nostr.unkn0wn.world", 0, 3),
            ("relay.pyramid.li", 3, 0),
            ("nostr.hifish.org", 4, 4),
            ("nostr.0x7e.xyz", 0, 5),
            ("nostr-2.21crypto.ch", 5, 0),
            ("nostr.21crypto.ch", 6, 0),
        ])
    );
}

#[test]
fn one_list_missing() {
    let (ios, _) = dirs();
    let only_ios = select_both(Some(&ios), None, "u09tv").unwrap();
    assert_eq!(only_ios.len(), 5);
    assert!(
        only_ios
            .iter()
            .all(|s| s.ios.is_some() && s.android.is_none())
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
