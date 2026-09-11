# Installation, update and removal

```python
from flyrail import Bundle, OperationStatus, Target, install, uninstall, update

bundle = Bundle.from_directory("./my-bundle")
targets = [Target.directory("./agent-skills")]
installed = install(bundle, targets)
updated = update(bundle, targets, lock_timeout=0.25)
removed = uninstall(bundle.id, targets)

for result in updated:
    if result.status is OperationStatus.APPLIED:
        installation = result.observation.installed
```

All three calls are synchronous and return a tuple of immutable `TargetResult`
values, one per requested target in request order. Targets are explicit; Flyrail
does not select agents, prompt, print, exit, access the network, or execute skill
scripts. Bundle snapshots supply all installation bytes, even after their source
has been removed. Uninstall only needs the bundle ID and targets.

## Behavior and arguments

| Call | Behavior |
| --- | --- |
| `install(bundle, targets, *, lock_timeout=0)` | Installs an absent bundle; an intact current revision is unchanged. Another owned revision requires `update`. |
| `update(bundle, targets, *, replace_modified=False, lock_timeout=0)` | Synchronizes the supplied version label and complete skill content; may install an absent bundle. Removes retired owned skills. |
| `uninstall(bundle_id, targets, *, replace_modified=False, lock_timeout=0)` | Removes the recorded bundle's owned skills and publishes a removal receipt. An absent bundle is unchanged. |

Version labels are opaque equality values. An explicit update can move from `"9"`
to `"1"`, or change content while retaining a label. A label-only update publishes
a new receipt without rewriting intact skills. Skills whose observed inventory
already matches the requested content also retain their files and directory
identity; the receipt still changes when its label or recorded content differs.

Targets must be a nonempty iterable of `Target` values. The entire iterable,
argument types, canonical destinations, aliases and source/state/target overlap
are validated before the first write. Incorrect types raise `TypeError`; invalid
values raise `ValueError`. `replace_modified` must be a boolean. `lock_timeout`
must be a finite, nonnegative number of seconds; booleans are rejected. Default
zero attempts acquisition once. A positive timeout bounds explicit lock waiting;
it does not impose a deadline on file copying, inspection or recovery.

Physical aliases are processed once and retain the original request attribution.
Independent targets continue after a conflict, busy lock or filesystem failure.
Successful targets stay applied when another fails. Removing a shared target
removes the physical installation for every consumer of that directory.

## Ownership and local edits

Untracked skill roots and other bundles' owned roots are conflicts, including
byte-identical or currently missing foreign-owned content. All receipts are read
and reconciled before ownership is trusted. Malformed or contradictory receipts,
unsafe paths, symlinks, reparse points and special files fail closed.

Changed, missing, added or type-changed content in any owned skill blocks the
requested bundle's entire operation in that target. This includes retired skills.
`replace_modified=True` permits update/removal of the exact safe same-owner
revision observed for this call, including added files. It cannot acquire foreign
ownership or authorize following a symlink. The observed inventory is checked
again during preparation and publication. Ordinary edits in another bundle do
not block a disjoint operation; unsafe paths and contradictory ownership do.

Whole skill directories are published using exclusive moves. Unrelated content,
other bundles, the shared skill container and permanent lock are retained. POSIX
files receive executable mode `0755` or nonexecutable mode `0644` according to the
bundle. Other permission bits and timestamps are outside content identity.
Windows retains logical executable intent in receipts.

New management directories are private: mode `0700` on POSIX, with each fresh
staging and backup root protecting its contents even under permissive existing
management parents. Existing directory permissions and the lock inode are retained.
Windows mutations require Python 3.11.10+, 3.12.4+, or 3.13+ for native private
directory creation; earlier patches return `UNSUPPORTED` before creating state.
See [platform support](support.md) for the standard-library contracts.

## Results

| Field | Meaning |
| --- | --- |
| `target`, `root`, `state_root`, `alias_of` | Same attribution and physical-target semantics as [inspection](inspection.md). |
| `status` | `APPLIED`, `UNCHANGED`, `FAILED`, or `INCOMPLETE`. |
| `observation` | Best-effort observation after the operation/recovery attempt; contains installed revision, content comparisons, modifications and conflicts. |
| `error` | Operation-level `TargetError`, or `None`. Distinct from `observation.error`. |
| `recovery_paths` | Pending transaction/staging/backup paths to retain, or an empty tuple. |

`APPLIED` means this call published its new receipt, including an uninstall
tombstone. Cleanup can remain pending: check both `error` and `recovery_paths`,
and retain those paths. Failures in cleanup, final observation or lock release
preserve this committed status. A committed operation is never rolled back because
cleanup failed. A later mutation recovers pending cleanup under the lock before
starting its own requested work. If that cleanup remains unsafe, the later call
returns `INCOMPLETE` without claiming it applied the earlier operation.

`UNCHANGED` means the requested state already holds; recovery may have finished
before reaching that conclusion. `FAILED` means this call did not commit and has
no unresolved transaction work. `INCOMPLETE` means recovery/preparation data
remains and the requested call could not complete. Target aliases share the same
status, observation, error and recovery paths.

An uninstall observation has no desired bundle snapshot: `version_matches`,
`recorded_content_matches` and `content_matches` are `None`, and `is_current` is
false. Its `installed` and `state` describe whether an active receipt remains.

Operation error codes include `CONFLICT`, `MODIFIED`, `UPDATE_REQUIRED`, `BUSY`,
`UNSUPPORTED`, and the inspection codes `IO_ERROR`, `UNSAFE_PATH`, `INVALID_STATE`,
`CONCURRENT_CHANGE`, `RECOVERY_NEEDED`. Errors retain available OS errno and path.
If a new operation fails and its recovery also fails, the original error code is
retained and the message includes the recovery failure. Observation can succeed
while the operation fails, for example when an update is required.

Read the [transaction protocol](../../spec/transaction-protocol.md) before interpreting or
moving pending data. Inspection reports pending work without locking or recovery;
it may be observing an active writer.
