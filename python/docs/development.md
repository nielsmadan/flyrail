# Python development

[Tool setup](#tool-setup) · [Hook runtimes and shell probes](#hook-runtimes-and-shell-probes) · [Commands](#commands) · [Repository files and generated output](#repository-files-and-generated-output) · [CI and maintenance](#ci-and-maintenance) · [Releasing Python](#releasing-python)

Run the commands below from `python/`. The repository-root Justfile forwards the
same commands here. Python 3.11+ and uv 0.12.13+ are required; just is optional.
The sole runtime dependency is `tomlkit`, loaded lazily for TOML editing.
Build, development and audit dependencies are pinned in `pyproject.toml` and resolved with artifact hashes in `uv.lock`.

## Tool setup

The check script selects uv from `FLYRAIL_UV`, then the repository-local
`.cache/tools/uv/uv` (`uv.exe` on Windows), then PATH. CI uses uv **0.12.13**.
For a local installation, download the matching OS/architecture archive from the
[uv release](https://github.com/astral-sh/uv/releases/tag/0.12.13), verify its
published checksum and extract the executable to that cache directory.

Workflow validation requires **actionlint 1.7.12**. Download the matching archive
and checksum file from the
[actionlint release](https://github.com/rhysd/actionlint/releases/tag/v1.7.12),
verify the checksum and extract the executable into repository-root
`.cache/tools/actionlint`. The script checks `FLYRAIL_ACTIONLINT`, that directory,
then PATH, and requires the exact version. [CI](../../.github/workflows/ci.yml)
contains the Linux download and checksum command.

## Hook runtimes and shell probes

Hook development checks require Node 24+, npm and Bun 1.4.2+. `just check-integration`
installs the exact `runtime-tooling/package-lock.json` development graph under
repository `.cache/hook-tooling`, audits it, and checks TypeScript strictly before
running the real child-process/host-loader tests. These are development and later
hook-execution prerequisites; ordinary Python package builds do not invoke them.
`just check` does not need them, so a registry outage cannot block a commit.

CI enables `FLYRAIL_TEST_CODEX_LOGIN_SHELL=1` so POSIX Codex bridge probes execute
its documented default `/bin/sh -lc` argument on clean runner profiles. Local
checks normally use `/bin/sh -c` and report POSIX non-login execution explicitly.
To include the default login shell locally, run in an environment whose startup
files are accessible (for example, an unsandboxed terminal):

```sh
FLYRAIL_TEST_CODEX_LOGIN_SHELL=1 just check
```

This does not imply Codex offers arbitrary shell arguments. It selects the test
probe, not host configuration. Keep strict stderr checks; do not alter HOME or
startup files to make a probe pass. Windows probes use native `cmd.exe /C` and
direct execution. Native Windows security/process checks and Linux behavior must
run on those platforms; configured mypy platforms are only static validation:

```sh
uv run --no-sync mypy --platform win32
uv run --no-sync mypy --platform linux
```

## Commands

| Command | Work performed |
| --- | --- |
| `just check` | Locked sync, Ruff format/lint/security checks, interpreter verification, strict mypy, and the unit test tier. Runs in seconds; this is the git-hook and local release gate. |
| `just check-integration` | Hook runtime checks, the complete test suite with branch coverage and the 95% gate, and package/consumer checks. Needs Node, npm and Bun. |
| `just audit-dependencies` | Audit every locked registry package/version with pip-audit. |
| `just check-workflow` | Validate all repository workflows with actionlint. |
| `just check-package` | Build and verify distributions and installed example applications. |
| `just build` | Build the wheel and source archive into `python/dist/`. |
| `just format` | Format Python code with Ruff. |
| `just sync` | Synchronize `python/.venv` to the locked environment. |
| `just lock` | Regenerate `uv.lock` after intentional dependency changes. |

Each command is also available as `python scripts/check.py ACTION` from `python/`,
or `python python/scripts/check.py ACTION` from the repository root. Use `python3`
where that is the Python 3 executable. No environment activation is needed.

Checks default to Python 3.11. Set `UV_PYTHON` to a version or interpreter path:

```sh
UV_PYTHON=3.14 just check
UV_PYTHON=3.11 just sync
```

In PowerShell, set `$env:UV_PYTHON = '3.14'` before running the command. Run
interpreter checks sequentially because they share `python/.venv`.

`check` fails on any failed command, no tests, or combined line/branch coverage
below 95%. Platform-specific tests run on applicable hosts. Package checks verify
exact payloads, metadata, source-archive rebuilds, installed typing and actual
example CLI behavior; see [examples](examples.md#what-check-package-verifies).

`audit-dependencies` uses the live PyPI vulnerability service and every locked
registry version across all dependency groups and platform markers. It uses
`--no-deps --disable-pip` to avoid a fresh resolution and `--strict` to fail
incomplete lookups. Vulnerability findings and lookup failures fail the command.
It does not assess native uv/actionlint binaries or GitHub Actions.

`check-workflow` validates YAML, expressions, action inputs, matrices and job
dependencies. Optional shellcheck/pyflakes integrations are disabled so results
do not depend on unpinned local tools. Dependency audit and workflow validation
are separate commands and CI jobs.

## Repository files and generated output

Source, tests, synthetic conformance fixtures, example applications, public docs
and build configuration belong in the repository. Keep run logs, coverage,
review reports, handoffs and other session records in ignored cache directories.
Do not paste machine-specific paths into tracked files.

Tools, downloaded interpreters, dependency caches and temporary roots use the
repository-root `.cache/`. Python reports, check-package installations and tool
caches use `python/.cache/`; the environment uses `python/.venv/` and release
artifacts use `python/dist/`. Scripts preserve HOME and tests use isolated targets.
The root `lefthook.yml` configures `just check`; maintainers manage hook activation.
Hooks run the unit tier only. Integration tests run on the nightly CI schedule, on
workflow dispatch, and in full on every release.

The Flyrail wheel contains the library, typing marker and distribution metadata.
Its source archive contains source, tests, the shared conformance fixtures, package
metadata and license. Examples and contributor tooling live in the repository.
Package checks compare complete archive contents against the selected source bytes.

## CI and maintenance

[CI](../../.github/workflows/ci.yml) runs the shared check on Ubuntu 24.04 and
macOS 15 with Python 3.11 and 3.14. `setup-python` supplies a modern
3.14 bootstrap; pinned uv selects the tested interpreter using matrix `UV_PYTHON`
and `UV_MANAGED_PYTHON=1`. The shared gate prints the running version/executable
and verifies its base against `uv python find --system --managed-python`.
All four jobs must pass before a release claims those platforms. Windows 2025 is
not in the matrix: the suite has never passed there, so Windows is unsupported.

Every push and pull request runs the fast tier. The integration matrix runs on the
nightly schedule and on workflow dispatch, and the release workflow runs both tiers
on every supported platform, so no release publishes without integration results.

Workflows use pinned action commits, pinned uv, a
checksum-pinned actionlint archive and read-only repository permissions.

[Dependabot](../../.github/dependabot.yml) groups weekly Python dependency updates
under `/python` and GitHub Actions updates separately. Keep Hatchling's build and
dev pins consistent. Update uv/actionlint versions and checksums explicitly; they
are embedded tool versions, separate from action versions.

## Releasing Python

Flyrail publishes the `pyflyrail` distribution while retaining the `flyrail`
import package. Python package versions, configuration bundle labels and shared
format schema versions are independent.

Before the first release, add a pending PyPI Trusted Publisher for project
`pyflyrail`, owner `nielsmadan`, repository `flyrail` and workflow
`release-python.yml`, with no environment. The release workflow requests a
short-lived OIDC identity and stores no PyPI token.

Run releases from a clean, complete `main` checkout whose origin and local Python
tags match:

```sh
just release --dry-run
just release
just release patch
just release 0.2.0
```

The helper considers commits touching `python/` or `spec/`, proposes a semantic
version from conventional commit subjects, then runs the complete check,
dependency audit and workflow validation. After confirmation it updates the
project and example dependency versions, refreshes `uv.lock` and
[`CHANGELOG.md`](../CHANGELOG.md), creates one release commit, creates an annotated
`python-vVERSION` tag and atomically pushes `main` with that tag. Package QA runs
again against the prepared version before the release commit is created.

CI validates that the immutable tag belongs to `main` and matches the project
version. A read-only job reruns all release gates, builds and installs the sdist
and wheel, and saves one verified artifact bundle. A separate OIDC-only job
publishes the bundle to PyPI. Only after PyPI succeeds does a `contents: write`
job create the GitHub Release and attach the same artifacts. The local helper
prints the workflow and release locations; it does not require the GitHub CLI and
does not wait for publication. Follow the printed workflow link to confirm it.

If publication fails after the tag is pushed, fix the workflow on `main` and
retry the existing tag from the workflow's Actions page with **Run workflow** on
`main`, supplying the tag. Never move or replace a published tag. With the GitHub
CLI available the same retry is:

```sh
gh workflow run release-python.yml --ref main -f tag=python-v0.1.0
```

Future implementations use independent ecosystem-aware tags: Python
`python-vX.Y.Z`, Go under `go/` uses `go/vX.Y.Z`, Swift uses bare `X.Y.Z`, Rust
uses `rust-vX.Y.Z` and TypeScript uses `typescript-vX.Y.Z`. A shared `spec/`
change must pass every available implementation's conformance tests, but it does
not force simultaneous registry publication.
