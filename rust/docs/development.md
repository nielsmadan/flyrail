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
edits, lifecycle observations and normalized store records against the Python
implementation. The two implementations never share a state store. The
operational interop suite runs each lifecycle scenario once per implementation,
each with its own `DirectoryStore`, and compares every step's result, destination
bytes and modes, and store records after normalizing transaction ids, node
identities and the case root. It also has a Python application and a Rust
application share one instruction file through separate stores, and covers codec
round trips, store lock exclusion and unknown-state refusal. The Python `scripts/interop_driver.py` is covered by
the existing Ruff and strict-mypy `scripts` include; it also runs through the Rust
interop integration test. No separate lint recipe is needed.

Hook runtime tests require Node 24+, npm, and Bun 1.4.2+ locally. If unavailable,
they print a skip notice outside CI. CI acceptance uses exactly Node **24.21.0**,
npm **11.19.0**, and Bun **1.4.2**, and the tests fail rather than skip when any
prerequisite or shared fixture is unavailable.

## CI and maintenance

[CI](../../.github/workflows/ci.yml) runs Rust checks on Ubuntu 24.04 and macOS 15
for changes under `rust/`, `python/` and shared paths; see the
[path selection table](../../python/docs/development.md#ci-and-maintenance).
Acceptance jobs provision the pinned Python, Node/npm, and Bun prerequisites.
Integration coverage runs the shared fixtures, Python/Rust differential and
operational interop pairs, packaged hook runtime, examples, and native security
tests on both supported operating systems.

Workflow actions are commit-pinned. [Dependabot](../../.github/dependabot.yml)
groups weekly Cargo updates under `/rust`, Python updates under `/python`, Swift
package updates at the root, and GitHub Actions updates separately. The root
[lefthook](../../lefthook.yml) runs the shared fast gate and Rustfmt/Clippy checks
without installing or activating anything itself.

The package QA script creates archives and vendored consumers below ignored cache
directories. It reads the crate version from `Cargo.toml` and never publishes.

## Releasing Rust

The `flyrail` crate is published to crates.io from
[`release-rust.yml`](../../.github/workflows/release-rust.yml). The example crates
set `publish = false`. Rust versions are independent of the Python and Swift
packages; tags are `rust-vX.Y.Z`. Releases use
[`curtains`](https://github.com/nielsmadan/curtaincall) from a clean, complete
`main` checkout at the repository root:

```sh
just release rust --dry-run
just release rust
just release rust patch
just release rust 0.2.0
```

Curtains considers commits touching `rust/` or `rust-toolchain.toml`, runs the
Rust `just check` and the workflow validation, then
[`prepare_release.py`](../scripts/prepare_release.py) sets the workspace version
and the examples' `flyrail` requirements, `just lock` refreshes `Cargo.lock`, and
git-cliff writes [`CHANGELOG.md`](../CHANGELOG.md). Package QA runs against the
prepared version before curtains commits, tags and atomically pushes `main` with
the tag.

The workflow validates the annotated tag and crate version, reruns formatting,
Clippy, the full test suite with the Python differential, package QA and
`cargo publish --dry-run` on Ubuntu, repeats Clippy and tests on macOS, and saves
the verified `.crate`. The publish job runs in the `crates-io` environment,
holds only `id-token: write`, skips an already published version, repackages the tagged source and requires it
to be byte-identical to the verified crate, publishes, and then checks that
crates.io serves the verified checksum. A `contents: write` job finally creates
the "Flyrail for Rust" GitHub release with the crate attached. Releases below 1.0
are marked as pre-releases.

Before the first release, create the `crates-io` environment under the
repository's **Settings → Environments** and limit its deployment branches and
tags to the tag pattern `rust-v*` and the `main` branch, which retries run from.
Other branches can then not reach the publish credentials. crates.io trusted publishing only works for a crate that already
exists, so the first release authenticates with a token:

1. On crates.io, create an API token limited to the `flyrail` crate name with the
   `publish-new` scope and a short expiry.
2. Store it as the `CARGO_REGISTRY_TOKEN` secret of the `crates-io` environment,
   not as a repository secret. The publish job uses that secret whenever it is set.
3. After the first version is published, delete the secret and the token, then
   add a trusted publisher on the crate's crates.io settings page: owner
   `nielsmadan`, repository `flyrail`, workflow `release-rust.yml`, environment
   `crates-io`. Later releases exchange the workflow's OIDC identity for a
   short-lived token through `rust-lang/crates-io-auth-action`.

If publication fails after the tag is pushed, fix the workflow on `main` and rerun
it from the Actions page with **Run workflow** on `main`, supplying the existing
tag, or with the GitHub CLI:

```sh
gh workflow run release-rust.yml --ref main -f tag=rust-v0.1.0
```

A version already on crates.io is not uploaded again. Never move a published tag.
