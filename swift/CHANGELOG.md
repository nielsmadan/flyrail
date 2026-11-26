# Changelog

All notable user-facing changes to Flyrail for Swift. Package versions are independent from
configuration bundle labels and shared format schema versions.

## [0.2.0] - 2026-11-26


### Features

- Add the Go implementation
- Add global loadout support


### Bug Fixes

- Satisfy swiftformat andOperator in loadout override guard (#30)
- Expect swift-system 1.9.0 in Swift package QA pins (#31)
- Check swift package pins against manifest ranges

## [0.1.0] - 2026-11-05


### Features

- Add Swift implementation
- Keep Swift ownership state in an app-supplied state store and gate CI on Swift
- Add Rust and Swift release automation


### Bug Fixes

- Close Swift state store parity gaps found in review
- Build Swift with Swift 6.3
- Feed the corpus-limit FIFO as fast as the driver drains it

[0.2.0]: https://github.com/nielsmadan/flyrail/compare/v0.1.0..v0.2.0
[0.1.0]: https://github.com/nielsmadan/flyrail/tree/v0.1.0

