# Public Rust API

[Bundles and rendering](#bundles-and-rendering) · [State stores](#state-stores) · [Configuration lifecycle](#configuration-lifecycle) · [Skill convenience API](#skill-convenience-api) · [Errors and results](#errors-and-results)

Import the supported surface from `flyrail`; the crate root re-exports the
consumer-facing types and functions. Bundles, renderings, previews, observations,
and results are owned snapshots. The library is synchronous, and the host
application owns destination selection, presentation, and replacement policy.

`detect_agents` reports `AgentPresence` values from an explicit or ambient
executable search path and supported macOS application directories. Evidence is
absolute and ordered by `Agent`; detection never chooses a rendering target.

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

`InstallationTarget::new(store, context, routing_context)` names one
installation: within `store`, the pair of bundle ID and `context` is its
identity. Reuse the same store and context for future inspection, updates,
recovery, and removal. `InstallationTarget::without_context(store)` names a
single installation per bundle in that store — two such targets sharing a
store collide. It records resource membership; each physical resource has its
own shared ownership authority, held in the same store. The separate
`routing_context` captures additional routing pairs in an immutable preview
and is not part of the persistent identity.

`RenderContext::installation(store, context)` builds a target from a render
context. Pass `None` for `context` to use the stable entries of that render
context's `routing_context` — `agent`, `scope`, `surface`, and `root`, per
`destinations.rs`'s `IDENTITY_KEYS` — so per-agent or per-scope installations
sharing one store stay separate, while a changed hook runtime, asset root,
platform, or environment entry keeps the same installation. Pass an explicit
`Some(context)` instead when an identity must survive a change to those
entries, such as a harness relocating a project root or user home.

## State stores

Flyrail keeps no persistent state of its own. Every index, receipt,
preparation, journal, and lock lives in a `StateStore` the application
constructs and passes explicitly — there is no default store and no default
location. The `StateStore` trait exposes `read`, `write`, `delete`, `keys`, and
`lock`; `write` and `delete` are compare-and-set against an `expected` value.
`lock` waits up to its timeout to acquire the lock and returns
`StoreError::Busy` when the wait runs out; the `StoreLock` it returns releases
the lock on drop. See the shared
[state store contract](../../spec/state-store.md) for every operation's exact
guarantees.

Two built-ins cover the common cases:

| Store | Use when |
| --- | --- |
| `DirectoryStore::new(path)` | Most applications; one file per record under `path`. |
| `MemoryStore::new()` | Tests and dry runs; nothing survives the process. |

SQLite-backed storage is Python-only; a Rust host that already keeps its own
state in SQLite implements `StateStore` itself.

A store decides what flyrail deletes and restores, so keep it private to the
user running flyrail. `DirectoryStore` refuses an existing location, staging
directory included, that is not private to its owner; lifecycle calls report
it as `IoError`.

`StateStore::location` names a store's filesystem footprint (`None` for
`MemoryStore`). `StateStore::staging_directory` names where flyrail stages its
writes (`<path>/staging` for `DirectoryStore`, `None` for `MemoryStore` and by
default for a custom store). A write stages there when the staging directory
is on the same mount as the destination — Linux compares `/proc/self/mountinfo`
mount IDs, macOS compares device IDs, and any doubt about the comparison
falls back — so the common setup, store and agent configuration both under
the user's home, creates nothing beside the user's files. Otherwise it stages
in a hidden `.flyrail-<hash>-<transaction-id>.txn` directory beside the
destination for the duration of the write. A destination, transaction
directory, or bundle source that overlaps the store's location or staging
directory is refused.

A custom store implements `StateStore` directly, using `StoreLock::new` to
build the guard its `lock` method returns, and must pass
`testing::check_store`. The kit checks single-process semantics through two
handles of one store: compare-and-set writes and deletes, value round-trips,
key listing and validation, and lock exclusion with bounded waits. It does
not verify cross-process lock exclusion or durability, which a store's own
tests must cover. It needs a store with nothing under the `conformance/` key
prefix and leaves the keys it writes there.

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

Call `summarize(&proposal)` to derive a `PlanSummary` with routing, notices,
errors, and create/update/delete/unchanged/conflict classifications for each
planned destination.

Lifecycle functions use an explicit `Acquisition`, `replace_modified` flag, and
`Duration` lock timeout where those choices apply:

```rust
use flyrail::{
    Acquisition, Agent, Artifact, Bundle, BundleIdentity, DirectoryStore, InstructionArtifact,
    OperationStatus, Platform, RenderContext, StateStore, Target, apply_preview,
    inspect_installation, preview, remove, render,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

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
let store: Arc<dyn StateStore> =
    Arc::new(DirectoryStore::new(root.join(".cache/project-guide"))?);
let target = context.installation(Arc::clone(&store), None)?;

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
skill-only bundles to the same resource lifecycle. Each takes a nonempty target
iterable and an explicit `store: &Arc<dyn StateStore>` and returns an ordered
vector, retaining alias attribution. Equivalent physical targets are
represented once and later entries expose `alias_of`.

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

A `StateStore` surfaces its own `StoreError`, which lifecycle calls convert
into a `TargetError`: `StoreError::Busy` becomes `ErrorCode::Busy`,
`StoreError::InvalidKey` becomes `ErrorCode::InvalidState`, and
`StoreError::Io` from any store keeps its errno and becomes
`ErrorCode::IoError`, except that `ENOTSUP`, `ENOSYS`, `EXDEV`, and `EINVAL`
become `ErrorCode::Unsupported` instead.

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
