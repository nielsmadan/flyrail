# Detect agents and install per agent

`detect_agents` reports which coding agents a host can plausibly use; the rest of
this walkthrough renders, applies, updates and removes a bundle for the agents the
developer chooses to offer. Flyrail never selects agents, prints or prompts on its
own — every decision below is the caller's.

```python
from flyrail import detect_agents

found = detect_agents()
```

`found` is a tuple of `AgentPresence(agent, executable, application)`, one entry per
`Agent` with at least one piece of evidence: a resolved binary on `PATH`, a matching
`.app` bundle under `/Applications` or `~/Applications`, or both. `path=` and
`applications=` override the search, which is what lets tests and this example run
without depending on what happens to be installed on a given machine.

Evidence is not proof of usability, and a developer offering agents from `found`
should account for three gaps:

- `pi` is a generic binary name. A resolved executable can be unrelated tooling that
  happens to sit on `PATH` under that name, not the Pi coding agent.
- A present `Visual Studio Code.app` shows the editor is installed, not that its
  Copilot extension is. `AgentPresence(Agent.COPILOT, application=...)` only ever
  means the application bundle exists.
- Application-bundle detection is macOS-only. A Cursor or VS Code install on Linux
  with no CLI shim on `PATH` produces no evidence at all, so `detect_agents` omits
  it rather than reporting a false negative some other way.

## One target per agent

Render and install into a separate `RenderContext` and `InstallationTarget` for
each agent the developer offers. Separate targets are what make later per-agent
removal possible: removing one agent's target can never touch another agent's
resources, even when both were rendered from the same bundle into the same
project. The rest of this walkthrough offers two agents that `found` can
report evidence for, Claude and Codex:

```python
from pathlib import Path

from flyrail import (
    Agent,
    Bundle,
    BundleIdentity,
    Command,
    InstructionArtifact,
    McpArtifact,
    Platform,
    RenderContext,
    Target,
)

root = Path("project").resolve()
bundle = Bundle.from_artifacts(
    BundleIdentity("project-tools", "autumn"),
    [
        InstructionArtifact("guide", "Run the project checks before committing.\n"),
        McpArtifact("tools", "tools", Command(["project-server"])),
    ],
)

offered = (Agent.CLAUDE, Agent.CODEX)
contexts = {agent: RenderContext(Target.project(agent, root), Platform.LINUX) for agent in offered}
targets = {
    agent: context.installation(root / ".cache" / "project-tools" / agent.value)
    for agent, context in contexts.items()
}
```

A real host offers whichever agents `found` actually reported; keeping two agents
here keeps the example free of a separate concern: Codex, Cursor, OpenCode and Pi
all default to `AGENTS.md` at the project root, and Copilot to `.mcp.json`
alongside Claude. Applying one agent's preview changes that shared file on disk, and
`apply_preview` rejects any other already-built preview whose precondition that
apply changed (see [Installation, update and removal](lifecycle.md)) — so a host
offering several agents that share a destination should apply promptly after
previewing, or build a fresh preview immediately before each apply.

`render_many` and a single shared `InstallationTarget` also exist, for an
application that wants one atomic installation covering several agents at once
(see [translation](translation.md)). The trade-off is per-agent granularity: a
shared target synchronizes and removes as one unit, so an application that needs
independent per-agent removal should keep separate targets as above.

## Render, preview, summarize

`render` is pure; `preview` reads the target and reports what applying it would do,
without writing anything. `summarize` turns that `LifecyclePreview` into a
`PlanSummary` a host can display without reaching into lifecycle internals.

```python
from flyrail import preview, render, summarize

previews = {
    agent: preview(bundle, render(bundle, context), targets[agent])
    for agent, context in contexts.items()
}
for agent, proposal in previews.items():
    plan = summarize(proposal)
    print(f"{agent.value}: applicable={plan.applicable}")
    for change in plan.changes:
        print(f"  {change.action.value:>9} {change.destination}")
```

Each `PlannedChange` carries the destination and a `ChangeAction`: `CREATE`,
`UPDATE`, `DELETE`, `UNCHANGED` or `CONFLICT`. Several rendered artifacts can share
one destination, so `artifact_ids` and `families` are parallel tuples covering all
of them, ordered by artifact id; both are empty when no rendered artifact matches
the resource, which is normal for a removal plan. A `CONFLICT` change carries the
resource's own `error` and `recovery_paths`, which is what a host needs to say
what conflicts; `PlanSummary.error` reports the target as a whole and is set
independently:

```python
for change in summarize(previews[Agent.CLAUDE]).changes:
    print(change.action.value, ",".join(change.artifact_ids) or "-", change.destination)
    if change.action.value == "conflict":
        print("  ", change.error, change.recovery_paths)
```

`PlanSummary.agent`, `.scope` and `.surface` come from the preview's
routing context and are `None` for a `Target.directory` installation.

## Deselecting an agent needs no API

If the developer shows these plans to a user and the user unchecks Codex, nothing
in Flyrail filters that out. The caller simply never calls `apply_preview` on
Codex's preview:

```python
selected = {agent: proposal for agent, proposal in previews.items() if agent.value != "codex"}
```

`selected` now holds only Claude's preview. Codex's preview is a plain immutable
value with no lifecycle effect until something applies it; leaving it unapplied and
letting it go out of scope is the entire mechanism.

## Apply and read the result

```python
from flyrail import apply_preview

results = {agent: apply_preview(proposal) for agent, proposal in selected.items()}
for agent, result in results.items():
    print(agent.value, result.status.value, result.error)
    for resource in result.resources:
        print(" ", resource.resource.destination, resource.status.value)
```

Check `result.status` before trusting anything else: `APPLIED` and `UNCHANGED` are
the two success outcomes, `PARTIAL` means some resources committed before another
failed, and `FAILED`/`INCOMPLETE` mean check `result.error` and each resource's own
`status`/`error`/`recovery_paths`. A committed resource can still need cleanup, so
`resources` is worth reading even after a successful `status`.

## Notices: activation and prerequisites

Rendering an MCP artifact always attaches an `ACTIVATION` notice under the code
`mcp-activation`: registering a server is not the same as the host trusting it,
approving its tools, supplying its credentials or having started it. Pi's
translation adds a second, `PREREQUISITE` notice, because Pi needs an external
adapter Flyrail cannot install:

```python
from flyrail import NoticeKind

pi_context = RenderContext(Target.project("pi", root), Platform.LINUX)
pi_rendered = render(bundle, pi_context)
activation = [n for n in pi_rendered.notices if n.kind is NoticeKind.ACTIVATION]
prerequisite = [n for n in pi_rendered.notices if n.kind is NoticeKind.PREREQUISITE]
for notice in activation + prerequisite:
    print(notice.kind.value, notice.code, notice.message)
```

```
activation instruction-loading Instruction discovery and loading are controlled by the host and current session.
activation mcp-activation Registration does not grant trust, approve tools, supply credentials or prove server activation.
prerequisite pi-mcp-adapter-v2-32-1 Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1.
```

`PlanSummary.notices` carries the same notices for a bundle that was already
previewed, so a host does not need to re-render just to display them.

## Update

`inspect_installation(...).is_current` tells the caller whether the installed
generation still matches what was last recorded, before spending the work of
building a fresh bundle. Updating is the same render/preview/summarize/apply loop
run again against the same target, with new content:

```python
from flyrail import inspect_installation

claude = next(agent for agent in selected if agent.value == "claude")
observation = inspect_installation(bundle.id, targets[claude])
observation.is_current  # True: nothing has changed since the last apply

updated = Bundle.from_artifacts(
    BundleIdentity("project-tools", "autumn"),
    [
        InstructionArtifact("guide", "Run the project checks before committing, twice.\n"),
        McpArtifact("tools", "tools", Command(["project-server"])),
    ],
)
updated_preview = preview(updated, render(updated, contexts[claude]), targets[claude])
summarize(updated_preview).changes  # UPDATE for both CLAUDE.md and .mcp.json
apply_preview(updated_preview)
```

## Removal

Removal follows the same preview/summarize/apply shape, using only the bundle ID
and the target being removed:

```python
from flyrail import preview_removal

removal = preview_removal(updated.id, targets[claude])
summarize(removal).changes  # DELETE for both CLAUDE.md and .mcp.json
apply_preview(removal)
```

Only Claude's resources are affected. Codex's preview was built and shown
earlier, but it was filtered out of `selected` and never reached
`apply_preview`, so its target still holds whatever it held before this
section — and removing Claude's target cannot reach it regardless, since the
two targets never shared an index. That separation from
[one target per agent](#one-target-per-agent) is what let the developer offer,
apply and now remove agents independently.
