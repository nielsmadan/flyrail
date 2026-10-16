# Destinations and read-only inspection

[Skill destinations](#skill-destinations) · [Explicit home and environment](#explicit-home-and-environment) · [Configuration inspection](#configuration-inspection) · [Skill inspection](#skill-inspection) · [Safety](#safety)

## Skill destinations

`Target::directory(root)` selects an exact skill container.
`Target::project(agent, root)` and the `Target::user*` constructors use documented
agent presets:

| Agent | User default | User relocation | Project, relative to root |
| --- | --- | --- | --- |
| `Claude` | `~/.claude/skills` | `CLAUDE_CONFIG_DIR/skills` | `.claude/skills` |
| `Codex` | `~/.agents/skills` | None | `.agents/skills` |
| `Opencode` | `~/.config/opencode/skills` | `XDG_CONFIG_HOME/opencode/skills` | `.opencode/skills` |
| `Pi` | `~/.pi/agent/skills` | `PI_CODING_AGENT_DIR/skills` | `.pi/skills` |
| `Cursor` | `~/.cursor/skills` | None | `.cursor/skills` |
| `Copilot` | `~/.copilot/skills` | `COPILOT_HOME/skills` | `.github/skills` |
| `Droid` | `~/.factory/skills` | None | `.factory/skills` |

Presets are path conveniences, not exclusive-discovery promises. Ownership
belongs to the physical installation. Removing a shared container affects every
agent that discovers it.

The Droid preset permits explicit target construction but is not a supported
destination. Skill rendering, inspection, installation, update, and removal
return an unsupported result before reading or writing its root.

## Explicit home and environment

`Target::user` snapshots the current account home and retained routing variables.
For deterministic applications and tests, prefer:

- `Target::user_with_home(agent, home)` for an explicit home and empty routing
  environment;
- `Target::user_with_environment(agent, pairs)` when `HOME` or `USERPROFILE` is
  supplied explicitly;
- `Target::user_with_home_and_environment(agent, home, pairs)` for both.

Supplied environment pairs replace ambient routing values. Relocations must be
absolute or start with `~/`, which expands against the selected home. Relative
ordinary target paths are anchored to the current directory at construction.
Constructors perform no filesystem mutation.

`CODEX_HOME` does not relocate Codex skills. `OPENCODE_CONFIG_DIR` does not replace
the OpenCode skill preset. Use `Target::directory` for custom layouts. See
[translation](translation.md) for instruction and MCP routing.

## Configuration inspection

`inspect_installation(bundle_id, target)` reads current, pending, previous, and
residual index membership plus referenced receipts and actual owned content
from the target's store. It does not acquire locks, recover transactions,
create directories, or change permissions. Reads may still touch the store —
for example updating access times on a `DirectoryStore`.

`InstallationObservation` reports recorded version and digests, pending state,
resource observations, and errors. Each resource exposes its destination,
claim attribution/status, revision digest, diagnostics, and recovery paths. A
nonempty `recovery_paths` names the transaction directory recorded by the
resource's preparation — in the store's staging directory, or
`P/.flyrail-<hash>-<transaction-id>.txn` beside destination `P/N` when the
store is on another mount — left behind by an interrupted write; a later
recovery removes it. A transaction directory that no record in the store
names is not reported. Full foreign bytes and takeover baselines are not
exposed.

`is_current` requires intact recorded claims with no unresolved membership or
recovery. `matches(bundle, rendered)` additionally compares the desired bundle
and rendering. Both are snapshot comparisons and do not verify host activation.
Inspect again to observe later changes.

## Skill inspection

`inspect(bundle, targets, store)` returns ordered `TargetInspection<T>` values,
reading recorded membership from the given `store`. Each contains the original
target, canonical root, optional alias index, and skill-facing `Observation`.
Independent target errors do not stop the remaining targets.

`ObservationState` is `Installed`, `Absent`, `RecoveryNeeded`, or `Unknown`.
Inventory entries expose paths, sizes, SHA-256 digests, and executable intent, not
file contents. Modifications identify changed owned claims; conflicts represent
desired unowned content. Use a general lifecycle preview for complete proposed
resource changes.

The skill adapter identifies an installation within `store` by an
`InstallationTarget` whose context is the canonical physical container's
destination, not a name derived from anything beside it. Equivalent directory
and agent-preset routes that canonicalize to the same container can inspect,
update, or remove that installation in later calls, regardless of their
order, as long as they are given the same store.

## Safety

Canonicalization resolves physical aliases and rejects managed symlinks, hard
links, special files, ambiguous portable names, and incompatible hierarchy
claims. Rendered destinations and retained bundle sources must not overlap
the store's location or staging directory.

Malformed state and contradictory ownership fail closed. Inspection may observe a
live writer; pending state alone does not prove interruption. Only a locked
mutation can run [recovery](lifecycle.md).
