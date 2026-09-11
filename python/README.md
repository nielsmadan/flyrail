# Flyrail for Python

A typed, synchronous library for installing coding-agent skill bundles. Requires
Python 3.11+ and has no third-party runtime dependencies. Windows mutations require
Python 3.11.10+, 3.12.4+, or 3.13+ for private directory creation.

## Install and use

From this repository's `python/` directory, install into your application's virtual
environment:

```sh
python -m pip install .
```

Run this example from `python/`:

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

Check every mutation result's `status`, `error` and `recovery_paths`. `APPLIED`
means the receipt committed; cleanup can still need attention. Successful targets
remain applied when another fails. Own edits block replacement/removal unless
`replace_modified=True`; untracked and foreign-owned skills remain protected.
Uninstall uses the bundle ID and receipts, without requiring source files.

## Documentation and examples

- [Public API](https://github.com/nielsmadan/flyrail/blob/main/python/docs/api.md)
- [Destinations and inspection](https://github.com/nielsmadan/flyrail/blob/main/python/docs/inspection.md)
- [Lifecycle and recovery](https://github.com/nielsmadan/flyrail/blob/main/python/docs/lifecycle.md)
- [Examples](https://github.com/nielsmadan/flyrail/blob/main/python/docs/examples.md)
- [Support and limits](https://github.com/nielsmadan/flyrail/blob/main/python/docs/support.md)
- [Shared formats](https://github.com/nielsmadan/flyrail/tree/main/spec)

The repository's `examples/` contains two independently packaged host CLIs: notes
with an explicit filesystem bundle and checksum with packaged resources. Examples
and development tooling live in the repository; the source distribution contains
library code, tests, shared fixtures, package metadata and the license.

For repository development, run `just check` from `python/` or the repository root.
It checks formatting, lint/security, strict typing, tests with branch coverage,
built package contents and installed consumers. The repository CI matrix covers
Python 3.11 and 3.14 on macOS, Linux and Windows. See
[development](https://github.com/nielsmadan/flyrail/blob/main/python/docs/development.md).
