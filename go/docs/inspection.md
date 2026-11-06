# Destinations and read-only inspection

[Skill destinations](#skill-destinations) · [Explicit home and environment](#explicit-home-and-environment) · [Configuration inspection](#configuration-inspection) · [Skill inspection](#skill-inspection) · [Safety](#safety)

## Skill destinations

`DirectoryTarget(root)` selects an exact skill container.
`ProjectTarget(agent, root)` and `UserTarget(agent, home, lookupEnv)` use
documented agent presets:

| Agent | User default | User relocation | Project, relative to root |
| --- | --- | --- | --- |
| `AgentClaude` | `~/.claude/skills` | `CLAUDE_CONFIG_DIR/skills` | `.claude/skills` |
| `AgentCodex` | `~/.agents/skills` | None | `.agents/skills` |
| `AgentOpencode` | `~/.config/opencode/skills` | `XDG_CONFIG_HOME/opencode/skills` | `.opencode/skills` |
| `AgentPi` | `~/.pi/agent/skills` | `PI_CODING_AGENT_DIR/skills` | `.pi/skills` |
| `AgentCursor` | `~/.cursor/skills` | None | `.cursor/skills` |
| `AgentCopilot` | `~/.copilot/skills` | `COPILOT_HOME/skills` | `.github/skills` |
| `AgentDroid` | `~/.factory/skills` | None | `.factory/skills` |

Presets are path conveniences, not exclusive-discovery promises. Ownership
belongs to the physical installation. Removing a shared container affects every
agent that discovers it.

The Droid preset permits explicit target construction but is not a supported
destination. Skill installation, update and removal return a `StatusFailed`
result with the message `Flyrail detects droid but has no verified configuration
translation.` before reading or writing its root.

## Explicit home and environment

`UserTarget` never reads the process environment. It takes the home directory
and a `lookupEnv func(string) (string, bool)`:

- a non-empty `home` is used as given;
- an empty `home` is read from `lookupEnv("HOME")`, then `USERPROFILE`, and must
  be absolute;
- a nil `lookupEnv` means no relocation variables.

A CLI that wants the process environment passes `os.LookupEnv`:

```go
target, err := flyrail.UserTarget(flyrail.AgentClaude, "", os.LookupEnv)
```

Go has no ambient account-database fallback for an unset `HOME`; a caller that
wants one passes `user.Current().HomeDir` as `home`. Relocations must be
absolute or start with `~/`, which expands against the selected home. Relative
ordinary target paths are made absolute against the current directory at
construction, without collapsing `..`. Constructors perform no filesystem
mutation.

`CODEX_HOME` does not relocate Codex skills. `OPENCODE_CONFIG_DIR` does not
replace the OpenCode skill preset. Use `DirectoryTarget` for custom layouts. See
[translation](translation.md) for instruction and MCP routing.

## Configuration inspection

`InspectInstallation(ctx, bundleID, target)` reads current, pending, previous,
and residual index membership plus referenced receipts and actual owned content
from the target's store. It does not acquire locks, recover transactions,
create directories, or change permissions. Reads may still touch the store, for
example updating access times on a `DirectoryStore`.

`InstallationObservation` reports recorded version and digests, pending state,
resource observations, and errors. Each `ResourceObservation` exposes its
destination, claim attribution and status, revision digest, diagnostics, and
`RecoveryPaths`. A nonempty `RecoveryPaths` names the transaction directory
recorded by the resource's preparation, in the store's staging directory or
beside the destination when the store is on another mount, left behind by an
interrupted write; a later recovery removes it. A transaction directory that no
record in the store names is not reported. Full foreign bytes and takeover
baselines are not exposed.

The `Current` field requires intact recorded claims with no unresolved
membership or recovery. `Matches(bundle, rendered)` additionally compares the
desired bundle and rendering. Both are snapshot comparisons and do not verify
host activation. Inspect again to observe later changes.

## Skill inspection

`Inspect(ctx, bundle, targets, store)` returns ordered `TargetInspection` values,
reading recorded membership from the given store. Each contains the original
target, canonical root, an alias flag and index, and the skill-facing
`Observation`. Independent target errors do not stop the remaining targets.

`ObservationState` is `StateInstalled`, `StateAbsent`, `StateRecoveryNeeded`, or
`StateUnknown`. Inventory entries expose paths, sizes, SHA-256 digests, and
executable intent, not file contents. Modifications identify changed owned
claims; conflicts represent desired unowned content. `Observation.IsCurrent()`
is computed from the version, recorded-content and content checks. Use a general
lifecycle preview for complete proposed resource changes.

The `*bool` fields of `Observation` (`VersionMatches`,
`RecordedContentMatches`, `ContentMatches`) are nil when the check could not be
made.

The skill adapter identifies an installation within the store by an
`InstallationTarget` whose context is the canonical physical container's
destination, not a name derived from anything beside it. Equivalent directory
and agent-preset routes that canonicalize to the same container can inspect,
update, or remove that installation in later calls, regardless of their order,
as long as they are given the same store.

## Agent evidence

`DetectAgents(ctx, home, opts)` reports executable and macOS application
evidence for each agent, in `Agents()` order. It never selects an agent. The
search path comes from `DiscoveryOptions.SearchPath`, so a CLI that wants the
ambient `PATH` passes `filepath.SplitList(os.Getenv("PATH"))`:

```go
presence, err := flyrail.DetectAgents(ctx, home, flyrail.DiscoveryOptions{SearchPath: filepath.SplitList(os.Getenv("PATH"))})
```

## Safety

Canonicalization resolves physical aliases and rejects managed symlinks, hard
links, special files, ambiguous portable names, and incompatible hierarchy
claims. Rendered destinations and retained bundle sources must not overlap
the store's location or staging directory.

Malformed state and contradictory ownership fail closed. Inspection may observe a
live writer; pending state alone does not prove interruption. Only a locked
mutation can run [recovery](lifecycle.md).
