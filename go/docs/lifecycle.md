# Installation, update, and removal

[Planning and applying](#planning-and-applying) · [Results and errors](#results-and-errors) · [Contexts and lock timeouts](#contexts-and-lock-timeouts) · [Ownership and replacement](#ownership-and-replacement) · [Current state](#current-state-and-results) · [Filesystem boundary](#filesystem-boundary)

Flyrail synchronizes an immutable `*Bundle` and explicit `*RenderedBundle` into a
logical `*InstallationTarget`, backed by the application's `store.Store`. One
receipt owns each physical document, tree, or skill container across callers
and bundles, within that store. A separate private index, held in the same
store, locates the resources belonging to one installation, including
interrupted or retired claims. Authority records are permanent within a
store: nothing ever deletes one, so a failed child admission keeps excluding
an enclosing tree, and a failed tree admission keeps excluding every
descendant it already reserved.

## Planning and applying

`Preview` and `PreviewRemoval` are read-only. They capture desired content,
context, complete resource revisions, security metadata, ancestors, receipts,
index state, and recovery evidence. `LifecyclePreview.Applicable()` is false
when known errors or pending recovery prevent application.

`Summarize` converts a preview into a presentation-oriented `PlanSummary`. Each
`PlannedChange` maps a destination to its artifact IDs and families and classifies
it as create, update, delete, unchanged, or conflict. Routing, notices, errors,
and recovery paths remain available in the summary.

```go
preview, err := flyrail.Preview(ctx, bundle, rendered, installation, flyrail.PreviewOptions{})
if err != nil {
	return err
}
if !preview.Applicable() {
	if failure := preview.Err(); failure != nil {
		return failure
	}
	return errors.New("preview is not applicable")
}
result := flyrail.ApplyPreview(ctx, preview, flyrail.LockOptions{LockTimeout: time.Second})
if err := result.Failure(); err != nil {
	return err
}
observed, err := flyrail.InspectInstallation(ctx, bundle.ID(), installation)
```

`preview.Err()` returns a `*TargetError`. Check it against nil before returning
it as an `error`; see [Results and errors](#results-and-errors).

`ApplyPreview` rejects any changed precondition, including foreign bytes outside
an owned section. It never refreshes a stale proposal. `Sync` and `Remove`
instead recover recognized interrupted work under locks and plan again.

Call `RecoverInstallation` before reading files whose post-recovery state chooses
new desired content. It recovers recognized interrupted resource transactions
without publishing new content or removing healthy ownership; pending and
residual membership in the index stays for the next mutation to reconcile.
Re-read selection inputs and create a fresh preview afterward.

Known conflicts across the logical target are checked before publication.
Resources then commit independently in dependency order. A later failure
preserves earlier successful commits and pending membership for retry or
source-free removal.

`RenderedArtifact.RequireExisting()` prevents creation of an absent destination
and binds the observed file through publication.

The skill convenience functions `Install`, `Update`, `Uninstall` and `Inspect`
take `[]Target` and return one result per target, in request order. They accept
only skill-only bundles without support dependencies.

## Results and errors

Go reports problems in two places.

- The `error` return of a lifecycle function is for invalid arguments: an
  empty target list, a nil or non-pointer store, a nil context, a negative
  `LockTimeout`, a non-skill bundle. It is a `*ValueError`, `*BundleError` or
  `*AuthorityError`; test with `errors.As`.
- Everything that happens to a target, such as a conflict, an I/O failure or an
  interrupted earlier write, is in the result as a `*TargetError` with a
  `Code`, not as the function's `error`.

`InstallationResult`, `TargetResult` and `ResourceResult` have two helpers so a
CLI does not have to inspect the `Err` field:

```go
results, err := flyrail.Install(ctx, bundle, targets, st, flyrail.LockOptions{LockTimeout: 5 * time.Second})
if err != nil {
	var invalid *flyrail.ValueError
	if errors.As(err, &invalid) {
		return fmt.Errorf("bad arguments: %w", err)
	}
	return err
}
for _, result := range results {
	if err := result.Failure(); err != nil {
		var failure *flyrail.TargetError
		if errors.As(err, &failure) && failure.Code == flyrail.CodeBusy {
			return fmt.Errorf("another flyrail process holds %s: %w", result.Root, err)
		}
		return err
	}
}
```

`Succeeded()` is true exactly for `StatusApplied` and `StatusUnchanged`.
`Failure()` returns the result's `Err` as an `error`, and a nil interface when
`Err` is nil. Writing `return result.Err` from a function that returns `error`
would return a non-nil interface holding a nil pointer even after a success;
`Failure()` avoids that. `TargetError` supports `errors.Is` for the underlying
cause: `errors.Is(err, store.ErrBusy)` holds for a busy lock, and
`errors.Is(err, context.Canceled)` holds when a canceled context ended the wait.
The codes are `CodeBusy`, `CodeConflict`, `CodeModified`, `CodeUpdateRequired`,
`CodeUnsupported`, `CodeIOError`, `CodeUnsafePath`, `CodeInvalidState`,
`CodeConcurrentChange` and `CodeRecoveryNeeded`, with Rust's wire spellings.

## Contexts and lock timeouts

Every function that touches the filesystem or a store takes `ctx` first. The
context carries cancellation, and a deadline that can shorten a lock wait; it
carries no values for Flyrail to read. Pure functions (`Render`, `EditDocument`,
`Summarize`, constructors) take none.

Lock waits use an explicit `time.Duration`, the `LockTimeout` field of
`LockOptions`, `SyncOptions`, `RemoveOptions` and `SkillOptions`, or the
`timeout` argument of `Store.Lock`:

- Zero means exactly one acquisition attempt, which is also the default for a
  zero options struct. The first attempt is made even if the context is already
  done.
- A positive value waits until the lock is free, the timeout passes, or the
  context ends, whichever comes first. `DirectoryStore` polls every 20 ms;
  `MemoryStore` wakes waiters directly when the lock is released.
- Each lock gets its own full timeout. An operation that needs several locks can
  wait for each in turn.
- A lock that is not acquired is a `*store.BusyError`, and
  `errors.Is(err, store.ErrBusy)` holds. If the context ended, the error also
  wraps `ctx.Err()`.
- `DirectoryStore` guards its own record replacement with an internal lock that
  waits a fixed 10 seconds, still honoring cancellation.

Once a transaction's preparation record exists, every further store and
filesystem call of that transaction ignores cancellation, so a canceled context
cannot leave a half-published resource; only lock acquisition observes it.

A host that starts processes, for example a hook runner or a test helper that
re-executes itself, concurrently with Flyrail operations should pass a non-zero
`LockTimeout`. Between `fork` and `exec` a child briefly holds inherited copies of
open file descriptions, including `flock` locks, despite `O_CLOEXEC`, so a
zero-timeout attempt in that window can report busy. This is inherent to `flock`
and `fork` in every language. The example CLIs in this repository use five
seconds.

## Ownership and replacement

| `Acquisition` | Existing unowned selection | Intact removal |
| --- | --- | --- |
| `AcquireConflict` | Refuse, even when content matches. | Remove content created by this owner. |
| `AcquireTakeover` | Record the original selected content and position. | Restore the first baseline. |
| `AcquireAdopt` | Require an exact matching selection. | Remove the adopted unit without restoration. |

Acquisition cannot take another owner's claim, and this holds within one
store only: content recorded in a different store is foreign to Flyrail,
which recognizes neither its claims nor its authority records as fences.
Targets sharing one store share its receipts and fences. Content installed
through a store the application has since lost or replaced becomes foreign
the same way, even though Flyrail itself wrote it: default `AcquireConflict`
refuses it, and explicit `AcquireAdopt` re-owns content that matches exactly.

A changed or missing owned unit blocks its resource by default, including a
retiring unit. `ReplaceModified` authorizes replacement of the safely
observed same-owner unit and retains the first takeover baseline. It cannot
repair malformed boundaries, follow unsafe links, or override another owner.

Renaming an artifact at the same physical claim retains ownership and baseline.
Moving it acquires a new claim and retires the old one. Dependencies publish
required resources before dependents and retire dependents before requirements.
Partially updated generations keep old required resources discoverable.

## Current state and results

Bundle labels compare for equality, without ordering. A new label with unchanged
content can update metadata without rewriting a resource. Changed generated bytes
or destinations under the same label still require synchronization.

`InstallationObservation.Current` compares actual owned content with the
recorded completed generation. `Matches` also verifies desired bundle identity,
label, bundle digest, render digest, and rendering support. Neither proves that an
agent loaded or activated the configuration.

| `OperationStatus` | Meaning |
| --- | --- |
| `StatusApplied` | Requested work committed; still inspect cleanup and observation errors. |
| `StatusUnchanged` | Requested state already holds, possibly after recovery. |
| `StatusPartial` | Some resource commits succeeded before another resource or index step failed. |
| `StatusFailed` | No requested resource commit completed, or recovery failed. |
| `StatusIncomplete` | Unresolved recovery evidence remains. |

A write stages in the store's staging directory when it is on the same mount
as the destination, and otherwise in `P/.flyrail-<hash>-<transaction-id>.txn`
beside destination `P/N`. Transaction ids are 128 random bits in hex. A nonempty
`RecoveryPaths` names the transaction directory recorded by the resource's
preparation, left behind when a write was interrupted; the next successful
recovery removes it, not the caller. A write records its preparation before it
creates that directory, so an interruption at any point leaves the directory
attributed to the store. A write that fails before its journal is written
removes its own staged data, transaction directory, and preparation in the same
call. A transaction directory that no record in the store names belongs to
another store, or outlived the store that created it: Flyrail never reports,
removes, or waits on it, and deleting it is the host's decision.

Staging creates every node below the transaction directory relative to an open
parent descriptor with `O_EXCL|O_NOFOLLOW`, reached component by component with
`O_DIRECTORY|O_NOFOLLOW`, so a symlink swapped in during staging cannot
redirect a write. Observation listings do not follow links either.

Removal needs only the bundle ID and index. Intact created and adopted claims are
removed, takeovers are restored, and foreign content survives. Empty receipts
and permanent authority records may remain in the store. Applications must
keep the store's location ignored independently of their disposable
configuration files.

Flyrail never deletes an authority record, so a resource keeps fencing its
ancestors and descendants after removal; see
[resource authority](../../spec/resource-authority.md) for the record keys and
how to clear one by hand while no Flyrail operation runs against it.

Recovery deletes an interrupted preparation whose recorded transaction
directory no longer exists, as happens when a store is moved and its staging
directory moves with it. When the recorded directory still exists outside the
store's current staging location, for example after the store was copied
rather than moved, recovery reports `StateRecoveryNeeded` and touches nothing.
Clear it by hand while holding the resource lock: delete the journal and
preparation records, then remove the stale directory. A directory whose journal
survives can hold the only copy of the original content in `backup/resource`;
reconcile the destination from it first. The record formats are in the
[state encoding](../../spec/receipt-format.md).

A receipt referenced by committed current or previous membership is required.
Missing or malformed ownership metadata blocks update, removal, and recovery
without discarding the payload or index; an index cannot reconstruct lost
baselines.

## Filesystem boundary

Publication rechecks complete bytes, identity, supported security metadata, and
ancestors. Partial document claims preserve current file-wide metadata. Whole
file/tree claims follow their explicit mode contract.

Recovery also retains a destination that matches a recorded revision but was
modified, or on macOS created, after the transaction was prepared, since a
replacement can reuse the original's inode. It compares modification time on
every platform and creation time only on macOS. After the caller reconciles
the resource to a recognized revision, including its earlier modification
time when it is restored in place, a source-free retry can recover. A
destination whose modification time already lies in the future when the
transaction is prepared, or a clock that steps backwards, makes recovery refuse
the same way until the destination's modification time is no later than the
time the transaction was prepared.

Rollback requires a complete recognized public revision. Unexpected edits retain
journals and backups with an incomplete result; there is no force-recovery API.
Locks coordinate Flyrail writers, not hostile same-user processes. There is no
cross-resource atomicity, simultaneous multi-file visibility, network-filesystem
guarantee, or power-loss durability guarantee.

See the shared [transaction protocol](../../spec/transaction-protocol.md).
