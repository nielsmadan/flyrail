# Public Python API

Import supported names from `flyrail`. Public values and returned collections are
immutable: tuples, frozen value objects and bytes. Flyrail is synchronous; the
host application chooses destinations, schedules calls, presents results and
decides whether to authorize replacement. There is no Flyrail console command.
The [example CLIs](examples.md) show that host integration using the public API.

## Loading and selecting

| API | Result and use |
| --- | --- |
| `Bundle.from_directory(root)` | Snapshot a filesystem bundle containing `flyrail.json`. |
| `Bundle.from_package(package, resource="flyrail")` | Snapshot an importable package's resources, including supported ZIP resources. `package` is an import name or module. Normal package import semantics apply. |
| `Target.directory(root)` | Select the skill container itself. |
| `Target.project(agent, root)` | Select a documented project-relative skill container. |
| `Target.user(agent, *, home=None, env=None)` | Select a documented user skill container with optional isolated overrides. |

`Bundle` exposes `id`, `version`, `identity`, `skills`, `entries`, `content_digest`,
and `source_roots`. `BundleIdentity(id, version)` and
`SkillSpec(name, path, executables=())` describe validated metadata.
`BundleEntry(path, data=None, executable=False)` represents an installed-relative
directory or file; `data=None` is a directory and `data=b""` is an empty file.
Its `is_directory` property makes that distinction explicit. Use the loaders to
construct a complete validated bundle; these metadata classes alone do not read
or package a source. The [bundle specification](../../spec/bundle-format.md) defines exact
validation, snapshot and digest behavior.

`Agent` contains `CLAUDE`, `CODEX`, `OPENCODE`, `PI`, `CURSOR` and `COPILOT`;
constructors also accept their lowercase string values. `TargetScope` identifies
`USER`, `PROJECT` or `DIRECTORY`. Targets expose `root`, `agent`, and `scope`.
The [destination reference](inspection.md) covers exact paths, environment
precedence, aliases, canonicalization and overlap rejection.

## Inspecting and changing installations

| Call | Return type |
| --- | --- |
| `inspect(bundle, targets)` | `tuple[TargetInspection, ...]` |
| `install(bundle, targets, *, lock_timeout=0)` | `tuple[TargetResult, ...]` |
| `update(bundle, targets, *, replace_modified=False, lock_timeout=0)` | `tuple[TargetResult, ...]` |
| `uninstall(bundle_id, targets, *, replace_modified=False, lock_timeout=0)` | `tuple[TargetResult, ...]` |

Pass a nonempty iterable of `Target` values. The whole request is validated before
writes. Results preserve request order. Physical aliases share an operation and
observation, with `alias_of` pointing to the first request's zero-based index.
There is no rollback of successful destinations when another destination fails.
Lock waiting defaults to a single nonblocking attempt; a positive finite timeout
bounds lock acquisition, not the duration of the complete operation.

Inspection observes without creating directories, acquiring locks or recovering
state. Use `result.observation.is_current` for the common update check. It requires
intact ownership and matching version, recorded content and actual content, with
no relevant conflict or recovery error. An uninstall result has no desired bundle
to compare against, so its `is_current` is false even after successful removal.

Installation is idempotent for a current revision. Another owned revision needs
an explicit update, which can also install an absent bundle and remove retired
skills. Uninstall reads receipts, so the source package or bundle directory is
unnecessary. Same-owner local modifications block a target by default.
`replace_modified=True` permits replacement/deletion of the observed same-owner
revision, including added files, subject to revalidation. It never permits taking
foreign ownership or following managed symlinks.

## Results and errors

`TargetInspection` and `TargetResult` carry the requested `target`, resolved
`root`, sibling `state_root`, `alias_of`, and an `Observation`. `TargetResult`
also carries `status: OperationStatus`, operation-level `error`, and `recovery_paths`.

| Status | Meaning |
| --- | --- |
| `APPLIED` | This call published its receipt, including a removal tombstone. Cleanup or final observation can still report an error. |
| `UNCHANGED` | The requested state already holds; prior recovery may have completed first. |
| `FAILED` | This call did not commit and has no unresolved transaction work. |
| `INCOMPLETE` | The requested call could not finish and recovery/preparation data remains. |

For example, a host can retain every result for presentation and separately decide
whether the operation needs attention:

```python
from flyrail import Bundle, OperationStatus, Target, TargetResult, update


def needs_attention(result: TargetResult) -> bool:
    return (
        result.status in {OperationStatus.FAILED, OperationStatus.INCOMPLETE}
        or result.error is not None
        or result.observation.error is not None
        or bool(result.recovery_paths)
    )


bundle = Bundle.from_directory("examples/filesystem/bundle")
results = update(bundle, [Target.directory(".cache/demo-skills")])
attention = tuple(result for result in results if needs_attention(result))
```

Do not interpret `APPLIED` as proof that cleanup finished or immediately retry it
as a new install. Preserve reported paths and present the committed result. A
later explicit mutation first attempts safe recovery under the lock. Unknown
preparation debris or edited backup data can require manual examination; there
is no public force-recovery or garbage-collection operation.

An `Observation` separates installation state, requested-version equality,
recorded-content equality, actual-content equality, modifications, conflicts,
observation errors and recovery paths. `ObservationState` is `ABSENT`,
`INSTALLED`, `RECOVERY_NEEDED` or `UNKNOWN`. `Installation` describes a receipt and
its `InventoryEntry` tuple. `Modification` identifies a bundle/path and a
`ModificationKind`; `Conflict` identifies a path and optional owning bundle ID.
Their complete fields and enum meanings are in [inspection](inspection.md).
Operation outcomes and recovery semantics are detailed in [lifecycle](lifecycle.md).

`TargetError` has an `ErrorCode`, readable `message`, optional absolute `path` and
optional OS `errno`. Expected destination failures are reported per target:

| Codes | Host response |
| --- | --- |
| `UPDATE_REQUIRED` | Offer an explicit update to the supplied revision. |
| `MODIFIED` | Present local changes and obtain the host user's decision before using `replace_modified=True`. |
| `CONFLICT` | Explain the foreign/untracked skill root; replacement authorization cannot acquire it. |
| `BUSY` | Report lock contention or retry later with bounded waiting. |
| `RECOVERY_NEEDED`, `INVALID_STATE` | Preserve state and recovery paths; inspect the protocol before any manual repair. |
| `UNSAFE_PATH`, `UNSUPPORTED` | Choose a supported safe target/filesystem; no weaker fallback is performed. |
| `CONCURRENT_CHANGE`, `IO_ERROR` | Present the path/error and re-observe after the external cause is resolved. |

Programmer and source errors fail upfront rather than appearing as target
outcomes. Incorrect argument types raise `TypeError`; invalid metadata, paths,
values, manifests and unsupported source layouts raise `ValueError`. Source
filesystem failures propagate their `OSError` subclasses; package-import and
corrupt-ZIP errors retain their standard-library types. A host should catch and
present these at its command boundary. Bundles can import application package
initializers while loading resources; Flyrail never executes skill scripts.
