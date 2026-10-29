## What and why

## Checks

- [ ] Rebased on the latest `main`, no merge commit
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
- [ ] `cargo deny check advisories licenses sources`, `reuse lint`
- [ ] Tests offline; no seed, key, private message or real pubkey in the diff
- [ ] CHANGELOG updated if the behaviour changes
