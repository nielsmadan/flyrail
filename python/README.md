# Flyrail for Python

A typed, synchronous library for coding-agent configuration bundles: skills,
instructions, MCP registrations, hooks and supporting assets. Requires Python
3.11+; TOML editing lazily imports pinned `tomlkit`. Windows mutations require
Python 3.11.10+, 3.12.4+, or 3.13+ for private directory creation.

## Install and use

Install the `pyflyrail` distribution from PyPI. The import package remains `flyrail`:

```sh
python -m pip install pyflyrail
```

To install the current checkout instead, run `python -m pip install .` from `python/`.
The following example uses the same public import in either installation:

```python
from flyrail import Bundle, Target, inspect, install, uninstall, update

bundle = Bundle.from_directory("examples/filesystem/bundle")
targets = [Target.directory(".cache/demo-skills")]

installed = install(bundle, targets)
current = all(result.observation.is_current for result in inspect(bundle, targets))
updated = update(bundle, targets, lock_timeout=0.25)
removed = uninstall(bundle.id, targets)
```

`Target.directory` names the skill container itself. This example installs
`.cache/demo-skills/notes-summary/SKILL.md`. Use `Bundle.from_package("your_package")`
for application resources with a `flyrail/` bundle directory. Both loaders snapshot
complete trees before returning, so later removal of the source does not invalidate
an already loaded bundle.

Presets cover Claude Code, Codex, OpenCode, Pi, Cursor and Copilot. For example,
`Target.user("codex")` selects a user installation and
`Target.project("claude", "./project")` selects a project installation. Hosts choose
explicit destinations; the library never selects agents, prints, prompts, exits,
executes skill scripts or requests the network.

Bundle version labels are independent of your application's version. Inspection
compares the label, recorded content and actual content separately. Updates can
change content under the same label or install an older-looking label. An intact
matching revision returns `UNCHANGED`.

Check every mutation result's status, errors and resource recovery paths. A
committed resource can still need cleanup; `PARTIAL` reports completed resource
commits followed by a failure. Own edits block replacement/removal unless
`replace_modified=True`; untracked and foreign-owned skills remain protected.
Uninstall uses the bundle ID and receipts, without requiring source files.

For portable instructions, MCP, hooks and supporting assets, use `render(bundle, context)`
with an explicit `RenderContext`. It returns a `RenderedBundle` for
`preview`/`apply_preview` or `sync` with an `InstallationTarget`.
[Translation](https://github.com/nielsmadan/flyrail/blob/main/python/docs/translation.md)
explains capabilities, native alternatives and exact unsupported boundaries.
`inspect_installation` and `remove` use the private index without source bytes.
For applications that select destinations from observed files, call source-free
`recover_installation` before re-reading those files and building a fresh immutable
preview. Recovery preserves healthy ownership and retains pending membership for
the subsequent update or removal.
Use `inspect_installation(bundle.id, target).matches(bundle, rendered)` to compare
the app's current bundle and rendering with intact installed content. The
source-free `is_current` field checks the recorded generation only. Neither check
establishes host trust, discovery or activation; inspect render notices separately.
The [configuration API](https://github.com/nielsmadan/flyrail/blob/main/python/docs/api.md)
shows an in-memory instruction example and explains explicit takeover/adoption.

## Documentation and examples

- [Public API](https://github.com/nielsmadan/flyrail/blob/main/python/docs/api.md)
- [Command hooks](https://github.com/nielsmadan/flyrail/blob/main/python/docs/hooks.md)
- [Destinations and inspection](https://github.com/nielsmadan/flyrail/blob/main/python/docs/inspection.md)
- [Lifecycle and recovery](https://github.com/nielsmadan/flyrail/blob/main/python/docs/lifecycle.md)
- [Examples](https://github.com/nielsmadan/flyrail/blob/main/python/docs/examples.md)
- [Support and limits](https://github.com/nielsmadan/flyrail/blob/main/python/docs/support.md)
- [Shared formats](https://github.com/nielsmadan/flyrail/tree/main/spec)

The repository's `examples/` contains two independently packaged host CLIs: notes
with an explicit filesystem bundle and checksum with packaged resources. They
include a combined checksum `config` interface with generated guidance, skills,
stdio/HTTP MCP environment references and a portable hook with packaged assets.
Native hook execution requires an explicitly selected Node 24+ executable;
OpenCode/Pi can use their existing host runtimes. Python package builds require
no JavaScript runtime or compiler.

Examples and development tooling live in the repository; the source distribution
contains library code, tests, shared fixtures, runtime test tooling, package
metadata and the license.

For repository development, run `just check` from `python/` or the repository root.
It checks formatting, lint/security, strict typing, tests with branch coverage,
built package contents and installed consumers. The repository CI matrix covers
Python 3.11 and 3.14 on macOS, Linux and Windows. See
[development](https://github.com/nielsmadan/flyrail/blob/main/python/docs/development.md).
