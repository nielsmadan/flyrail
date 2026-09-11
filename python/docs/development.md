# Python development

Run the commands below from `python/`. The repository-root Justfile forwards the
same commands here. Python 3.11+ and uv 0.12.13+ are required; just is optional.
Runtime dependencies are empty. Build, development and audit dependencies are
pinned in `pyproject.toml` and resolved with artifact hashes in `uv.lock`.

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

## Commands

| Command | Work performed |
| --- | --- |
| `just check` | Locked sync, Ruff format/lint/security checks, strict mypy, pytest with branch coverage, and package/consumer checks. |
| `just dependency-audit` | Audit every locked registry package/version with pip-audit. |
| `just workflow-check` | Validate all repository workflows with actionlint. |
| `just package-check` | Build and verify distributions and installed example applications. |
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
example CLI behavior; see [examples](examples.md#what-package-check-verifies).

`dependency-audit` uses the live PyPI vulnerability service and every locked
registry version across all dependency groups and platform markers. It uses
`--no-deps --disable-pip` to avoid a fresh resolution and `--strict` to fail
incomplete lookups. Vulnerability findings and lookup failures fail the command.
It does not assess native uv/actionlint binaries or GitHub Actions.

`workflow-check` validates YAML, expressions, action inputs, matrices and job
dependencies. Optional shellcheck/pyflakes integrations are disabled so results
do not depend on unpinned local tools. Dependency audit and workflow validation
are separate commands and CI jobs.

## Repository files and generated output

Source, tests, synthetic conformance fixtures, example applications, public docs
and build configuration belong in the repository. Keep run logs, coverage,
review reports, handoffs and other session records in ignored cache directories.
Do not paste machine-specific paths into tracked files.

Tools, downloaded interpreters, dependency caches and temporary roots use the
repository-root `.cache/`. Python reports, package-check installations and tool
caches use `python/.cache/`; the environment uses `python/.venv/` and release
artifacts use `python/dist/`. Scripts preserve HOME and tests use isolated targets.
The root `lefthook.yml` configures `just check`; maintainers manage hook activation.

The Flyrail wheel contains the library, typing marker and distribution metadata.
Its source archive contains source, tests, the shared conformance fixtures, package
metadata and license. Examples and contributor tooling live in the repository.
Package checks compare complete archive contents against the selected source bytes.

## CI and maintenance

[CI](../../.github/workflows/ci.yml) runs the shared check on Ubuntu 24.04, macOS 15
and Windows 2025 with Python 3.11 and 3.14. All six jobs must pass before a release
claims those platforms. Workflows use pinned action commits, pinned uv, a
checksum-pinned actionlint archive and read-only repository permissions.

[Dependabot](../../.github/dependabot.yml) groups weekly Python dependency updates
under `/python` and GitHub Actions updates separately. Keep Hatchling's build and
dev pins consistent. Update uv/actionlint versions and checksums explicitly; they
are embedded tool versions, separate from action versions.

## Preparing a release

1. Set the package version in `pyproject.toml` and run `just lock`. Host application
   versions and skill bundle labels change independently.
2. Run `just check`, `just dependency-audit` and `just workflow-check`, and obtain
   passing native CI results for every OS/interpreter cell.
3. Run `just build` and review the wheel and source archive in `python/dist/`.
4. Publish the reviewed artifact filenames with `uv publish` when the release is
   authorized. Check/build commands do not publish.
