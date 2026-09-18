# Destinations and read-only inspection

[Paths, home and environment](#paths-home-and-environment) · [Read-only configuration inspection](#read-only-configuration-inspection) · [Skill convenience inspection](#skill-convenience-inspection) · [Path and state safety](#path-and-state-safety)

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
| `copilot` | `~/.copilot/skills` | `COPILOT_HOME/skills` (CLI) | `.github/skills` |

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
configuration variables do not relocate these presets. `COPILOT_HOME` applies to
the Copilot CLI preset; the VS Code render surface retains documented home paths. Use an explicit
directory target for a custom arrangement. See [translation](translation.md) for
instruction/MCP routes and surface-specific relocation.

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

## Read-only configuration inspection

`inspect_installation(bundle_id, installation_target)` reads the index and the
union of current, pending, previous and residual resource references, including
references in complete prepared index metadata. It does not
need the bundle source, create directories, acquire locks, recover transactions
or change permissions. Reads may update access times.

Its `InstallationObservation` includes recorded version/digests, pending state,
resource summaries and an error. Each resource summary contains claim attribution,
claim status, a revision digest, diagnostics and recovery paths. It excludes full
foreign content and baseline payloads.

A missing receipt required by a committed current or previous generation, including
a recognized prepared completion, is a resource `INVALID_STATE` error, with the
receipt path retained in the diagnostic.
It makes the installation noncurrent and blocks mutation without discarding index
membership. Pending first-install resources that never committed and valid retained
empty receipts are distinct from missing committed ownership metadata.

`is_current` means all recorded generation claims agree with their version,
bundle/render digests and actual owned content, with no unresolved membership or
recovery. It does not compare with an unavailable new application bundle. To check
that bundle, use `observation.matches(bundle, rendered)`. It requires a supported
rendering, matching bundle identity/version/content and render digest, and intact
recorded state. A same-label change to generated text or rendered destinations
returns false. The comparison is pure and remains a snapshot: inspect again to
observe later filesystem changes. Use `preview` to review proposed acquisitions
and retirements. Neither comparison verifies activation or prerequisites.

A preview is also read-only, but intentionally contains full preimages and
baselines. Treat it as sensitive explicit review data, not ordinary status output.
Changes to unowned bytes stale an existing preview even when owned claims remain
current.

## Skill convenience inspection

`inspect(bundle, targets)` returns an ordered tuple of `TargetInspection` values
for a nonempty iterable of skill `Target` values. Each carries the requested
target, canonical root, adjacent hashed authority `state_root`, `alias_of` and
an `Observation`. Same-physical aliases share an observation and point to the
first request's zero-based index.

If a target cannot be canonicalized, its observation is `UNKNOWN` with a typed
error, and independent targets are still inspected. Its `root` retains the
requested path; `state_root` is derived from that path for diagnostics. Physical
alias attribution requires successful canonicalization.

The skill adapter uses a deterministic sibling index named
`.<container>.flyrail-index-<bundle-id>`. Its recorded context binds the canonical
physical container. Equivalent directory and agent-preset routes can inspect,
update and remove that installation in later calls, regardless of their order.
The selected agent, scope, home and routing environment remain captured in the
preview. Ownership remains at the shared container authority.

`Observation` preserves the skill-facing shape: installed inventory summaries,
version/content comparisons, same-resource ownership, modifications, desired
conflicts, errors and recovery paths. Modification summaries identify changed
owned claims; use the general preview for the complete proposed resource change.
Desired-content conflicts remain in `conflicts`. Other preview failures appear in
`error` and make the state `UNKNOWN`, while an existing `RECOVERY_NEEDED` state
and its recovery paths are preserved.

`ObservationState` is `INSTALLED`, `ABSENT`, `RECOVERY_NEEDED` or `UNKNOWN`.
An uninstall observation has no desired bundle snapshot, so comparison fields are
null and its skill-facing `is_current` is false. Inventory entries expose paths,
file sizes/SHA-256 and executable intent, without file contents.

## Path and state safety

Canonicalization resolves physical aliases and rejects managed symlinks, reparse
points, hard links, special files and ambiguous portable names. Actual rendered
outputs and authority/index storage must not overlap retained bundle sources.
A project containing a bundle may still manage a disjoint document.

Malformed state, contradictory ownership or an inconsistent index fails closed.
Unknown transaction/preparation data is reported for recovery. Inspection may
observe an active writer; pending state is not proof that the writer stopped.
Only a locked mutation can perform [recovery](../../spec/transaction-protocol.md).

The [lifecycle](lifecycle.md) describes conflicts, replacement, aggregate outcomes
and source-free removal. [Resource state](../../spec/receipt-format.md) defines
the durable layout and strict encoding.
