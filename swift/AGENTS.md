# Flyrail for Swift

Flyrail is a synchronous macOS 13+ library. Keep public values immutable and
`Sendable`, preserve persisted identities as exact bytes, and keep library calls
free of printing, prompting, process execution, network access, and implicit
agent selection.

Use `just check` for formatting, lint, warnings-as-errors build, unit tests, and
the value differential. SwiftPM commands run through the root package and keep
build outputs under the repository `.cache`. Set
`FLYRAIL_SWIFT_DISABLE_SANDBOX=1` under nono.

Tests use checkout-local fixtures and temporary roots. Do not read or write real
agent configuration. Shared conformance fixtures live in `../spec/fixtures`.
Prefer immutable transformations, clear names, and terse caller-facing DocC.
