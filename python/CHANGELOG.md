# Changelog

All notable user-facing changes to Flyrail for Python. Package versions are independent from
configuration bundle labels and shared format schema versions.

## [0.4.0] - 2026-11-01


### Features

- Add full Rust implementation
- Add Rust feature parity
- Keep ownership state in an app-supplied state store
- Port the state store to Rust and stage transactions in the store
- Add Swift implementation
- Keep Swift ownership state in an app-supplied state store and gate CI on Swift


### Bug Fixes

- Bump urllib3 to 2.8.0
- Satisfy mypy on linux for xattr ignores
- Detect replaced recovery revisions
- Stop faking st_birthtime_ns on macOS stat results
- Bump certifi to 2026.10.19

## [0.3.0] - 2026-09-23


### Features

- Detect droid and refuse to translate for it

## [0.2.0] - 2026-09-22


### Features

- Report installed agents
- Summarize installation plans


### Bug Fixes

- Report every artifact and error a planned change covers

## [0.1.0] - 2026-09-20


### Features

- Python prototype
- Add reversible agent configuration bundles
- Add Python release process


### Bug Fixes

- Unblock Python release
- Initialize Windows lock files
- Separate Windows lock range
- Correct Windows filesystem semantics
- Ignore inherited git context in the release helper

[0.4.0]: https://github.com/nielsmadan/flyrail/compare/python-v0.3.1..python-v0.4.0
[0.3.0]: https://github.com/nielsmadan/flyrail/compare/python-v0.2.0..python-v0.3.0
[0.2.0]: https://github.com/nielsmadan/flyrail/compare/python-v0.1.0..python-v0.2.0
[0.1.0]: https://github.com/nielsmadan/flyrail/tree/python-v0.1.0

