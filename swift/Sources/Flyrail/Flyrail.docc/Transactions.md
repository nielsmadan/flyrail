# Transactions and recovery

Swift uses the schema-2 resource protocol shared with Python and Rust. The
transaction engine is package-internal; lifecycle policy is a separate layer.

## Locking and planning

An operation acquires the installation index's permanent `flock` lock first.
Resource locks follow in byte-sorted authority-path order. Each authority is
reserved before hierarchy admission, and reservations survive failed preflight
and removal. Lock waits use a monotonic deadline and never infer abandonment.

The resource planner consumes an immutable observation. It binds complete
preimages, receipt records, state revisions and ancestor evidence, including
foreign bytes and native identity/security metadata. Acquisition, updates,
label changes, retirement, shared selections and skill-container directory
provenance all produce explicit receipt effects. Applying a resource plan
rechecks its evidence without refreshing it.

Every content or ownership transition receives a fresh receipt transaction ID.
Only a complete no-op reuses a previous receipt. Inspection compares installed
selections directly; baseline restoration constraints are checked when planning
retirement, not when deciding whether installed content is current.

## Commit boundaries

Before changing any resource, the index records pending membership and every
previous or desired resource reference. Complete payloads are created exclusively
under private adjacent staging and backup directories. New files are private
before their first bytes are written.

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
Private metadata uses checked exclusive `.next` files followed by atomic rename.

Publishing the intended receipt commits that resource permanently. Cleanup
deletes only recognized private revisions, in reverse node order, and removes
the journal last. Index completion follows all resource commits and cleanup.
A later failure leaves successful receipts authoritative and pending membership
discoverable.

## Source-free recovery

Recovery holds the same locks and checks required committed receipts before
cleaning resources or prepared index metadata. It rolls back recognized
uncommitted revisions, or performs cleanup only for an intended committed
receipt. Subsequent public edits survive committed cleanup.

Full revision equality includes bytes, modes, owner/group, device/inode and
supported security evidence. Later modification or creation timestamps also
reject a matching public replacement after journal preparation. Interrupted
private deletion is resumable only when the remaining revision is an exact
prefix of the recorded ordered nodes.

Unknown data, replaced evidence, incomplete preparations and missing committed
receipts are retained for reconciliation. Repeated recognized interruptions
converge; there is no age-based cleanup, forced recovery or ownership inferred
from an index. The guarantee covers process interruption, not power loss.

The internal interop executable exercises writing, inspection, source-free
removal/recovery, state records and barriers. Fault and barrier hooks are explicit
package-internal arguments, never public API or ambient environment settings.
