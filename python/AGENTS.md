# Flyrail

Flyrail is a synchronous Python 3.11+ library for installing skill bundles into
explicit local agent directories. Keep runtime dependencies small and public
values immutable. Library code must never print, prompt, exit, execute skill
scripts, make network requests, or select installed agents automatically.

Read README.md and the relevant docs before changing an interface. Public prose
belongs in documentation. Prefer clear names over comments and docstrings.

Use `just check` for the shared Ruff formatting/lint/security, strict mypy, and
pytest branch-coverage checks. `just format` formats Python code. The same commands
are available through `python3 scripts/check.py` (Windows: `python`). Set
`UV_PYTHON` to check another supported interpreter; checks default to Python 3.11.
Keep dependencies pinned in pyproject.toml and regenerate uv.lock with `just lock`.

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
