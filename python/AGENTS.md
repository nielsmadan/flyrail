# Flyrail

Flyrail is a synchronous Python 3.11+ library for reversible skills, instructions,
MCP registrations and hooks at explicit local destinations. Keep dependencies small
and public values immutable. Python library calls must never print, prompt, exit, execute
bundled commands, make network requests, or select installed agents automatically.

Read README.md and the relevant docs before changing an interface. Keep public
API, lifecycle, translation and example docs consistent with behavioral changes.
Public prose belongs in documentation. Prefer clear names over comments and docstrings.

Use `just check` for the shared Ruff formatting/lint/security, strict mypy, and the
unit test tier; it is the git-hook gate and must stay fast. Use `just check-integration`
for hook runtime checks, the complete suite with branch coverage, and package QA.
Mark a test `@pytest.mark.integration` when it spawns a runtime or subprocess, or
drives a full on-disk lifecycle. `just format` formats Python code. The same commands
are available through `python3 scripts/check.py`. Set `UV_PYTHON` to check another
supported interpreter; checks default to Python 3.11. Supported platforms are macOS
and Linux; the Windows adapter is unsupported and not covered by CI.
Keep dependencies pinned in pyproject.toml and regenerate uv.lock with `just lock`.
`just release --dry-run` previews a Python release. Creating a release commit or
tag, pushing, and publishing require explicit user authorization.

Keep caches, temporary files, downloaded tools, and test installations inside this
checkout. The shared check script uses repository-root `.cache` for tools and temporary
files, and `python/.venv` for the development environment.
Tests must use temporary roots and explicit environment overrides, never the real
user's agent configuration. Do not change HOME, activate hooks, or install global
tools. The repository-root lefthook.yml is configuration only.

Validate public input behavior and observable filesystem outcomes. Do not add
tests that just repeat enum members or implementation details. Run the complete
shared check and read its exit status and full test summary before reporting.

Shared conformance fixtures live in `../spec/fixtures`; packaging includes exact
copies for tests in a standalone source distribution. Keep fixture bytes portable.

Hook development checks require Node, npm and Bun; Python-only builds do not.
See docs/development.md for runtime pins and optional Codex login-shell coverage.
