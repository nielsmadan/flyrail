# Installation, update, and removal

[Planning and applying](#planning-and-applying) · [Results and errors](#results-and-errors) · [AbortSignal](#abortsignal) · [Lock timeouts](#lock-timeouts) · [Ownership and replacement](#ownership-and-replacement) · [Filesystem boundary](#filesystem-boundary) · [Process lifecycle](#process-lifecycle)

Flyrail synchronizes an immutable `Bundle` and an explicit `RenderedBundle` into
a logical `InstallationTarget`, backed by the application's `DirectoryStore` or
`MemoryStore`. One receipt owns each physical document, tree or skill container
within that store, and a private index in the same store locates the resources
belonging to one installation. Every rule below is the Rust crate's; this page
states how the TypeScript API exposes it.

## Planning and applying

`preview` and `previewRemoval` are read-only. They capture desired content,
context, resource revisions, receipts, index state and recovery evidence in a
`LifecyclePreview`, which keeps the native proposal. `preview.applicable` is
false when known errors or pending recovery prevent application.

`summarize(preview)` converts a preview into a `PlanSummary`. Each
`PlannedChange` maps a destination to its artifact ids and families and
classifies it as `create`, `update`, `delete`, `unchanged` or `conflict`.

```ts
import {
  DirectoryStore,
  RenderContext,
  Target,
  applyPreview,
  inspectInstallation,
  preview,
  render,
  summarize,
} from 'flyrail';

const context = new RenderContext(Target.project('codex', root), 'linux');
const rendered = render(bundle, context);
const store = await DirectoryStore.open(`${root}/.cache/project-guide`);
const target = context.installation(store);

const proposal = await preview(bundle, rendered, target);
if (!proposal.applicable) {
  throw proposal.error ?? new Error('preview is not applicable');
}
console.log(summarize(proposal).changes);
const result = await applyPreview(proposal, { lockTimeoutMs: 5000 });
if (!result.succeeded) {
  throw result.error ?? new Error(`installation ${result.status}`);
}
const observed = await inspectInstallation(bundle.id, target);
observed.matches(bundle, rendered);
```

`applyPreview` rejects any changed precondition, including foreign bytes
outside an owned section, and never refreshes a stale proposal. `sync` and
`remove` instead recover recognized interrupted work under locks and plan
again. `recoverInstallation` recovers interrupted resource transactions without
publishing new content; create a fresh preview afterwards.

## Results and errors

Problems arrive in two places.

- Invalid arguments reject the Promise: a relative path, a lock timeout that is
  not finite and nonnegative, an object that is not a built-in store, a
  malformed bundle id. These are `ValueError`s, raised before any native work
  starts. Store methods called directly reject with `StoreError`.
- Everything that happens to a target, such as a conflict, a busy lock, an I/O
  failure or interrupted earlier work, is a `TargetError` in the result, in
  `result.error`, `result.resources[i].error` or the observation, never a
  rejection. `TargetError` extends `FlyrailError`, so `throw result.error`
  works.

`InstallationResult.succeeded` is true exactly for `applied` and `unchanged`.

| `status`     | Meaning                                                                       |
| ------------ | ----------------------------------------------------------------------------- |
| `applied`    | Requested work committed; still inspect cleanup and observation errors.       |
| `unchanged`  | The requested state already holds, possibly after recovery.                   |
| `partial`    | Some resource commits succeeded before another resource or index step failed. |
| `failed`     | No requested resource commit completed, or recovery failed.                   |
| `incomplete` | Unresolved recovery evidence remains.                                         |

Preserve reported `recoveryPaths`: they name transaction directories left by an
interrupted write, which the next successful recovery removes.

## AbortSignal

Every lifecycle call takes `options.signal`. An already-aborted signal rejects
with `signal.reason` before any native work starts.

Mutations (`applyPreview`, `sync`, `recoverInstallation`, `remove`) pass a
native cancel token into the crate's `cancellable` scope. Lock waits end early
once the token is cancelled, and the token is checked once more right after a
transaction's locks are acquired.

- Cancelled before the first durable write: the Promise rejects with the exact
  `signal.reason`. `Err(Cancelled)` means no index, receipt, journal or
  preparation was written, no destination content was published or removed,
  and no recovery ran. Lock files, store/staging/destination parent
  directories and authority records (which the transaction protocol retains
  permanently anyway) may persist, the same footprint a busy timeout leaves.
- Aborted after the first durable write began: the token is ignored, and the
  Promise resolves with the real result.

The internal 10 s mutation lock inside a single `DirectoryStore` write is not
cancellable, so cancellation latency can reach that bound.

Read-only calls (`preview`, `previewRemoval`, `inspectInstallation`, store
`read` and `keys`, `DirectoryStore.open`) reject with `signal.reason` as soon as
the signal aborts; the native work finishes in the background and its result is
discarded. Store `write` and `delete` are compare-and-set calls with no
cancellable wait: the signal is checked only before they start, and once
started they resolve with their real result. Every call removes its abort
listener when it settles.

## Lock timeouts

Mutations take `lockTimeoutMs`, a finite number of milliseconds of at least 0:

- `0`, the default, makes exactly one attempt per lock.
- A positive value waits until the lock is free, the timeout passes or the
  signal aborts, whichever comes first. `DirectoryStore` polls every 20 ms;
  `MemoryStore` wakes waiters when the lock is released.
- Each lock gets its own full timeout, so an operation that needs several locks
  can wait for each in turn. A lock that is not acquired is a `busy`
  `TargetError` in the result.

A host that starts processes concurrently with Flyrail operations should pass
a non-zero `lockTimeoutMs`: between `fork` and `exec` a child briefly holds
inherited copies of `flock` locks, so a zero-timeout attempt in that window can
report busy.

## Ownership and replacement

| `acquisition` | Existing unowned selection                         | Intact removal                               |
| ------------- | -------------------------------------------------- | -------------------------------------------- |
| `'conflict'`  | Refuse, even when content matches.                 | Remove content created by this owner.        |
| `'takeover'`  | Record the original selected content and position. | Restore the first baseline.                  |
| `'adopt'`     | Require an exact matching selection.               | Remove the adopted unit without restoration. |

Acquisition never takes another owner's claim, and this holds within one store
only: content recorded in a different store is foreign. A changed or missing
owned unit blocks its resource by default, with a `modified` `TargetError`.
`replaceModified: true` authorizes replacement of the safely observed
same-owner unit and retains the first takeover baseline; it cannot repair
malformed boundaries, follow unsafe links or override another owner.

Removal needs only the bundle id and the index. Intact created and adopted
claims are removed, takeovers are restored and foreign content survives.
Authority records are permanent within a store, so keep the store's location
out of disposable configuration.

## Filesystem boundary

A write stages in the store's staging directory when it is on the same mount as
the destination, and otherwise in a hidden `.flyrail-<hash>-<transaction-id>.txn`
directory beside the destination. A destination, transaction directory or
bundle source that overlaps the store is refused. `DirectoryStore` refuses a
location, staging directory included, that is not private to its owner. Locks
coordinate Flyrail writers, not hostile same-user processes, and there is no
cross-resource atomicity or power-loss durability guarantee. See the shared
[transaction protocol](../../spec/transaction-protocol.md) and the
[Rust lifecycle guide](../../rust/docs/lifecycle.md) for the full rules.

## Process lifecycle

A pending call keeps the event loop alive until it settles, so await every
mutation before calling `process.exit` or Electron's `app.exit`; exiting
earlier abandons the operation wherever it is, which the next `sync`,
`remove` or `recoverInstallation` then recovers. Blocking work runs on the
addon's own worker pool, never on libuv's, so lock waits do not starve Node's
filesystem work. Calling the package from `worker_threads` is supported: the
design spike terminated a worker with a call in flight on Node, Bun and
Electron without a crash or a hang.
