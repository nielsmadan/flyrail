# Transactions and recovery

Swift uses the schema-2 resource protocol shared with Python and Rust. The
transaction engine is package-internal; lifecycle policy is a separate layer.

## Locking and planning

An operation acquires the installation index's store lock first. Resource store
locks follow in byte-sorted destination order. Each resource's authority record is
written before hierarchy admission, and records survive failed preflight and
removal. Lock waits are bounded by the store, use a monotonic deadline and never
infer abandonment; an exhausted wait is reported as busy.

The resource planner consumes an immutable observation. It binds complete
preimages, receipt records, state revisions and ancestor evidence, including
foreign bytes and native identity/security metadata. Acquisition, updates,
label changes, retirement, shared selections and skill-container directory
provenance all produce explicit receipt effects. Applying a resource plan
rechecks its evidence without refreshing it.

Every content or ownership transition receives a fresh transaction ID, which
also names its transaction directory.
Only a complete no-op reuses a previous receipt. Inspection compares installed
selections directly; baseline restoration constraints are checked when planning
retirement, not when deciding whether installed content is current.

## Commit boundaries

Before changing any resource, the index records pending membership and every
previous or desired resource reference. Before creating any transaction directory,
the engine records a ``Preparation`` naming it, so a crash leaves no directory
without a store record. A preparation or journal already in the store refuses the
mutation until recovery resolves it.

The transaction directory is always on the destination's mount. When the store's
`stagingDirectory` is on that mount, it is `<staging>/<destination-key>-<id>`;
otherwise it is the adjacent reserved `.flyrail-<digest>-<id>.txn`
(``transactionPath(destination:transactionID:)``). Complete payloads are created
exclusively under private `staging` and `backup` subdirectories. New files are
private before their first bytes are written.

Before pending index publication or payload staging, the engine checks complete
canonical receipt and journal encoding, including repeated baselines and bounded
native identities, against state limits. It also checks peak private-storage
bytes, node counts and depth. Unsupported directory modes are refused before
preparation: every recorded tree directory must permit owner read, write and
search so population, restaging and reverse-order cleanup remain possible.

An immutable journal records original, staged and published revisions and the
intended receipt. Resource moves use descriptor-relative
`renameatx_np(..., RENAME_EXCL)`, the `renamex_np` exclusive native transition.
There is no overwrite or check-then-rename fallback for resource publication.
Records are written with the store's compare-and-set.

Publishing the intended receipt commits that resource permanently. Cleanup
deletes only recognized private revisions, in reverse node order, and removes
the journal last. Index completion follows all resource commits and cleanup.
A later failure leaves successful receipts authoritative and pending membership
discoverable.

## Source-free recovery

Recovery holds the same locks and checks required committed receipts before
cleaning resources or prepared index metadata. It acts only on the directory a
preparation names, and only when that is one of the two locations for the
resource and transaction ID; a directory no record names is not attributed to the
store. It rolls back recognized uncommitted revisions, or performs cleanup only
for an intended committed receipt. Subsequent public edits survive committed
cleanup.

| Observed state | Action |
| --- | --- |
| Preparation without journal | Remove the named directory, then the preparation. |
| Previous receipt, recognized original or backup | Clean staging, or restore the original. |
| Intended committed receipt | Cleanup only. |
| Unexpected content, identity, security, ancestors, receipt or backup | Retain evidence. |

Full revision equality includes bytes, modes, owner/group, device/inode and
supported security evidence. Before writing the journal the engine sets the
transaction directory's modification time; recovery rejects a matching
destination with a later modification or birth time as replaced after
preparation. Interrupted private deletion is resumable only when the remaining
revision is an exact prefix of the recorded ordered nodes.

Unknown data, replaced evidence, incomplete preparations and missing committed
receipts are retained for reconciliation. Repeated recognized interruptions
converge; there is no age-based cleanup, forced recovery or ownership inferred
from an index. The guarantee covers process interruption, not power loss.

The internal interop executable exercises writing, inspection, source-free
removal/recovery, state records and barriers. Fault and barrier hooks are explicit
package-internal arguments, never public API or ambient environment settings.
