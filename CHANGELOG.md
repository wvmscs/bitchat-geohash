# Changelog

## [Unreleased]

### Fixed
- Relay choice: each app's own rule (step 2, lot 11 bis, from bitchat-envelope's relay selection corrections). iOS keeps one entry per normalised host (first coordinates) and takes exactly 5 by (distance, host); Android keeps raw lines (`host` and `host:443` are two entries) and takes exactly 5 by distance, stable, so it can pick a server twice. No more ties beyond the 5th slot.

### Changed
- `AggregatorConfig::k_threshold` defaults to 0: every count is shown, level 6 included (D20). The threshold stays available as an option.
- `RelayDirectory::parse(csv, platform)` and `directory::fetch(source, platform)` take the app; header lines are skipped wherever they are (`relay url…`). `normalize_url` strips any scheme except `ws://`.
- `Selected.android` and `CellRelay.android` are `Vec<usize>` (0, 1 or 2 ranks).
- An iOS list with a host at two coordinates (`RelayDirectory::conflict`, `Error::RelayListConflict`) is rejected as the app does: the last valid list in the database is used, or the current one on reload; with none, it is used with a warning (D18).

### Added
- `server::serve` (`ServerConfig`, `Server`): live map server around a running `Monitor`: `/api/state`, `/api/events` (SSE: `hello` then deltas, pushed at most once per second), `POST /api/descend` and `/api/reinstate` with JSON errors; `Host`, `Origin` and `Content-Type` checks, CSP, bounded bodies and streams; loopback only unless `allow_remote` (`Error::RemoteListen`). The binary takes a geohash and serves the live map (step 2, lot 13).
- `server::model`: what the browser receives: `CellView`, `MeshView`, `RelayView`, `StatusView`, `Params`, SSE messages (`Push`: `hello`, `cells`, `removed`, `relays`, `status`, with a sequence number) and deltas computed once for all browsers (`Published`). A participant count hidden by the k threshold is `"low"` (step 2, lot 12).
- `monitor`: channels added while running (`Monitor::descend`, `Monitor::watch_channels`), listened to at once, new relays probed in the background, kept across restarts (migration `0002_channel_origin`, `forget_descents`); `Monitor::relays()` (`RelayInfo`); change notifications (`Monitor::subscribe`, `MonitorEvent`). `ListenPlan::extend`. `watch`: `descend <geohash>`, `--forget-descents` (step 2, lot 11).
- Feature `server` (default) and the `bitchat-geohash` binary: axum router serving the map page and its built-in files, Leaflet 1.9.4 included (step 2, lot 10).
- Crate skeleton: modules of step 1, `Error` type, protocol constants.
- `directory`: relay list parsing (`RelayDirectory`, `Platform`, `normalize_url`).
- `select`: the apps' relay choice per channel (`closest`, `select_both`), ties for the last slot included.
- `channels`: channel levels (`Level`), validation, enumeration with a cap (`enumerate`, `count`).
- `plan`: listen plan (`ListenPlan`): relay → channels, `#g` subscriptions in stable groups, retire/reinstate, dead relays, diff between plans.
- `store`: SQLite store with append-only, versioned migrations (`0001_init`), writer thread (`StoreWriter`), relay list fallback, channel states, per-minute cell counters.
- `probe`: relay probe (`Status`, `Verdict`, `probe_relays`), `install_crypto_provider()`.
- `aggregate`: `Aggregator` (deduplication, signature check, sliding window, inactivity, k threshold, meshes, per-minute buckets).
- `listener`: one task per relay connection, plan sync, reconnection with jittered backoff, AUTH ignored, abandon on dead or AUTH-only relays.
- `monitor`: `Monitor` control loop: retirement of inactive channels (logged at INFO, kept across restarts), reinstatement, snapshots, status.
- Relay list sources (`ListSource`, `ListSources::official()`, `fetch`), `Monitor::start_from_sources` with fallback on the last valid list and reload every 24 h.
- `examples/watch.rs`: terminal monitor (plan, relay verdicts, refreshed table of active cells, stdin commands).
- Frozen relay lists in `tests/data/` (iOS `online_relays_gps.csv`, Android `nostr_relays.csv`).

### Changed
- `Cargo.lock` committed: standalone builds are reproducible (same versions as the local workspace).
- No dependency on `bitchat-envelope` any more: `nostr` module (NIP-01 event, Schnorr verification with `k256`, filter, `unix_now`); tests use an integrated NIP-01 relay (`tests/common/relay.rs`). The crate builds on its own.
- License: GPL-3.0-only (the repository's `LICENSE`).
- Default inactivity delay before a channel is retired: 15 min (was 5 min).
- `normalize_url` keeps an explicit `ws://` (local test relays).
- Swift package moved to `swift/`.
