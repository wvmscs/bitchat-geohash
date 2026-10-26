# Changelog

## [Unreleased]

### Added
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
- Default inactivity delay before a channel is retired: 15 min (was 5 min).
- `normalize_url` keeps an explicit `ws://` (local test relays).
- Swift package moved to `swift/`.
