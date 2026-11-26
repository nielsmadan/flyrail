# Changelog

All notable user-facing changes to Flyrail for Rust. Package versions are independent from
configuration bundle labels and shared format schema versions.

## [0.1.0] - 2026-11-26


### Features

- Add full Rust implementation
- Add Rust feature parity
- Port the state store to Rust and stage transactions in the store
- Add Swift implementation
- Add Rust and Swift release automation
- Add the Go implementation
- Add global loadout support


### Bug Fixes

- Satisfy clippy under rust 1.99
- Detect replaced recovery revisions
- Support sha2 upgrades in checksum example
- Install rustfmt and clippy with the pinned toolchain
- Pass a state store to the Rust example package checks
- Fetch locked crates before vendoring in Rust package QA

[0.1.0]: https://github.com/nielsmadan/flyrail/tree/rust-v0.1.0

