# Flyrail

Libraries for applications that ship coding-agent skills. Flyrail installs bundled
skills into explicit agent directories, checks independently versioned bundles,
updates owned content, and removes it without needing the original source.

Each language implementation lives in its own directory and shares the bundle,
receipt and transaction formats in `spec/`.

| Implementation | Status | Documentation |
| --- | --- | --- |
| Python | Prototype; Python 3.11+ | [API and quickstart](python/README.md) |

Swift, Rust, Go and TypeScript implementations can use the same contracts and
conformance fixtures. Their public APIs should be native to each language.

## Repository

- `python/`: package, tests, examples, documentation and Python tooling.
- `spec/`: language-independent formats, recovery protocol and shared fixtures.
- `.github/`: repository CI and dependency maintenance.

Application versions and bundle labels are independent. Flyrail compares bundle
labels for equality and checks content separately. Local edits are preserved
unless the host explicitly authorizes replacement; foreign and untracked skills
remain conflicts. Installation results are independent per destination.

See the [shared specifications](spec/README.md) and
[implementation guidance](spec/porting.md) for ownership, interoperability and
filesystem limits.

## Development

The root commands currently run the Python implementation:

```sh
just check
just dependency-audit
just workflow-check
```

Without just, run `python3 python/scripts/check.py check` from this directory
(Windows: `python`). See [Python development](python/docs/development.md) for tool
setup and the individual build/test commands. Generated logs, test installations,
reports and build outputs stay in ignored local directories.

Licensed under the [MIT License](LICENSE).
