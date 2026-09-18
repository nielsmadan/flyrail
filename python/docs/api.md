# Public Python API

[Bundles and rendered content](#bundles-and-rendered-content) · [Configuration lifecycle](#configuration-lifecycle) · [Skill convenience API](#skill-convenience-api) · [Errors and host handling](#errors-and-host-handling)

Import supported names from `flyrail`. Bundles, previews, observations and results
are immutable snapshots. The library is synchronous; the application chooses
destinations, presents results and decides when to request replacement.

## Bundles and rendered content

`Bundle.from_directory`, `from_package`, `from_zip`, `from_memory` and
`from_artifacts` detach content from its source. Package loading follows normal
Python import semantics. Bundle labels are opaque and independent of application
and library versions. Content digests detect changes under the same label.

[Configuration](configuration.md) describes skills, instructions, MCP, hooks,
support assets and native payloads. The general lifecycle consumes explicit
`RenderedBundle` values containing absolute destinations and file, tree, section
or structured content. [Agent-specific translation](translation.md) is a pure layer: `render` accepts an
explicit `RenderContext`; `render_many` combines several contexts for one logical
installation. `capabilities` distinguishes portable semantics and native routing.
Unsupported notices block lifecycle mutation before lock/recovery admission.

`InstallationTarget` names a private logical index and snapshots string context
pairs. Reuse the same index and context for future inspection, updates and removal.
Choose one index per bundle installation. It records resource membership; each
physical resource has its own shared ownership authority.

The optional keyword-only `routing_context` captures additional routing pairs in
an immutable preview. It does not change the persistent index identity. Use
`context` for installation identity and `routing_context` for the route selected
for a particular call.

## Configuration lifecycle

| API | Purpose |
| --- | --- |
| `preview` | Observe a bundle/rendering/target and return an immutable proposed change. |
| `preview_removal` | Propose removal using only bundle ID and index. |
| `apply_preview` | Apply exactly those preconditions; reject stale input. |
| `recover_installation` | Recover recognized interrupted work using only bundle ID and index, then re-observe before planning. |
| `sync` | Recover recognized interrupted work, plan afresh under locks and synchronize. |
| `inspect_installation` | Read recorded membership and actual owned content without source bytes or mutation. |
| `remove` | Recover and remove recorded ownership without the bundle source. |

Full signatures and public result types are in
[configuration.py](../src/flyrail/configuration.py) and
[lifecycle.py](../src/flyrail/lifecycle.py). A generated instruction example:

```python
from pathlib import Path

from flyrail import (
    Bundle,
    BundleIdentity,
    Family,
    InstallationTarget,
    InstructionArtifact,
    RenderedArtifact,
    RenderedBundle,
    SectionContent,
    inspect_installation,
    preview,
    apply_preview,
    remove,
)

root = Path("project").resolve()
bundle = Bundle.from_artifacts(
    BundleIdentity("project-guide", "autumn"),
    [InstructionArtifact("guide", "Run the project checks.\n")],
)
rendered = RenderedBundle(
    [
        RenderedArtifact(
            "guide",
            Family.INSTRUCTIONS,
            root / "AGENTS.md",
            SectionContent("project-guide", "Run the project checks.\n"),
        ),
    ]
)
target = InstallationTarget(root / ".cache" / "project-guide")
proposal = preview(bundle, rendered, target)
result = apply_preview(proposal)
observation = inspect_installation(bundle.id, target)
configured_current = observation.matches(bundle, rendered)
removed = remove(bundle.id, target)
```

The example can create AGENTS.md. Set `require_existing=True` on its rendered
artifact when an application must only modify an existing document.

`recover_installation(bundle_id, target, *, lock_timeout=0)` returns an
`InstallationResult`. It recovers resource transactions and recognized index
preparations under the same locks as `sync`, without applying a desired bundle or
removing healthy ownership. `APPLIED` reports completed recovery work and
`UNCHANGED` reports that no recovery was needed. An absent index is a no-op.
Pending/residual logical membership can remain after successful recovery and is
reconciled by the next update or removal. Check errors and recovery paths, re-read
application selection conditions, and build a fresh `preview` or `preview_removal`
before `apply_preview`. A preview captured before recovery remains stale.

`inspect_installation(...).is_current` compares the installed generation with
its recorded claims and actual content. `observation.matches(bundle, rendered)`
also checks bundle identity, version, content digest and render digest, and requires
supported rendering. This pure comparison uses the captured observation without
reading files, resolving environment variables or checking host readiness.
A preview checks proposed acquisitions and retirements
as well. [Lifecycle](lifecycle.md) explains these distinctions and replacement.

## Skill convenience API

`Target.directory`, `Target.project` and `Target.user` select skill containers.
`install`, `update`, `uninstall` and `inspect` adapt skill-only bundles to the
same resource lifecycle. They accept a nonempty target iterable and return ordered
tuples, retaining alias attribution. They do not translate non-skill artifacts.
Expected filesystem failures produce results for the affected targets; independent
targets continue in request order. Invalid arguments and unsupported skill bundle
contents are validated before mutation.
Bundles with support assets or dependencies raise `ValueError` before mutation.
Use `render` with an explicit `RenderContext` and the configuration lifecycle for
those bundles so every dependency has a destination.

`install` requires `update` when an existing bundle has another version or digest.
`update` can also install an absent bundle. `uninstall` needs only its ID and
targets. See [destinations and observations](inspection.md).

## Errors and host handling

An `InstallationResult` has aggregate status, final observation, per-resource
results, notices and an optional operation error. Check all of them:
`PARTIAL` means resources committed before another step failed;
`INCOMPLETE` means unresolved recovery work remains. An individual resource can
be committed while its cleanup still reports an error.

`TargetError` carries a code, message, optional path and OS errno. Expected
filesystem/conflict/recovery failures become results. Invalid public arguments
raise `TypeError` or `ValueError`; source I/O, import and archive errors retain
their normal exceptions. A finite nonnegative `lock_timeout` bounds lock waiting,
not the duration of the operation.

`preview` and `preview_removal` return a non-applicable `LifecyclePreview` with a
`TargetError` when filesystem or index observation fails. If observation of the
overall preview fails, `preview.error` carries the failure and there is no
`index_revision` or resource plan; absent evidence does not mean an empty index.
Resource-specific planning failures retain their observed plans and per-resource
errors. `apply_preview` returns an overall preview error without mutation, even if
the filesystem has since been repaired. Create a fresh preview after resolving the error.

`sync` also returns a failed result when initial destination resolution encounters
an expected filesystem error, including an unsafe link. Invalid request types and
destination/source overlaps still raise their normal exceptions.

Ordinary observations expose claim attribution, versions, digests and diagnostics.
Full foreign bytes and restoration baselines are reserved for explicit previews
and private state. Do not serialize a preview as ordinary CLI status output.
