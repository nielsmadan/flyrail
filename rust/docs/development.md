# Rust development

[Tool setup](#tool-setup) · [Commands](#commands) · [Interop and runtime prerequisites](#interop-and-runtime-prerequisites) · [CI and maintenance](#ci-and-maintenance)

Run commands from `rust/`. The repository-root Justfile delegates to the same
checks. Use a stable Rust toolchain with Rustfmt and Clippy; the workspace uses
the Rust 2024 edition. `just` is optional.

Cargo uses repository-root `.cache/cargo-home` through the Rust Justfile. Keep
downloaded tools, temporary installations, logs, and generated reports under
ignored cache directories. Tests use explicit temporary roots and must not touch
real agent configuration.

## Commands

| Command | Work performed |
| --- | --- |
| `just check` | Rustfmt check, Clippy with all targets/features, and workspace library tests. |
| `just check-integration` | The fast gate, Python differential preflight, and the complete workspace suite. |
| `just check-examples` | End-to-end tests for both example host CLIs. |
| `just check-package` | Archive listing plus extracted root/example package builds and CLI probes. |
| `just package-list` | List the root and both example crate archives. |
| `just format` | Format the workspace. |
| `just lint` | Run Clippy with warnings denied. |
| `just test` | Run the complete workspace test suite. |

The repository root also provides `just check`, `just check-integration`, and
`just check-package` across both implementations. Git hooks run the fast gate;
the complete integration suite belongs in CI and before accepting a release.

## Interop and runtime prerequisites

`just check-integration` requires the locked Python environment at
`../python/.venv/bin/python` and verifies imports before running Rust. Provision it
from the repository root with:

```sh
python3 python/scripts/check.py sync
```

The differential suite compares typed values, bundle/render digests, document
edits, and lifecycle observations against the Python implementation. The
operational interop suite runs both implementations against the same private
state and verifies cross-language inspect/update/remove, lock exclusion, recovery,
and unknown-state refusal. The Python `scripts/interop_driver.py` is covered by
the existing Ruff and strict-mypy `scripts` include; it also runs through the Rust
interop integration test. No separate lint recipe is needed.

Hook runtime tests require Node 24+, npm, and Bun 1.4.2+ locally. If unavailable,
they print a skip notice outside CI. CI acceptance uses exactly Node **24.21.0**,
npm **11.19.0**, and Bun **1.4.2**, and the tests fail rather than skip when any
prerequisite or shared fixture is unavailable.

## CI and maintenance

[CI](../../.github/workflows/ci.yml) runs Rust checks on Ubuntu 24.04 and macOS 15.
Acceptance jobs provision the pinned Python, Node/npm, and Bun prerequisites.
Integration coverage runs the shared fixtures, Python/Rust differential and
operational interop pairs, packaged hook runtime, examples, and native security
tests on both supported operating systems.

Workflow actions are commit-pinned. [Dependabot](../../.github/dependabot.yml)
groups weekly Cargo updates under `/rust`, Python updates under `/python`, and
GitHub Actions updates separately. The root [lefthook](../../lefthook.yml) runs
the shared fast gate and Rustfmt/Clippy checks without installing or activating
anything itself.

The package QA script creates archives and vendored consumers below ignored cache
directories. It does not publish a crate. This repository has no Rust publishing
automation.
