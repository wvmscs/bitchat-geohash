# AGENTS.md

Guidance for coding agents (and humans) working in this crate.

## What this is

`bitchat-geohash`: live monitor of BitChat location channels (geohash). It enumerates the
channels inside a geohash, computes the relays the iOS and Android apps use for each one,
listens to kinds 20000/20001 there, and aggregates activity per cell, without personal data.
The spec is [SPECIFICATIONS.md](SPECIFICATIONS.md); step 1 progress is tracked in
[PLAN_ET1.md](PLAN_ET1.md).

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
- Add dependencies without discussion. Approved: those listed in lot 0 of `PLAN_ET1.md`.
- Remove existing functionality.

### DO
- Read relevant files before making changes.
- Follow existing naming conventions.
- Add comments for complex logic.
- Update documentation when adding features.
- Keep `PLAN_ET1.md` up to date: tick the boxes, update the dashboard, add a dated line to
  the journal, record decisions in the decisions table.
- Before each commit, run the commands listed under Commands.

### Others
- Pushing commits and tags to the remote (`origin`), once one is configured, is authorized.
  Never force-push or rewrite pushed history without an explicit request.

## Workflow

1. Always read the file you are about to modify.
2. Check existing tests before changing code.
3. Check the existing API before writing code, in this crate and in `bitchat-envelope`
   (`Event`, `Filter`, `envelope::unix_now`, mocks). Be DRY.
4. Run the commands listed under Commands.
5. Update tests for new functionality.
6. Update `PLAN_ET1.md`.
7. Keep commits focused and atomic, scoped to `bitchat-geohash/`.

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
4. Documentation updated (`SPECIFICATIONS.md`, `PLAN_ET1.md`, rustdoc).
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
- [ ] Integration tests pass (local `WsRelay`)
- [ ] Manual testing completed (`examples/watch.rs` on real relays)

## Screenshots (if UI changes)

 ![Screenshot](path/to/screenshot)
```

## Commands

Run from the workspace root:

```sh
cargo fmt -p bitchat-geohash --check
cargo test -p bitchat-geohash
cargo clippy -p bitchat-geohash --all-targets --all-features -- -D warnings   # must be 0 warnings
cargo test -p bitchat-geohash --release -- --ignored                         # timing tests (plan build)
cargo run -p bitchat-geohash --example watch -- u09 --depth 2                # real network
cargo run -p bitchat-geohash --example watch -- u09tvw --depth 0 --no-android \
    --relays-csv own.csv --db own.sqlite --refresh 1s     # one relay only (own.csv lists it)
```

Tests run offline: the frozen CSVs from `tests/data/` and the local `WsRelay` of
`bitchat-envelope` (feature `mock-ws`, dev-dependency). Only the `watch` example touches the
real network.

## Layout

Planned for step 1 (see `PLAN_ET1.md` for the details and current state):

- `src/directory.rs`: `RelayDirectory`, CSV parsing and normalisation, list sources.
- `src/select.rs`: relay choice per channel (`closest`, `select_both`), from `geo.rs`.
- `src/channels.rs`: `Level`, channel enumeration.
- `src/plan.rs`: `ListenPlan` (relay → channels), filters, retire/reinstate, diff.
- `src/store/`: SQLite `Store` and `migrations/NNNN_name.sql`.
- `src/probe.rs`: relay probe, from `probe.rs`.
- `src/aggregate.rs`: deduplication, window, inactivity, `CellStats`.
- `src/listener.rs`: one tokio task per relay.
- `src/monitor.rs`: wiring, control loop, 24 h reload.
- `examples/watch.rs`, `tests/`, `tests/data/`, `swift/`.

## Invariants (do not break these)

1. **No personal data leaves memory.** `CellStats`, the database schema, logs and anything
   sent to a UI hold counts only: no pubkey, no nickname, no message content. Pubkeys are kept
   in memory only for the aggregation window. Below *k* participants at level 6, the count is
   hidden. A test asserts the schema has no forbidden column; extend it, and the snapshot
   tests, when you add fields.
2. **Read-only on relays.** Never publish an event, never answer NIP-42 AUTH (the app does
   not). `CLOSED auth-required:` marks the relay unusable, without retry.
3. **Mirror the app's relay choice.** The 5 relays closest to each channel's centre, ties for
   the last slot included, normalised like the app, iOS list and Android list both. Dead
   relays are set aside, never replaced by the next one.
4. **Migrations are append-only.** One numbered file per change, applied in order inside a
   transaction, version in `PRAGMA user_version`. Never edit or reorder a committed migration;
   fix with a new one. Refuse to open a database newer than the code.
5. **Inactive channels stay retired.** A channel without any event for the inactivity delay
   (default 5 min) leaves the filters, and is reinstated only on explicit request. Every
   retirement and reinstatement is logged at INFO. The state survives a restart.
6. **Count each event once.** Deduplicate by event `id` across relays; reject bad signatures,
   kinds other than 20000/20001 and `g` tags outside the plan.
7. **Reproducible tests.** Tests use the frozen CSVs and an injected clock; they never
   download relay lists nor depend on wall-clock time.

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
  iOS breaks ties by host, Android keeps CSV order.

## bitchat-envelope gotchas

- Edition 2021, MSRV 1.75; this crate is edition 2024. Do not change `bitchat-envelope`.
- `Event::new_unsigned(...)` then `.sign(&identity)`; `Event::verify()` checks id and
  Schnorr signature; `Event::tag_value("g")` reads the first `g` tag.
- `Filter::tags` is a `BTreeMap<char, Vec<String>>` (`'g'` → geohashes); `Filter::to_json()`
  builds the REQ filter.
- `MockBitchat` models private messages only. Build geohash events (kinds 20000/20001,
  `g` tag) by hand with `Event` and an `Identity`.
- `MockRelay` is sync and thread-safe; `mock::ws::WsRelay::spawn("127.0.0.1:0", relay)` gives
  a real WebSocket relay for listener tests.

## Real-network checks

- Read-only observation (`watch`) is always fine. Never publish to public relays or real
  channels without asking (no `geohash_bot` there).
- Closed loop: a relay list with only `wss://relay.famillevital.club` (the user's own relay,
  usable for tests), then a kind 20000 event published there with `nak event -k 20000
  -t g=<geohash> --sec $(nak key generate)`: a throwaway key, never a real identity.
- A real-app check needs the user: they can teleport their BitChat app to any geohash and post.

## Tests

- Unit tests cover pure logic (directory, selection, enumeration, plan, diff, aggregation,
  migrations). Listener and monitor tests use a local `WsRelay` and an injected clock.
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
