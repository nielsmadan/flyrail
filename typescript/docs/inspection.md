# Skills, inspection and agent discovery

[Skill destinations](#skill-destinations) | [Explicit home and environment](#explicit-home-and-environment) | [Skill inspection](#skill-inspection) | [Agent evidence](#agent-evidence) | [Safety](#safety)

## Skill destinations

`Target.directory(root)` selects an exact skill container.
`Target.project(agent, root)` and `Target.user(agent, home, environment?)` use
the documented agent presets:

| Agent      | User default                | User relocation (`environment` key) | Project, relative to root |
| ---------- | --------------------------- | ----------------------------------- | ------------------------- |
| `claude`   | `~/.claude/skills`          | `CLAUDE_CONFIG_DIR/skills`          | `.claude/skills`          |
| `codex`    | `~/.agents/skills`          | None                                | `.agents/skills`          |
| `opencode` | `~/.config/opencode/skills` | `XDG_CONFIG_HOME/opencode/skills`   | `.opencode/skills`        |
| `pi`       | `~/.pi/agent/skills`        | `PI_CODING_AGENT_DIR/skills`        | `.pi/skills`              |
| `cursor`   | `~/.cursor/skills`          | None                                | `.cursor/skills`          |
| `copilot`  | `~/.copilot/skills`         | `COPILOT_HOME/skills`               | `.github/skills`          |
| `droid`    | `~/.factory/skills`         | None                                | `.factory/skills`         |

Presets are path conveniences, not exclusive-discovery promises. Ownership
belongs to the physical installation. Removing a shared container affects every
agent that discovers it.

The Droid preset permits explicit target construction but is not a supported
destination. `install`, `update` and `uninstall` return a `failed` result with
the message "Flyrail detects droid but has no verified configuration
translation." before reading or writing its root.

## Explicit home and environment

`Target.user` never reads the process environment. `home` is required and must
be absolute. `environment` is an explicit record of relocation variables
(`CLAUDE_CONFIG_DIR`, `XDG_CONFIG_HOME`, `PI_CODING_AGENT_DIR`, `COPILOT_HOME`);
omit it for none. Constructors perform no filesystem mutation. A CLI that wants
the process environment passes `os.homedir()` and the variables itself.

## Skill inspection

`inspect(bundle, targets, store)` returns one `TargetInspection` per target, in
order, reading recorded membership from the store. Each carries the caller's
`Target`, the canonical `root`, `aliasOf` (`null`, or the index of the earlier
target with the same canonical root) and the skill-facing `Observation`.
Independent target errors do not stop the remaining targets. `inspect` takes no
lock, runs no recovery and creates no directories.

`Observation.state` is `installed`, `absent`, `recovery_needed` or `unknown`.
Inventory entries expose paths, `bigint` sizes, SHA-256 digests and executable
intent, never file contents. A `Modification` names a changed owned skill and
its kind (`missing`, `added`, `type_changed`, `content_changed`,
`executable_changed`); a `Conflict` represents desired unowned content.
`isCurrent` is computed from the version, recorded-content and content checks.
`versionMatches`, `recordedContentMatches` and `contentMatches` are `null` when
the check could not be made. Use a lifecycle `preview` for complete proposed
resource changes.

The skill adapter identifies an installation within the store by an
`InstallationTarget` whose context is the canonical physical container's
destination. Equivalent directory and preset routes that canonicalize to the
same container can inspect, update or remove that installation in later calls,
in any order, as long as they are given the same store.

## Agent evidence

`detectAgents({ searchPath, ... })` reports executable and macOS application
evidence per agent. It never selects an agent. The search path is always
explicit:

```ts
const presence = await detectAgents({ searchPath: ['/usr/local/bin', '/opt/homebrew/bin'], home });
```

## Safety

Canonicalization resolves physical aliases and rejects managed symlinks, hard
links, special files, ambiguous portable names and incompatible hierarchy
claims. Malformed state and contradictory ownership fail closed. Inspection may
observe a live writer; pending state alone does not prove interruption. Only a
locked mutation runs [recovery](lifecycle.md).

`install`, `update` and `uninstall` follow the AbortSignal rules in
[lifecycle](lifecycle.md): an abort before the first durable write rejects with
`signal.reason` (no index, receipt, journal or preparation written, no
destination content published or removed, no recovery run; lock files, parent
directories and authority records may persist); after it, the real result
resolves. A multi-target call stays cancellable across targets whose run ended
`unchanged` or failed preflight.
