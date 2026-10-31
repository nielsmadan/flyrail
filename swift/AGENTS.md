# Flyrail for Swift

Flyrail is a synchronous macOS 13+ library. Keep public values immutable and
`Sendable`, preserve persisted identities as exact bytes, and keep library calls
free of printing, prompting, process execution, network access, and implicit
agent selection.

Use `just check` for formatting, lint, warnings-as-errors build, unit tests, and
the value differential. SwiftPM commands run through the root package and keep
build outputs under the repository `.cache`. Set
`FLYRAIL_SWIFT_DISABLE_SANDBOX=1` under nono. CI pins SwiftFormat 0.64.0 and
SwiftLint 0.65.1; use the same versions locally. CI path selection is described
in `../python/docs/development.md` under "CI and maintenance".

`just check-integration` and `just check-package` run the cross-language scripts
and package QA; CI runs them nightly and on dispatch, and `CI result` requires
them. Tests build stores and targets with `NativeTestRoot.store` and `target()`.

State-store behaviour follows Rust (`../rust/src`) where Python and Rust differ.
Skill convenience functions report every failure, including a store that
overlaps a bundle source, as a failed per-target result instead of throwing.

Tests use checkout-local fixtures and temporary roots. Do not read or write real
agent configuration. Shared conformance fixtures live in `../spec/fixtures`.
Prefer immutable transformations, clear names, and terse caller-facing DocC.
