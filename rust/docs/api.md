# Public Rust API

[Bundles and rendering](#bundles-and-rendering) · [Configuration lifecycle](#configuration-lifecycle) · [Skill convenience API](#skill-convenience-api) · [Errors and results](#errors-and-results)

Import the supported surface from `flyrail`; the crate root re-exports the
consumer-facing types and functions. Bundles, renderings, previews, observations,
and results are owned snapshots. The library is synchronous, and the host
application owns destination selection, presentation, and replacement policy.

## Bundles and rendering

`Bundle::from_directory`, `Bundle::from_zip`, `Bundle::from_memory`,
`Bundle::from_memory_value`, and `Bundle::from_artifacts` detach content from its
source. Bundle labels are opaque equality values independent of application and
crate versions. `Bundle::content_digest` detects changed content under the same
label.

Schema-2 bundles can contain `SkillArtifact`, `InstructionArtifact`,
`McpArtifact`, `HookArtifact`, and `NativeArtifact` values plus `SupportAsset`
trees and `Dependency` edges. The pure `render` and `render_many` functions turn
those values into a `RenderedBundle` for explicit `RenderContext` values. Check
`RenderedBundle::supported` and every `Notice`; an unsupported rendering blocks
lifecycle mutation before lock or recovery admission.

`InstallationTarget` names one private logical index. Keep the same index root and
identity context for future inspection, updates, recovery, and removal. Its
routing context records how a particular rendering was selected but is not part
of the persistent index identity.

## Configuration lifecycle

| Function | Purpose |
| --- | --- |
| `preview` | Observe a bundle, rendering, and target and return an immutable proposal. |
| `preview_removal` | Propose source-free removal using a bundle ID and index. |
| `apply_preview` | Apply the captured proposal only if every precondition still matches. |
| `recover_installation` | Recover recognized interrupted work without applying new desired content. |
| `sync` | Recover recognized work, plan afresh under locks, and synchronize. |
| `inspect_installation` | Read recorded membership and owned content without mutation. |
| `remove` | Remove recorded ownership without the source bundle. |

Lifecycle functions use an explicit `Acquisition`, `replace_modified` flag, and
`Duration` lock timeout where those choices apply:

```rust
use flyrail::{
    Acquisition, Agent, Artifact, Bundle, BundleIdentity, InstructionArtifact,
    OperationStatus, Platform, RenderContext, Target, apply_preview,
    inspect_installation, preview, remove, render,
};
use std::{path::PathBuf, time::Duration};

let root = PathBuf::from("project").canonicalize()?;
let bundle = Bundle::from_artifacts(
    BundleIdentity::new("project-guide", "autumn")?,
    [Artifact::Instruction(InstructionArtifact::new(
        "guide",
        "Run the project checks.\n",
    )?)],
    [],
    [],
)?;
let context = RenderContext::new(Target::project(Agent::Codex, &root)?, Platform::Linux)?;
let rendered = render(&bundle, &context)?;
let target = context.installation(root.join(".cache/project-guide"))?;

let proposal = preview(&bundle, &rendered, &target, Acquisition::Conflict, false)?;
let result = apply_preview(&proposal, Duration::ZERO);
assert!(matches!(
    result.status(),
    OperationStatus::Applied | OperationStatus::Unchanged
));

let observed = inspect_installation(bundle.id(), &target)?;
assert!(observed.matches(&bundle, &rendered));
let _removed = remove(bundle.id(), &target, false, Duration::ZERO)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`InstallationObservation::is_current` compares installed claims with the recorded
generation and current resources. `InstallationObservation::matches` additionally
checks bundle identity, label, bundle digest, render digest, and rendering support.
It is a pure comparison against the captured observation.

## Skill convenience API

`Target::directory`, `Target::project`, and the `Target::user*` constructors
select skill containers. `inspect`, `install`, `update`, and `uninstall` adapt
skill-only bundles to the same resource lifecycle and return ordered vectors.
Equivalent physical targets are represented once and later entries expose
`alias_of`.

`install` refuses a different owned label or digest and requires an explicit
`update`. `update` also installs an absent bundle. Use `SkillOptions` with
`update_with_options` or `uninstall_with_options` to authorize replacement of a
modified same-owner unit or to set a lock timeout. Invalid arguments fail before
mutation; expected target failures are returned per target and independent
targets continue in request order.

The convenience API rejects non-skill artifacts and skill bundles with assets or
dependencies. Use explicit rendering and the configuration lifecycle for those
bundles.

## Errors and results

Construction and validation generally return `ValueError`; source loading returns
`BundleError`; path authority construction returns `AuthorityError`. Expected
filesystem, conflict, and recovery failures are represented by `TargetError` in
previews, observations, and results.

An `InstallationResult` contains an aggregate `OperationStatus`, final
`InstallationObservation`, per-resource results, notices, and an optional error.
Inspect all of them:

- `Applied`: requested resource work committed.
- `Unchanged`: the requested state already held.
- `Partial`: at least one resource committed before another step failed.
- `Failed`: no requested resource commit completed, or recovery failed.
- `Incomplete`: unresolved recovery evidence remains.

A resource may remain `Applied` when only its post-commit cleanup failed. Preserve
reported recovery paths. A finite lock timeout bounds waiting for locks, not the
whole operation.

Ordinary observations expose attribution, digests, statuses, diagnostics, and
recovery paths. Previews intentionally retain complete preimages and restoration
baselines; do not serialize them as routine status output.
