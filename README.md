# Flyrail

Libraries for applications that ship coding-agent configuration. Flyrail manages
skills, instructions, MCP registrations, hooks and supporting assets through
independently versioned bundles and reversible local resource ownership.

Each language implementation lives in its own directory and shares the bundle,
receipt and transaction formats in `spec/`.

| Implementation | Status | Documentation |
| --- | --- | --- |
| Python | Alpha; Python 3.11+ | [API and quickstart](python/README.md) |
| Rust | Alpha; Rust stable | [API and quickstart](rust/README.md) |

Additional Swift, Go and TypeScript implementations can use the same contracts
and conformance fixtures. Their public APIs should be native to each language.

## Repository

- `python/`: package, tests, examples, documentation and Python tooling.
- `rust/`: Rust crate and tooling.
- `spec/`: language-independent formats, recovery protocol and shared fixtures.
- `.github/`: repository CI and dependency maintenance.

Application versions and bundle labels are independent. Flyrail compares bundle
labels for equality and checks content separately. Local edits are preserved
unless the host explicitly authorizes replacement. Unowned content conflicts by
default; explicit takeover records a baseline for restoration. Resource commits
are independent, and source-free removal retains unresolved ownership.

See the [shared specifications](spec/README.md) and
[implementation guidance](spec/porting.md) for ownership, interoperability and
filesystem limits.

The combined checksum examples for
[Python](python/examples/packaged/README.md) and
[Rust](rust/examples/packaged/README.md) ship all four families through real
agent presets. Their status compares the bundled generation with installed
content and reports host prerequisites and activation separately.

## Development

`just setup` synchronizes the locked environment and installs the Git hooks.
The root check commands run both implementations:

```sh
just check
just check-integration
just check-package
just audit-dependencies
just check-workflow
```

Install the published Python distribution with `python -m pip install pyflyrail`;
the import package is `flyrail`.

Without just, run `python3 python/scripts/check.py check` from this directory.
See [Python development](python/docs/development.md) and
[Rust development](rust/docs/development.md) for tool setup and the individual
build/test commands. Generated logs, test installations, reports and build
outputs stay in ignored local directories.

Licensed under the [MIT License](LICENSE).
