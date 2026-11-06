# AGENTS.md

Guidance for coding agents (and humans) working in this crate.

## What this is

`bitchat-geohash`: live monitor of BitChat location channels (geohash). It enumerates the
channels inside a geohash, computes the relays the iOS and Android apps use for each one,
listens to kinds 20000/20001 there, and aggregates activity per cell, without personal data.
A Rust library, and an internal web server in Rust (axum, feature `server`) that serves the map.

## Constraints

### Do NOT
- Modify another repository from here: `geohash-relays`, `bitchat-console` and
  `bitchat-launcher` follow their own `AGENTS.md`. Rules that only concern the author's local
  checkout, if any, are in `AGENTS.local.md` (not committed).
- Modify files without reading them first.
- Modify existing tests, except to fix them or when an instruction in this file requires it
  (for example, extending the privacy tests when adding fields).
- Edit or regenerate `tests/data/*.csv` or `tests/data/tls/`: they are frozen for
  reproducibility (the TLS files are test-only; their CA key was deleted).
- Edit a migration that has been committed (see invariant 4).
- Add dependencies without discussion. Approved: those already in `Cargo.toml`.
- Edit `src/server/assets/leaflet/`: Leaflet 1.9.4 as released (a test checks its SHA-256).
- Remove existing functionality.

### DO
- Read relevant files before making changes.
- Follow existing naming conventions.
- Add comments for complex logic.
- Update documentation when adding features.
- Before each commit, run the commands listed under Commands.

### Others
- This crate is its own git repository (`origin` = `git@github.com:wvmscs/bitchat-geohash.git`,
  branch `main`), its own Cargo workspace (`[workspace]` in `Cargo.toml`).
- Pushing commits and tags to `origin` is authorized.
  Never force-push or rewrite pushed history without an explicit request.

## Workflow

1. Always read the file you are about to modify.
2. Check existing tests before changing code.
3. Check the existing API before writing code (`nostr::{Event, Filter, unix_now}`,
   `tests/common/`). Be DRY.
4. Run the commands listed under Commands.
5. Update tests for new functionality.
6. Keep commits focused and atomic.

## Environment

### Secret Management
- Never commit API keys or secrets.
- Use environment variables for sensitive data; reference them via environment variables or
  `.env` files.
- Never commit `.env` files; build and maintain `.env.sample` for the tests.
- Never commit SQLite databases (`*.sqlite`, `*.db`).

## Quality Gates

### Before PR
1. All tests pass.
2. No errors, no clippy warnings.
3. Code is formatted (`cargo fmt`).
4. Documentation updated (README, rustdoc).
5. Changelog entry added (`CHANGELOG.md` in this crate).

### PR Template
```markdown
## Description

[What this PR does]

## Related Issues

[GitHub issue numbers]

## Changes
- [ ] Feature addition
- [ ] Bug fix
- [ ] Documentation
- [ ] Refactoring
- [ ] Dependencies
- [ ] Schema migration

## Testing
- [ ] Unit tests added
- [ ] Integration tests pass (local `TestRelay`)
- [ ] Manual testing completed (`examples/watch.rs` on real relays)

## Screenshots (if UI changes)

 ![Screenshot](path/to/screenshot)
```

## Commands

Run from the crate (its own Cargo workspace):

```sh
cargo fmt -p bitchat-geohash --check
cargo test -p bitchat-geohash
cargo test -p bitchat-geohash --no-default-features                          # library without the server
cargo clippy -p bitchat-geohash --all-targets --all-features -- -D warnings   # must be 0 warnings
cargo clippy -p bitchat-geohash --all-targets --no-default-features -- -D warnings
cargo test -p bitchat-geohash --release -- --ignored                         # timing tests (plan build)
node --test tests/js/view.test.mjs                                          # page helpers (view.js)
cargo clippy -p bitchat-geohash --target wasm32-unknown-unknown --no-default-features --lib -- -D warnings  # browser build
web/build.sh && cargo test -p bitchat-geohash --test browser -- --ignored   # browser build in headless Chrome
BITCHAT_TEST_RELAY=wss://… cargo test --test browser -- --ignored the_map_on_the_test_relay   # closed loop (nak)
reuse lint                                                                  # copyright and licence of every file
cargo run -p bitchat-geohash --example watch -- u09 --depth 2                # real network
cargo run -p bitchat-geohash --example watch -- u09tvw --depth 0 --no-android \
    --relays-csv own.csv --db own.sqlite --refresh 1s     # one relay only (own.csv lists it)
cargo run -p bitchat-geohash -- u09 --listen 127.0.0.1:8080                 # map server (real network)
cargo run -p bitchat-geohash -- u09 --tls-cert localhost+2.pem --tls-key localhost+2-key.pem   # HTTPS, HTTP/2
```

<!-- REUSE-IgnoreStart -->
A new file starts with `SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)` and
`SPDX-License-Identifier: GPL-3.0-or-later` in a comment, or is covered by `REUSE.toml`;
third-party files (Leaflet, relay lists) keep their own lines there.
<!-- REUSE-IgnoreEnd -->

`Cargo.lock` is committed for reproducible builds; Cargo uses it directly (check with
`cargo test --locked`).

Tests run offline: the frozen CSVs from `tests/data/` and the local `TestRelay`
(`tests/common/relay.rs`). Only the `watch` example and the binary touch the real network
(the browser also loads map tiles).

## Layout

- `src/nostr.rs`: NIP-01 subset: `Event` (id, Schnorr verification), `Filter`, `unix_now`.
- `src/cli.rs`: command-line options shared by the binary and `watch` (`cli::parse`; each
  caller reads its own extra options). No argument-parsing dependency.
- `src/directory.rs`: `RelayDirectory` (CSV parsing with each app's rules, normalisation,
  iOS conflict check) re-exported from `geohash-relays`; list sources and download here.
- `src/strategy.rs`: the relay strategy the map listens with (`StrategyKind`): the
  apps' choice by default, or AirHop's, A0, A1 from `geohash-relays`.
- `src/reach.rs`: whether a cell's phones exchange with the reference apps as released.
- `src/select.rs`: relay choice per channel (`closest`, `select_both`) from `geohash-relays`,
  wrapped to return this crate's `Error`. Fix selection bugs in `geohash-relays`, not here.
- `src/channels.rs`: `Level`, channel enumeration.
- `src/plan.rs`: `ListenPlan` (relay → channels), filters, retire/reinstate, diff.
- `src/store/`: the records (`mod.rs`); SQLite `Store` and `StoreWriter` (`sqlite.rs`) and
  `migrations/NNNN_name.sql`; the browser build's store, in memory and saved as a JSON
  snapshot (`memory.rs`, built natively too: `tests/store_memory.rs` checks both stores).
- `src/rt.rs`, `src/ws.rs`: tasks, timers, clocks and a text WebSocket, tokio and
  tokio-tungstenite natively, the browser's in `wasm32`. Library code uses them, not tokio's
  runtime or tungstenite directly.
- `src/probe.rs`: relay probe, from `probe.rs`.
- `src/aggregate.rs`: deduplication, window, inactivity, `CellStats`.
- `src/listener.rs`: one tokio task per relay.
- `src/monitor.rs`: wiring, control loop, 24 h reload; descents (`descend`, `watch_channels`),
  `relays()`, change notifications (`subscribe`).
- `src/server/` (feature `server`, on by default): axum router; `assets.rs` lists the files
  built into the binary from `src/server/assets/` (page, `app.js`, `view.js`, `app.css`,
  Leaflet). `view.js` holds the page's pure helpers (no DOM), tested with node.
  `api.rs`: `/api/*` routes and the SSE stream over HTTP; `guard.rs`: `Host`/`Origin`/`Content-Type` checks and security
  headers; `proxy.rs`: behind a TLS proxy (checks, start probe); `tls.rs`: optional TLS (certificate and key files, ALPN `h2`/`http/1.1`);
  `mod.rs`: `serve`, `Server`, the task that pushes the deltas.
- `src/service/` (feature `server`, or the `wasm32` target): the map's service without its
  transport, shared by the web server and the browser build. `model.rs` (re-exported as
  `server::model`): the JSON views sent to the browser, SSE messages and deltas; `api.rs`: the
  `/api/*` answers and commands, and `handle` (a request without HTTP); `area.rs`: area and
  strategy changes; `mod.rs`: the task that publishes the deltas, and `Service` (the page's
  requests and event stream without a web server).
- `src/main.rs`: the `bitchat-geohash` binary (`required-features = ["server"]`): server
  options, start-up summary, `map: <url>` on stderr (read by `tests/cli.rs`), exit codes 2
  (command line) and 1 (start-up).
- `src/browser.rs` (`wasm32`): `Engine`, the browser build's entry point for `web/worker.js`.
  `web/`: `build.sh` (the static site in `target/web`: the server's page, plus `shim.js`, which
  hands its `/api/*` requests and event stream to `worker.js`, where the engine runs and keeps
  its snapshot in IndexedDB). The page itself is not copied: `src/server/assets/` is the only one.
- `examples/watch.rs`, `tests/`, `tests/data/`.

## Invariants (do not break these)

1. **No personal data leaves memory.** `CellStats`, the database schema, logs and anything
   sent to a UI hold counts only: no pubkey, no nickname, no message content. Pubkeys are kept
   in memory only for the aggregation window. Counts are shown at every level; an
   optional *k* threshold (`k_threshold`, default 0 = none) hides participant counts below *k*
   at level 6. A test asserts the schema has no forbidden column; extend it, and the snapshot
   tests, when you add fields.
2. **Read-only on relays.** Never publish an event, never answer NIP-42 AUTH (the app does
   not). `CLOSED auth-required:` marks the relay unusable, without retry.
3. **Mirror the app's relay choice** by default (`--strategy apps`). Another strategy
   (`--strategy airhop|a0|a1`, or the page's selector) makes the map listen where that
   strategy reads, for study: it never changes what the apps-strategy code below does. The 5
   relays closest to each channel's centre, with each
   app's own rule (`RelayDirectory::parse(csv, platform)`, `closest`): iOS keeps one entry per
   normalised host and sorts by (distance, host); Android keeps raw lines (`host` and
   `host:443` are two entries) and sorts by distance only, stable. Exactly 5, no tie beyond.
   iOS list and Android list both; one connection and one probe per server. An iOS list with a
   host at two coordinates is rejected as the app does. Dead relays are set aside,
   never replaced by the next one.
4. **Migrations are append-only.** One numbered file per change, applied in order inside a
   transaction, version in `PRAGMA user_version`. Never edit or reorder a committed migration;
   fix with a new one. Refuse to open a database newer than the code.
5. **Inactive channels stay retired.** A channel without any event for the inactivity delay
   (default 15 min) leaves the filters, and is reinstated only on explicit request. Every
   retirement and reinstatement is logged at INFO. The state survives a restart.
   Channels added by a descent survive a restart too (`origin = 'descent'`), unless
   `forget_descents`. Preview channels (`Monitor::preview`) are the exception: never
   saved nor retired, they leave the plan when the preview ends (`MonitorEvent::Removed`).
6. **Count each event once.** Deduplicate by event `id` across relays; reject bad signatures,
   kinds other than 20000/20001 and `g` tags outside the plan.
7. **Reproducible tests.** Tests use the frozen CSVs and an injected clock; they never
   download relay lists nor depend on wall-clock time.

## Web server

- The library must build and test without the `server` feature: keep server code under
  `src/server/` and `#[cfg(feature = "server")]`; server tests start with
  `#![cfg(feature = "server")]`.
- No CDN: every file the page loads is built in; a new asset gets a line in
  `assets.rs`. Only the map tiles come from outside.
- Page text, like logs, is in English. Strings from relays or relay lists go through
  `textContent` (`el()` in `app.js`), never `innerHTML`.
- HTTPS: make a certificate the browser trusts with `mkcert -install` then
  `mkcert localhost 127.0.0.1 ::1` (gives `localhost+2.pem` and `localhost+2-key.pem`), and pass
  them with `--tls-cert` / `--tls-key`. HTTP/2 then carries every SSE stream of the browser on
  one connection (HTTP/1.1 allows 6 per host). Never commit these files. A non-local address
  needs `--allow-remote` and TLS, or `--insecure-remote`, or a TLS proxy in front
  (`--behind-proxy`, `docs/behind-proxy.md`): then only the proxy's addresses, with
  HTTPS upstream and the public name, are served.
- A hidden tab closes its SSE stream after 10 s and reopens it when shown: over
  HTTP/1.1 open streams would otherwise use up the 6 connections per host, and a new tab
  could not load at all. Keep it when touching `connect()`.
- Tiles must be requested with a `Referer` (OSM tile policy): the tile layer sets
  `referrerPolicy: "strict-origin-when-cross-origin"`, overriding the page's `no-referrer`.
  Without it Firefox gets 403 "Access blocked".
- Hovering a watched cell previews the level below (`POST /api/preview`): the page
  draws those channels inside the cell with `interactive: false`, so the hover stays on the
  cell. Going up a level (list or URL) refits the map to the coarser cells in view.
- Map layers: relay pins (`.pin`, colour = status, square = one app, faded = not
  listened to) on a ring when they share coordinates (`ringed`, `ringAnchors` in `view.js`);
  meshes (`MeshView`, levels `Params.mesh_levels`) replace the cells when shown. Both are
  updated in place, so an open tooltip or popup survives a delta.
- On the map, every rectangle has a fill, transparent when there is no colour: in SVG the
  inside of an unfilled shape takes no hover nor click.
- Every route goes through `guard.rs`: `Host` checked (DNS rebinding), `POST` only with JSON and
  the server's own `Origin`, CSP. Keep it that way for new routes; a refusal is logged.
- What the browser receives obeys invariant 1: counts only. Everything goes through `service::model`,
  built field by field from the library types (no `serde` on them); a new field gets its
  line in the privacy test (`tests/model.rs`). With a k threshold set, a hidden participant count is `"low"`.

## BitChat protocol gotchas

- A geohash is a channel, not a subset of its parent: an event of `u09tv` carries only
  `["g","u09tv"]`. Each level is subscribed separately, with its own relays.
- App channel levels are 2, 4, 5, 6, 7. Level 3 is only an aggregation mesh. Level 7 is off by
  default.
- Kinds 20000 (message) and 20001 (presence) are ephemeral: relays forward them live and
  mostly do not store them. `since` (1 h) brings back little.
- The app sends presence heartbeats only at precisions 2, 4 and 5: at level 6 a participant
  is seen only when they write.
- Heartbeats go only to the channels of the device's real (GPS) location, never to a channel
  the user teleported to: a teleported user is counted only when they write. iOS 1.8.0:
  `GeohashPresenceService` (every 40–80 s, app in the foreground, Tor ready) sends to
  `LocationStateManager.availableChannels`, computed from GPS only. Presence and messages in
  a channel are signed by the same per-geohash key (`deriveIdentity(forGeohash:)`), so one
  person counts once.
- iOS reads bitchat's reviewed `online_relays_gps.csv`, Android georelays' daily
  `nostr_relays.csv`. The two choices drift apart: always compute both.
- Relay coordinates come from IP geolocation: many relays share the same point, hence ties.
  iOS breaks ties by host, Android keeps CSV order. Android does not normalise: it can pick
  the same server twice (`host` and `host:443`), so `Selected.android` is a list of ranks.
  Reference: geohash-relays' [relay-selection-corrections.md](https://github.com/wvmscs/geohash-relays/blob/strategies/docs/relay-selection-corrections.md).

## NIP-01 module and test helpers

- No Nostr library: `src/nostr.rs` holds the NIP-01 subset the crate needs. It verifies (`k256` BIP-340 `verify_raw` over the 32-byte id) but never signs:
  signing lives in the tests only.
- `Event::verify()` checks the id against the content, then the signature;
  `Event::tag_value("g")` reads the first `g` tag. `Filter::tags` maps `'g'` to geohashes,
  `Filter::to_json()` builds the REQ filter, `Filter::matches()` serves the test relay.
- Tests: `TestRelay::spawn("127.0.0.1:0")` is a real WebSocket NIP-01 relay (`url()`,
  `addr()`, `publish(event)`, `shutdown()`). Like real relays it forwards kinds 20000–29999
  live without storing them: publish after the listener is connected.
- Tests: `Identity::generate()` gives deterministic keys (no random source);
  `Event::new_unsigned(...).sign(&who)` comes from the `Sign` trait of `tests/common/`.

## Real-network checks

- Read-only observation (`watch`) is always fine. Never publish to public relays or real
  channels without asking (no bot there).
- Closed loop: a relay list with only the user's own test relay (`$BITCHAT_TEST_RELAY`), then a kind 20000 event published there with `nak event -k 20000
  -t g=<geohash> --sec $(nak key generate)`: a throwaway key, never a real identity.
- A real-app check needs the user: they can teleport their BitChat app to any geohash and post.

## Tests

- Unit tests cover pure logic (directory, selection, enumeration, plan, diff, aggregation,
  migrations). Listener and monitor tests use the local `TestRelay` and an injected clock.
- Add a test with each behavior change; for privacy changes add a negative test (what must
  *not* appear in a snapshot, a log line or the database).
- Timing tests (plan of `u09` depth 2 under one second) are `#[ignore]` and run in `--release`.

## Style

- Code, comments, rustdoc and documentation in English.
- `thiserror` error type in the library (`Error`), so callers can match.
- Keep the public API FFI-friendly (a Swift UI may come): owned types, strings, no lifetimes in
  signatures.
- `tracing` for logs, never `println!` in library code (examples may print).
- SQLite access goes through one dedicated writer thread; never block a tokio task on it.
- Do not commit `target/`, `.env` or database files (see `.gitignore`).
