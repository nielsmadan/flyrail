# Destinations and read-only inspection

`Target.directory(root)` selects the actual skill container. For example,
`Target.directory("./custom-skills")` places skill `review` at
`./custom-skills/review`. `Target.project(agent, root)` and
`Target.user(agent, *, home=None, env=None)` select documented paths beneath an
explicit project or user root. Agent names are the exact lowercase strings below,
or the corresponding `Agent` enum values. Unknown names are errors.

| Agent | User default | User relocation | Project, relative to root |
| --- | --- | --- | --- |
| `claude` | `~/.claude/skills` | `CLAUDE_CONFIG_DIR/skills` | `.claude/skills` |
| `codex` | `~/.agents/skills` | None | `.agents/skills` |
| `opencode` | `~/.config/opencode/skills` | `XDG_CONFIG_HOME/opencode/skills` | `.opencode/skills` |
| `pi` | `~/.pi/agent/skills` | `PI_CODING_AGENT_DIR/skills` | `.pi/skills` |
| `cursor` | `~/.cursor/skills` | None | `.cursor/skills` |
| `copilot` | `~/.copilot/skills` | None | `.github/skills` |

These paths follow the agent documentation for
[Claude skills](https://code.claude.com/docs/en/skills) and
[configuration](https://code.claude.com/docs/en/claude-directory),
[Codex skills](https://learn.chatgpt.com/docs/build-skills),
[OpenCode skills](https://opencode.ai/docs/skills/) and
[configuration](https://opencode.ai/docs/config/),
[Pi skills](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md)
and [configuration](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/config.ts),
[Cursor skills](https://cursor.com/docs/skills), and
[Copilot CLI skills](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-skills)
and [VS Code agent skills](https://code.visualstudio.com/docs/agent-customization/agent-skills).
Presets are path conveniences. Several agents discover `.agents` and Claude
directories, so they do not promise exclusive visibility. Ownership belongs to
the physical installation; it is not a count of logical agent consumers. Removing
a shared installation affects every consumer of that directory. These presets
make no cloud-agent compatibility claim.

`CODEX_HOME` does not relocate `.agents/skills`. `OPENCODE_CONFIG_DIR` adds
discovery and does not replace OpenCode's default skill directory. General Cursor
or Copilot configuration variables do not relocate these presets. Use an explicit
directory target for a custom arrangement.

## Paths, home and environment

Constructors snapshot the chosen paths and attribution as immutable `Target`
values. `target.root` is an absolute requested path; `target.agent` is an `Agent`
or `None` for directory targets; `target.scope` is a `TargetScope` value. Relative
explicit paths are anchored to the working directory at construction. Explicit
root arguments do not expand shell variables or `~`. `..` components retain their
filesystem meaning until canonicalization, including traversal after an ancestor
symlink. Empty paths, NUL, bytes paths, and filesystem-root skill containers are
rejected. Project roots and user homes may themselves be filesystem roots.

With neither `home` nor `env`, user targets use `Path.home()` and the process
environment at construction. An explicit `home` with omitted `env` uses an empty
environment, preventing relocation into another home's configuration. Supplied
`env` completely replaces ambient values and is copied; all keys and values must
be strings. If supplied without `home`, it must contain an absolute `HOME` or
`USERPROFILE`, with `HOME` taking precedence. There is no ambient fallback in
that case. An explicit `home` always takes precedence over environment home values.

Recognized relocation values must be absolute or start with `~/`, expanded against
the selected home; `~` alone selects that home. Empty relocation values select the
default. Relative relocation values are rejected. No other environment-variable
expansion takes place. Invalid argument types raise `TypeError`; invalid values
raise `ValueError`. Target construction performs no filesystem reads or writes.

```python
from flyrail import Target

isolated = Target.user("claude", home="./test-home")
relocated = Target.user(
    "claude",
    home="./test-home",
    env={"CLAUDE_CONFIG_DIR": "~/custom-claude"},
)
custom = Target.directory("./shared-skills")
```

## Inspection API

`inspect(bundle, targets)` accepts a `Bundle` snapshot and a nonempty iterable of
`Target` values. It consumes and validates the complete request before observing
installations. It resolves explicit roots once, permits ordinary system symlinks
above those roots, and refuses symlinks, known Windows reparse points, junctions,
and special files at an explicit root or inside managed paths. Preset path
components and management directories must have exact, unambiguous spelling.

Canonical target and sibling state roots must not overlap any retained bundle
source root, including a package archive, or another destination or its state.
The overlap comparison also rejects portable case-folding collisions. Nested
destinations are errors even when currently absent. Such request errors raise
`ValueError` before any installation content is inspected. Expected destination
filesystem failures, including permission failures during resolution, become
per-target errors and do not prevent independent destinations being inspected.

The return value is a tuple of `TargetInspection` values in request order:

| Field | Meaning |
| --- | --- |
| `target` | Original immutable target with agent/scope attribution. |
| `root` | Resolved physical skill container; requested path if resolution failed. |
| `state_root` | Sibling `.<container-name>.flyrail` directory. |
| `alias_of` | Zero-based index of the first request for the same physical target, or `None`. |
| `observation` | Immutable `Observation`, shared by requests that alias one physical target. |

Existing directories are deduplicated by device and inode identity as well as
canonical path. Missing destinations use their canonical paths. Each physical
installation is read once. The original requested path and consumer attribution
remain available through `target`.

Inspection creates no destination, state, lock, staging, or backup directory. It
does not acquire locks, recover abandoned work, or alter receipt or installed
bytes or permissions. Reads may update filesystem access times. The observation
is best effort: directory and file metadata are rechecked to detect concurrent
changes, but there is no globally coherent snapshot against arbitrary external
writers.

## Observation fields

`observation.is_current` is the convenience answer for whether the requested
bundle revision is installed and intact. It requires an installed same-owner
receipt, matching version, recorded content and actual content, no same-owner
modification, no desired-skill conflict, and no error or pending recovery. An
unrelated bundle's content edits do not by themselves change this value.

The explanatory fields stay independent:

| Field | Meaning |
| --- | --- |
| `state` | `INSTALLED` when an active same-owner receipt exists; `ABSENT` without one, including after a removal receipt; `RECOVERY_NEEDED` when pending state exists; `UNKNOWN` when inspection cannot safely establish state. |
| `installed` | Requested bundle's `Installation`, or `None`. |
| `installations` | All active receipts as `Installation` values, ordered by receipt filename. |
| `version_matches` | Receipt version equals requested opaque version; `None` without an active same-owner receipt. |
| `recorded_content_matches` | Receipt content digest equals the bundle digest; `None` without an active same-owner receipt. |
| `content_matches` | Actual selected bytes, directory names and executable intent equal the requested content. `None` when inspection fails. |
| `modifications` | Changes against every active receipt, including other bundles, ordered by receipt filename then UTF-8 path bytes. |
| `conflicts` | Desired skill roots claimed by another bundle, occupied without ownership, or present with different portable-equivalent spelling. |
| `error` | `TargetError`, or `None`; an error has `code`, `message`, optional absolute `path`, and optional OS `errno`. |
| `recovery_paths` | Existing transaction record or nonempty staging/backup locations that require recovery. |

Actual-content comparison includes all requested skills plus any retired skills
still owned by this bundle. Unrelated unowned entries and unrelated bundles'
skills are excluded. Byte-identical untracked or foreign-owned content can match
the desired content while remaining a conflict. Editing installed bytes to match
a newly supplied bundle can make `content_matches=True` while
`recorded_content_matches=False` and same-owner modifications remain. Neither
case is current. A version-only change leaves the two content comparisons true
and `version_matches=False`.

Each `Installation` contains `bundle_id`, `version`, `content_digest`,
`transaction_id`, and a tuple of `InventoryEntry` values. Inventory entries retain
installed paths, directories, file sizes, file SHA-256, and executable intent.
`InventoryEntry` exposes `path`, `size`, `sha256`, `executable`, and
`is_directory`. Directories have `size=None` and `sha256=None`; files have an
integer byte size and hexadecimal SHA-256, including size zero for empty files.
Receipt versions are equality-only labels; there is no ordering.

`Modification` contains `bundle_id`, installed-relative `path`, and a
`ModificationKind`: `MISSING`, `ADDED`, `TYPE_CHANGED`, `CONTENT_CHANGED`, or
`EXECUTABLE_CHANGED`. File bytes and executable intent changing together produce
two modifications. Removing a directory produces missing records for it and its
recorded descendants. Every recorded directory, including an empty one, matters.
POSIX considers a file executable when any execute bit is set; other permission
bits are not part of installed content identity. On Windows, the receipt retains
logical executable intent because POSIX execute bits cannot be verified there.
Unowned Windows files have no recorded executable intent.

`Conflict` contains `path` and `owner`, which is a bundle ID or `None` for an
untracked entry. Ownership persists when the owned skill or files are missing.
All receipts are validated and reconciled before ownership is trusted. Missing,
changed, or added content is an observation, while malformed, unsupported, or
contradictory receipt state is an `INVALID_STATE` error. Other error codes are
`IO_ERROR`, `UNSAFE_PATH`, `CONCURRENT_CHANGE`, and `RECOVERY_NEEDED`.

Any transaction record, even a truncated or unsupported one, or nonempty staging
or backup directory makes inspection report recovery needed. This read-only API
does not interpret a transaction as permission to recover or claim its outcome.
An invalid receipt still fails closed when recovery state is also present.
Unknown management entries are invalid state. See the
[receipt format](../../spec/receipt-format.md) for storage and validation rules.


Mutation results reuse these observations and retain operation status/error fields
separately. See the [lifecycle API](lifecycle.md). Only a mutation holding the
permanent lock can interpret pending transaction state as abandoned and perform
[recovery](../../spec/transaction-protocol.md). Inspection can encounter a live transaction;
`RECOVERY_NEEDED` does not establish that its writer has stopped.
