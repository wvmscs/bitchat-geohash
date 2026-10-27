# AGENTS.md

Guidance for coding agents (and humans) working in this crate.

## What this is

`bitchat-geohash`: live monitor of BitChat location channels (geohash). It enumerates the
channels inside a geohash, computes the relays the iOS and Android apps use for each one,
listens to kinds 20000/20001 there, and aggregates activity per cell, without personal data.
The spec is [SPECIFICATIONS.md](SPECIFICATIONS.md); progress is tracked in
[PLAN_ET1.md](PLAN_ET1.md) (step 1, done) and [PLAN_ET2.md](PLAN_ET2.md) (step 2).

Step 1 is the Rust library. Step 2 adds an internal web server in Rust (axum), not Node.js.
Step 3 adds a Swift UI through `uniffi`; the Swift package lives in `swift/`.

## Constraints

### Do NOT
- Modify files outside `bitchat-geohash/`. `bitchat-envelope`, `nip90-dvm`, the workspace
  `Cargo.toml` and `src/` are read-only. If a change there looks necessary, ask first.
- Modify files without reading them first.
- Modify existing tests, except to fix them or when an instruction in this file requires it
  (for example, extending the privacy tests when adding fields).
- Edit or regenerate `tests/data/*.csv`: they are frozen for reproducibility.
- Edit a migration that has been committed (see invariant 4).
- Add dependencies without discussion. Approved: those listed in lot 0 of `PLAN_ET1.md` and
  lot 10 of `PLAN_ET2.md`.
- Edit `src/server/assets/leaflet/`: Leaflet 1.9.4 as released (a test checks its SHA-256).
- Remove existing functionality.

### DO
- Read relevant files before making changes.
- Follow existing naming conventions.
- Add comments for complex logic.
- Update documentation when adding features.
- Keep the plan of the current step (`PLAN_ET2.md`) up to date: tick the boxes, update the
  dashboard, add a dated line to the journal, record decisions in the decisions table.
- Before each commit, run the commands listed under Commands.

### Others
- This crate is its own git repository (`origin` = `git@github.com:wvmscs/bitchat-geohash.git`,
  branch `main`), nested in the local `Nostr/` Cargo workspace, which is not under git.
- Pushing commits and tags to `origin` is authorized.
  Never force-push or rewrite pushed history without an explicit request.

## Workflow

1. Always read the file you are about to modify.
2. Check existing tests before changing code.
3. Check the existing API before writing code (`nostr::{Event, Filter, unix_now}`,
   `tests/common/`). Be DRY.
4. Run the commands listed under Commands.
5. Update tests for new functionality.
6. Update `PLAN_ET2.md`.
7. Keep commits focused and atomic.

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
4. Documentation updated (`SPECIFICATIONS.md`, `PLAN_ET2.md`, rustdoc).
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

Run from the crate (a standalone package) or from the local `Nostr/` workspace root:

```sh
cargo fmt -p bitchat-geohash --check
cargo test -p bitchat-geohash
cargo test -p bitchat-geohash --no-default-features                          # library without the server
cargo clippy -p bitchat-geohash --all-targets --all-features -- -D warnings   # must be 0 warnings
cargo clippy -p bitchat-geohash --all-targets --no-default-features -- -D warnings
cargo test -p bitchat-geohash --release -- --ignored                         # timing tests (plan build)
cargo run -p bitchat-geohash --example watch -- u09 --depth 2                # real network
cargo run -p bitchat-geohash --example watch -- u09tvw --depth 0 --no-android \
    --relays-csv own.csv --db own.sqlite --refresh 1s     # one relay only (own.csv lists it)
cargo run -p bitchat-geohash -- --listen 127.0.0.1:8080                     # map server
```

`Cargo.lock` is committed for reproducible standalone builds. Inside the local workspace,
Cargo uses `Nostr/Cargo.lock` instead and ignores it: after a dependency change, refresh it
from a copy of the crate outside the workspace (copy the workspace lock there, run
`cargo tree` to prune it, check with `cargo test --locked`), so both pin the same versions.

Tests run offline: the frozen CSVs from `tests/data/` and the local `TestRelay`
(`tests/common/relay.rs`). Only the `watch` example and the binary touch the real network
(the browser also loads map tiles).

## Layout

Step 1 (see `PLAN_ET1.md`), then step 2 (`PLAN_ET2.md`):

- `src/nostr.rs`: NIP-01 subset: `Event` (id, Schnorr verification), `Filter`, `unix_now`.
- `src/directory.rs`: `RelayDirectory`, CSV parsing with each app's rules, normalisation,
  iOS conflict check, list sources.
- `src/select.rs`: relay choice per channel (`closest`, `select_both`), from `geo.rs`.
- `src/channels.rs`: `Level`, channel enumeration.
- `src/plan.rs`: `ListenPlan` (relay → channels), filters, retire/reinstate, diff.
- `src/store/`: SQLite `Store` and `migrations/NNNN_name.sql`.
- `src/probe.rs`: relay probe, from `probe.rs`.
- `src/aggregate.rs`: deduplication, window, inactivity, `CellStats`.
- `src/listener.rs`: one tokio task per relay.
- `src/monitor.rs`: wiring, control loop, 24 h reload; descents (`descend`, `watch_channels`),
  `relays()`, change notifications (`subscribe`).
- `src/server/` (feature `server`, on by default): axum router; `assets.rs` lists the files
  built into the binary from `src/server/assets/` (page, `app.js`, `app.css`, Leaflet).
  `model.rs`: the JSON views sent to the browser, SSE messages and deltas.
- `src/main.rs`: the `bitchat-geohash` binary (`required-features = ["server"]`).
- `examples/watch.rs`, `tests/`, `tests/data/`, `swift/`.

## Invariants (do not break these)

1. **No personal data leaves memory.** `CellStats`, the database schema, logs and anything
   sent to a UI hold counts only: no pubkey, no nickname, no message content. Pubkeys are kept
   in memory only for the aggregation window. Counts are shown at every level (D20); an
   optional *k* threshold (`k_threshold`, default 0 = none) hides participant counts below *k*
   at level 6. A test asserts the schema has no forbidden column; extend it, and the snapshot
   tests, when you add fields.
2. **Read-only on relays.** Never publish an event, never answer NIP-42 AUTH (the app does
   not). `CLOSED auth-required:` marks the relay unusable, without retry.
3. **Mirror the app's relay choice.** The 5 relays closest to each channel's centre, with each
   app's own rule (`RelayDirectory::parse(csv, platform)`, `closest`): iOS keeps one entry per
   normalised host and sorts by (distance, host); Android keeps raw lines (`host` and
   `host:443` are two entries) and sorts by distance only, stable. Exactly 5, no tie beyond.
   iOS list and Android list both; one connection and one probe per server. An iOS list with a
   host at two coordinates is rejected as the app does (D18). Dead relays are set aside,
   never replaced by the next one.
4. **Migrations are append-only.** One numbered file per change, applied in order inside a
   transaction, version in `PRAGMA user_version`. Never edit or reorder a committed migration;
   fix with a new one. Refuse to open a database newer than the code.
5. **Inactive channels stay retired.** A channel without any event for the inactivity delay
   (default 15 min) leaves the filters, and is reinstated only on explicit request. Every
   retirement and reinstatement is logged at INFO. The state survives a restart.
   Channels added by a descent survive a restart too (`origin = 'descent'`), unless
   `forget_descents`.
6. **Count each event once.** Deduplicate by event `id` across relays; reject bad signatures,
   kinds other than 20000/20001 and `g` tags outside the plan.
7. **Reproducible tests.** Tests use the frozen CSVs and an injected clock; they never
   download relay lists nor depend on wall-clock time.

## Web server

- The library must build and test without the `server` feature: keep server code under
  `src/server/` and `#[cfg(feature = "server")]`; server tests start with
  `#![cfg(feature = "server")]`.
- No CDN: every file the page loads is built in (D10); a new asset gets a line in
  `assets.rs`. Only the map tiles come from outside.
- Page text, like logs, is in English. Strings from relays or relay lists go through
  `textContent` (`el()` in `app.js`), never `innerHTML`.
- What the browser receives obeys invariant 1: counts only. Everything goes through `server::model`,
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
- iOS reads bitchat's reviewed `online_relays_gps.csv`, Android georelays' daily
  `nostr_relays.csv`. The two choices drift apart: always compute both.
- Relay coordinates come from IP geolocation: many relays share the same point, hence ties.
  iOS breaks ties by host, Android keeps CSV order. Android does not normalise: it can pick
  the same server twice (`host` and `host:443`), so `Selected.android` is a list of ranks.
  Reference: `bitchat-envelope/Documents/relay-selection-corrections.md`.

## NIP-01 module and test helpers

- The crate has no dependency on `bitchat-envelope`: `src/nostr.rs` holds the NIP-01 subset it
  needs. It verifies (`k256` BIP-340 `verify_raw` over the 32-byte id) but never signs:
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
  channels without asking (no `geohash_bot` there).
- Closed loop: a relay list with only `wss://relay.famillevital.club` (the user's own relay,
  usable for tests), then a kind 20000 event published there with `nak event -k 20000
  -t g=<geohash> --sec $(nak key generate)`: a throwaway key, never a real identity.
- A real-app check needs the user: they can teleport their BitChat app to any geohash and post.

## Tests

- Unit tests cover pure logic (directory, selection, enumeration, plan, diff, aggregation,
  migrations). Listener and monitor tests use the local `TestRelay` and an injected clock.
- Add a test with each behavior change; for privacy changes add a negative test (what must
  *not* appear in a snapshot, a log line or the database).
- Timing tests (plan of `u09` depth 2 under one second) are `#[ignore]` and run in `--release`.

## Style

- Code, comments and rustdoc in English; `SPECIFICATIONS.md` and `PLAN_ET1.md` in French.
- `thiserror` error type in the library (`Error`), so callers (and the FFI later) can match.
- Keep the public API FFI-friendly: owned types, strings, no lifetimes in signatures.
- `tracing` for logs, never `println!` in library code (examples may print).
- SQLite access goes through one dedicated writer thread; never block a tokio task on it.
- Do not commit `target/`, `swift/.build/`, `.env` or database files (see `.gitignore`).
