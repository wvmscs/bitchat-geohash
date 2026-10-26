# Changelog

## [Unreleased]

### Added
- Crate skeleton: modules of step 1, `Error` type, protocol constants.
- `directory`: relay list parsing (`RelayDirectory`, `Platform`, `normalize_url`).
- `select`: the apps' relay choice per channel (`closest`, `select_both`), ties for the last slot included.
- `channels`: channel levels (`Level`), validation, enumeration with a cap (`enumerate`, `count`).
- `plan`: listen plan (`ListenPlan`): relay → channels, `#g` subscriptions in stable groups, retire/reinstate, dead relays, diff between plans.
- `store`: SQLite store with append-only, versioned migrations (`0001_init`), writer thread (`StoreWriter`), relay list fallback, channel states, per-minute cell counters.
- Frozen relay lists in `tests/data/` (iOS `online_relays_gps.csv`, Android `nostr_relays.csv`).

### Changed
- Swift package moved to `swift/`.
