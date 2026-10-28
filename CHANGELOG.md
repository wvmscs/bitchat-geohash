# Changelog

## [Unreleased]

## [0.3.0] - 2026-10-29

### Changed
- Relay directory and relay choice come from the `geohash-relays` crate (lot 21), the same code bitchat-console uses: `directory::{RelayDirectory, DirectoryRelay, Platform, normalize_url, IOS_LIST_URL, ANDROID_LIST_URL}`, `select::{GeoRelay, Selected}` and `GEO_RELAY_COUNT` are re-exports; `select::{geohash_center, closest, select_both}` still return this crate's `Error`. Same public paths, same results.

### Removed
- The crate's own copy of the CSV parsing and selection code, its `tests/select.rs` (tested in `geohash-relays`) and the `geohash` dependency.

## [0.2.2] - 2026-10-28

### Changed
- Own Cargo workspace (`[workspace]` in `Cargo.toml`), like bitchat-console and bitchat-launcher: the crate builds the same anywhere, with its own `Cargo.lock` (it was out of date: built inside the local `Nostr/` workspace, it used that one) and its own `target/`.

## [0.2.1] - 2026-10-28

Faster starts and area changes (lot 20): about 0.5 s instead of 8 to 10 s.

### Changed
- Relay probes: a verdict in the database younger than `verdicts_fresh_for` (1 h) is used without probing again. Before listening, only the relays of the area's own channel (`g` = the area) are probed; the others are probed in the background and set aside as their verdicts come (`probe_in_background`, on by default). `probe::probe_where`, `Status::parse`.
- Map page: after an area change it shows the level of the area's subdivisions, not the level kept in the URL for the previous area.
- Relay lists are not downloaded again at start when the database has one from the same source loaded less than `reload_every` (24 h) ago, nor at an area change (lot 20); the next reload comes 24 h after that load. `--reload-lists` (`MonitorConfig::reload_lists`) downloads them at start anyway.

## [0.2.0] - 2026-10-28

### Added
- The watched area changes while the map runs (lot 19, D23): `POST /api/area {"geohash"}`, and an area field with a "Watch" button in the page's panel. The area is checked first (geohash, channel cap) and nothing stops if it is refused; then the monitor starts again on it with the same database, and the open pages get a new `hello` and frame the new area. A start that fails goes back to the previous area. `server::serve_with` takes a `Relaunch` (list sources or fixed lists, database path); `serve` keeps a fixed area (`409 area_fixed`). `monitor::check_area`, `Published::starting_at`, `Published::params`, `Error::AreaFixed`.
- bitchat-console follows its cell with the map: it asks for the parent of its cell.

## [0.1.0] - 2026-10-28

First version: the library (step 1) and the live map server with its `bitchat-geohash` binary (step 2).

### Fixed
- Binary: a missing `--tls-cert` or `--tls-key` file is reported at once, with the directory relative paths start from, instead of after the relay lists and probes.
- Map page over HTTP/1.1: a hidden tab lets go of its live stream after 10 s and takes it back when shown (D22). Each open stream holds one of the browser's 6 connections per host, and past them a new tab could not even load the page. HTTPS (HTTP/2) carries every stream on one connection.
- Map tiles blocked by OpenStreetMap under Firefox (403 "Access blocked"): the tile requests now carry the page origin as `Referer` (Leaflet `referrerPolicy`), as the OSM tile policy requires; the rest of the page keeps `no-referrer`.
- Map page: cells without colour can be hovered and clicked inside (transparent fill; an unfilled SVG shape only reacts on its outline).
- Relay choice: each app's own rule (step 2, lot 11 bis, from bitchat-envelope's relay selection corrections). iOS keeps one entry per normalised host (first coordinates) and takes exactly 5 by (distance, host); Android keeps raw lines (`host` and `host:443` are two entries) and takes exactly 5 by distance, stable, so it can pick a server twice. No more ties beyond the 5th slot.

### Changed
- `Push::Hello.params` is boxed (same JSON).
- CSP allows `data:` images (Leaflet's blank tile).
- `AggregatorConfig::k_threshold` defaults to 0: every count is shown, level 6 included (D20). The threshold stays available as an option.
- `RelayDirectory::parse(csv, platform)` and `directory::fetch(source, platform)` take the app; header lines are skipped wherever they are (`relay url…`). `normalize_url` strips any scheme except `ws://`.
- `Selected.android` and `CellRelay.android` are `Vec<usize>` (0, 1 or 2 ranks).
- An iOS list with a host at two coordinates (`RelayDirectory::conflict`, `Error::RelayListConflict`) is rejected as the app does: the last valid list in the database is used, or the current one on reload; with none, it is used with a warning (D18).

### Added
- `README.md` with a screenshot of the map (`docs/map.jpg`), how to run it over HTTP and HTTPS (step 2, lot 18).
- `bitchat-geohash` binary, full command line (step 2, lot 17): the spec's options (`--levels`, `--max-cells`, `--window`, `--inactivity`, list sources, `--no-probe`, `--reinstate-all`, `--forget-descents`, `--open`, `--verbose`…), a start-up summary on stderr (channels, relays and verdicts) then `map: <url>`, exit code 2 for a wrong command line and 1 for a start-up failure, with messages in the command line's words (busy port checked before the probes). `cli` module: option parsing shared with the `watch` example.
- Map layers (step 2, lot 16): relays (pins by status and platform, on a ring when they share coordinates, popup, lines from the hovered cell) and meshes (activity of the channels inside, levels 2 to 6, level 3 included). Server: `meshes` in `hello` and a `meshes` SSE message, `Params.mesh_levels`, `Published::update_meshes`. Both layers kept in the URL.
- Preview of the level below on hover (D21): `Monitor::preview` listens for a while to the channels of the next level inside a cell, saves nothing, and drops them from the plan `MonitorConfig::preview_for` (5 min) after the latest request unless a descent keeps them; previews give way, oldest first, under `max_channels`. `ListenPlan::remove`, `Aggregator::forget`, `MonitorEvent::Removed`, `CellStats.preview`, `CellView.preview`, `POST /api/preview`. The page asks after 400 ms over a cell, draws the channels below inside it and sums them up in the tooltip.
- Map page: going up a level from the list or the URL refits the map to the coarser cells in view.
- Map page interaction: click a watched cell to descend one level (refusals shown, with the real count over the cap), go back up one or several levels from the same menu, click a retired cell to reinstate it, "Reinstate all" with confirmation, breadcrumb to go back up, level and scale kept in the URL (step 2, lot 15).
- Map page: one rectangle per channel coloured by participants or messages (log scale, viridis), one level shown at a time, hover details (counts, relays with ranks and status, divergence), status panel, live updates in place from the SSE stream with automatic resume, "too many tabs" message, light and dark, 360 px. `view.js` (pure helpers, `node --test tests/js/`). Binary: `--depth`, `--tiles`, `--tiles-attribution` (step 2, lot 14).
- Optional TLS for the server (`ServerConfig::tls`, `TlsFiles`): HTTPS with a given certificate and key, HTTP/2 or HTTP/1.1 by ALPN; `Host` checked against the certificate names; `Error::Tls` for unreadable or mismatched files; a non-local address needs TLS unless `insecure_remote` (`Error::RemoteWithoutTls`). Binary: `--tls-cert`, `--tls-key`, `--insecure-remote`. Frozen test certificates in `tests/data/tls/` (step 2, lot 13 bis).
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
