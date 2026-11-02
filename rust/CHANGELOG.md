# Changelog

All notable user-facing changes to Flyrail for Rust. Package versions are independent from
configuration bundle labels and shared format schema versions.

## [Unreleased]


### Features

- Add full Rust implementation
- Add Rust feature parity
- Port the state store to Rust and stage transactions in the store
- Add Swift implementation


### Bug Fixes

- Satisfy clippy under rust 1.99
- Detect replaced recovery revisions
- Support sha2 upgrades in checksum example
- Install rustfmt and clippy with the pinned toolchain
- Pass a state store to the Rust example package checks
- Fetch locked crates before vendoring in Rust package QA

[unreleased]: https://github.com/nielsmadan/flyrail/tree/HEAD

