# Embedding Flyrail in a host CLI

[Build and try the examples](#build-and-try-the-examples) · [Explicit configuration commands](#explicit-configuration-commands) · [Resource packaging](#resource-packaging) · [What check-package verifies](#what-check-package-verifies)

The examples are independent installable applications. Each exposes a useful
host command and four explicit `ai` subcommands. They use only Flyrail's public
API and standard-library argument parsing, reporting and exit handling.
Checksum also offers a [combined `config` interface](../examples/packaged/README.md#combined-configuration)
for all four families, with explicit project/agent routing and source-free removal.

| Distribution | Installed command | Bundle source | Host version | Bundle label |
| --- | --- | --- | --- | --- |
| [Notes](../examples/filesystem/) | `flyrail-notes words FILE` | Explicit filesystem directory | `0.4.0` | `notes-2026-09` |
| [Checksum](../examples/packaged/) | `flyrail-checksum digest FILE` | Resources included in its wheel | `2.1.0` | `checksum-2026-09` |

Neither host reads skill resources during its ordinary work command. Bundle
labels come from `flyrail.json`, independently of application package metadata.
Updating skills is an explicit user action, including when a new host package
ships a new bundle. Flyrail does not compare application versions.

## Build and try the examples

From `python/`, create an isolated environment and install the library and both
examples with uv:

```sh
uv venv .cache/example-venv
uv pip install --python .cache/example-venv/bin/python . examples/filesystem examples/packaged
.cache/example-venv/bin/flyrail-notes words README.md
.cache/example-venv/bin/flyrail-checksum digest LICENSE
.cache/example-venv/bin/flyrail-notes ai install \
  --source examples/filesystem/bundle --target .cache/demo-notes-skills
.cache/example-venv/bin/flyrail-checksum ai install --target .cache/demo-checksum-skills
```

On Windows, virtual-environment executables are under `Scripts`:
`Scripts/python.exe`, `Scripts/flyrail-notes.exe`, and
`Scripts/flyrail-checksum.exe`. Use those paths in the commands above. The example
destinations stay inside the checkout's ignored cache.

Run `just check-package` to exercise built distributions with the pinned tools.
It retains its artifacts and disposable installation under
`python/.cache/check-package/` and prints the run directory when it finishes.

## Explicit configuration commands

Filesystem content operations require a source path:

```sh
flyrail-notes ai status --source /path/to/bundle --target /path/to/skills
flyrail-notes ai install --source /path/to/bundle --target /path/to/skills
flyrail-notes ai update --source /path/to/bundle --target /path/to/skills
flyrail-notes ai uninstall --bundle-id example-notes --target /path/to/skills
```

The package example obtains its source from
`Bundle.from_package("flyrail_example_checksum")`:

```sh
flyrail-checksum ai status --target /path/to/skills
flyrail-checksum ai install --target /path/to/skills
flyrail-checksum ai update --target /path/to/skills
flyrail-checksum ai uninstall --target /path/to/skills
```

Repeat `--target` to request several destinations. Each path selects the skill
container itself, as with `Target.directory`. The hosts never discover or choose
agents automatically. The notes host accepts a bundle ID on removal. The checksum
host retains its stable ID in code and checks it against loaded manifests for
content operations. Both take the uninstall branch before loading resources.

Each AI command prints one JSON report containing the application version, bundle
ID, desired bundle label (`null` during uninstall), and ordered target results.
Results include public target attribution, resolved root, alias attribution,
observation, and `is_current`; mutations also include operation status, error and
recovery paths. Private target-resolution fields are omitted. This JSON is the
examples' presentation choice, not an additional Flyrail serialization API.

| Exit | Meaning |
| --- | --- |
| `0` | Host command succeeded; status is current at every target; or every mutation completed without errors or remaining recovery work. |
| `1` | Status is absent/outdated/modified or errored; or a mutation reported failure, incomplete recovery, or an error after commit. Inspect every target result: other targets may have succeeded. |
| `2` | Invalid CLI usage or a source/file/request error, explained on stderr. |

`update` and `uninstall` accept `--replace-modified` for explicit replacement or
deletion of observed same-owner edits, including files added inside owned skills.
It cannot acquire untracked or foreign-owned skill roots. Review observations
first; see [lifecycle behavior](lifecycle.md) for the full rules. No host prompts
and no library calls print or exit.

## Resource packaging

The checksum wheel includes the manifest, `SKILL.md`, a binary fixture,
`scripts/verify_checksum.py`, and `scratch/README.txt`. The manifest declares
executable intent; unpacked archive permission bits are not the authority.
Flyrail applies that intent on POSIX and records it on Windows.

Git does not track empty directories, and Hatchling 1.32.0 omits them from wheels.
Flyrail preserves the tree the resource provider actually supplies; it cannot
restore directories omitted upstream. Include a visible placeholder such as
`scratch/README.txt` when a directory must survive Git and packaging. Filesystem
and explicit-ZIP conformance tests separately cover genuinely empty directories.
No additional manifest schema is needed for this packaging constraint.

## What check-package verifies

The shared `just check` also runs this gate. It compares wheel payloads and source
archive members against expected source bytes, checks metadata and dependencies,
and rebuilds every wheel from its source archive with byte-identical results.
The Flyrail source archive contains library source, tests, shared conformance
fixtures, package metadata and license. Each example has its own distribution.
Runtime Flyrail depends only on pinned `tomlkit`, imported lazily for TOML editing.
The package check downloads that exact wheel, verifies its lockfile hash and
metadata, and rejects unexpected transitive runtime dependencies.

Builds and byte-identical rebuilds run with an empty PATH, so they cannot use
Node, npm, Bun or a JavaScript compiler. All three authored runtime modules ship
as source assets.

The gate installs the three rebuilt wheels and the verified runtime dependency with `--no-index --link-mode copy`
into a fresh environment outside source directories. Console scripts run from
a separate consumer directory with no source `PYTHONPATH`. Strict mypy checks an
external consumer against that environment, and runtime checks verify its
Flyrail import and `py.typed` come from the installed wheel.

Installed CLI probes cover ordinary host commands, status/install/update/uninstall
and repeats, version-only and same-label content updates, partial target failures,
refusal of local edits and explicit replacement, preservation of foreign and
untracked data, and uninstall after filesystem sources or installed resources
are deleted. For the package example, these probes alter only a disposable
installed copy to simulate another bundle revision while retaining host metadata.
An additional probe imports resources directly from the built wheel as a ZIP,
removes the archive after loading, and verifies snapshot installation, binary
bytes, executable intent, placeholder directories and removal.

The combined CLI consumer verifies generated guidance and both MCP environment
reference forms, executes the installed portable hook with its packaged assets,
and compares new guidance and moved rendered assets under the same label. It
checks read-only status, unchanged updates, edit protection, unsupported preflight,
and source-free removal. Host activation stays explicitly unchecked.
