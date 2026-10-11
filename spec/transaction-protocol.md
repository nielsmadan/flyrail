# Resource transaction and recovery protocol, schema 2

The [state layout](receipt-format.md) provides one store lock per resource and
at most one published journal record. A mutation holds the index lock before
all required resource locks, each acquired in a fixed destination order within
the store. A store's `lock` implements bounded waiting itself: nonblocking
attempts against a monotonic deadline, never interpreting a busy lock as
abandonment. A filesystem-backed store holds its lock with POSIX `flock`, or a
Windows byte-range lock of byte zero with length one; a store with no
filesystem footprint, such as an in-memory store, holds it with a process-local
mutex instead.

Before acquiring the index lock, a mutation creates any missing parent
directories of the store's own filesystem footprint, when the store has one,
each with an exclusive `mkdir` that fails if another writer already created it
meanwhile. After acquiring each resource's own lock, it creates missing
parent directories of that resource's transaction path the same way, before
admission runs. Only directories a mutation created itself are later approved
as part of its ancestor evidence; one that appeared through some other writer
is a concurrent change.

## Immutable journal

The journal is a closed tagged `Journal` record with schema version 2, held at
`resource/<destination-key>/journal`. It binds the resource, previous and
intended receipts, complete original, staged, published and private-backup
revisions, and ancestor snapshots. Ancestor evidence includes the transaction
directory and its staging and backup subdirectories. The complete receipt and
journal must agree on their resource.

Every journaled transaction gives its intended receipt a fresh transaction ID,
including a repair that moves content while leaving ownership otherwise
unchanged, so the intended receipt always differs from the previous one. The
ID names the transaction directory and identifies the commit boundary:
recovery decides that a transaction committed only by finding its intended
receipt in the store, which is unambiguous only while that receipt cannot equal
the previous one, and each attempt's directory stays distinct from any that an
interrupted earlier attempt left behind. A resource whose content and receipt
would both stay unchanged keeps its previous receipt, including its transaction
ID, and is not journaled.

Revision equality includes every owned and foreign byte, node identity, mode,
owner/group and supported security evidence. Equal-byte adoption and label-only
changes still journal ownership transitions; they do not need a content move.

Staging and backups stay under the resource's transaction directory, on the
resource's volume. Existing management directories must satisfy the supported
private-access contract before they receive payloads. Unsafe directories are
refused without changing their permissions. Newly created payload files are
private before their first bytes are written.

The journal record is written to the store with compare-and-set, absent
expected, after staging completes and before the first content move. Unknown
or incomplete transaction data remains recovery evidence; only a specifically
recognized, complete preparation can be resumed or discarded. If preparation
fails at any point before the journal record is written, including when writing
it fails, the same call destroys the staged revision where it still equals what
it staged and removes the transaction directory it created, leaving the
destination and the previous receipt untouched. Data it cannot recognize stays
in place; no journal names it, so it never blocks a later mutation.

The installation index records desired `pending` membership and the full
`residual` resource-reference set directly in its `Index` record, written to
the store with compare-and-set before any resource content changes. Required
receipts, including those for resources the pending generation references, are
checked before resource or index cleanup. Resource recovery succeeds before the
index advances to its completed generation. An index record that changed since
it was read fails the operation with a concurrent-change error rather than
recomputing against stale membership.

## Publication

1. Write each resource's own authority record, retained permanently, before
   checking hierarchy conflicts. Recover recognized prior transaction data
   under locks and observe a fresh target.
2. Preflight all known target conflicts and write the pending index record
   (compare-and-set) before the first resource content change.
3. Validate the planned ancestor evidence and stage the complete changed resource
   in private storage. Recheck full preimage, receipt, stage and ancestors, then
   write the immutable journal record to the store. Only verified directories
   created by the operation may extend its ancestor evidence; preparation cannot
   refresh it.
4. Publish through exclusive moves or the supported native Windows document
   replacement transition. Recheck moved backups and the published full revision.
5. Recheck the previous receipt, expected published resource, backups, journal
   and recognized private layout, including stage/backup placement. Atomically
   publish the intended receipt. **This is the resource commit.**
6. Remove only recognized staging/backup data and preparations; remove the journal
   last. Complete the installation index only after every resource reconciles.

Exclusive POSIX moves use macOS `renamex_np(RENAME_EXCL)` or Linux
`renameat2(RENAME_NOREPLACE)`. Windows uses exclusive moves for applicable trees
and `ReplaceFileW` with flags zero for existing supported documents.
There is no existence-check-plus-rename or in-place overwrite fallback.

Windows document replacement narrows the live file to the journaled private DACL
before producing its backup. The replacement is published privately, then receives
the recorded original descriptor at its public destination. Both protected and
original-security states must be explicitly recognized. Backups and staging never
regain broad DACLs inside private storage. A process interruption can leave the
destination restricted until recovery; unavailable required descriptor access
safely refuses publication.

## Recovery

Only a mutation holding the resource lock decides that a journal can be recovered.
Read-only inspection can observe an active writer and cannot establish abandonment.

Recovery acts only on the transaction directory named by a journal record in
the host's store. A transaction directory that no journal in the store names —
another store's live transaction, or the remains of a crash before the journal
was written — is not attributed to this store: recovery neither reports nor
removes it, and it does not block mutation. Tree hierarchy probes still treat
it as a fence.

| Observed receipt/revisions | Action |
| --- | --- |
| Previous receipt, exact original destination, recognized preparation | Clean preparation and preserve previous ownership. |
| Previous receipt, destination absent, complete recognized original backup | Restore the backup. |
| Previous receipt, exact published destination, original backup and no post-preparation replacement evidence | Return recognized new data to staging and restore the original revision. |
| Previous receipt, equal-byte ownership-only transition | Preserve previous receipt ownership. |
| Intended committed receipt | Cleanup only; preserve subsequent public content edits. |
| Unexpected content, identity, security, ancestors, receipt or backup | Retain evidence and report `INCOMPLETE`. |

A journal whose intended receipt equals its previous receipt is invalid: it
has no recognizable commit. Recovery refuses it with `RECOVERY_NEEDED` and
retains the journal, the destination and the transaction directory as evidence.

Rollback requires complete recognized revisions. It never selectively recomposes
a foreign edit around an owned selection. On Windows, restore a recognized private
backup to the public destination before restoring original security there.
Every recovery transition must itself be recognizable after interruption.

Recovery rejects a matching destination that has a modification time after the
journal or, where the filesystem records it, a later creation time. This detects
replacement when a filesystem reuses an inode; staging is complete before the
journal is written, so recognized publication remains older than the journal.

After commit, cleanup cannot roll back ownership. Nodes are deleted in reverse
revision order. An interrupted deletion is recognized only when the remaining
nodes form an exact prefix of the recorded revision, including their bytes,
identities and security metadata. This also permits resuming cleanup after
rollback has restored the complete original destination and emptied its backup.
Other remaining data is retained. A later source-free retry can resume after the
caller reconciles an unknown state to a recognized revision.
There is no automatic age-based garbage collection or force-clean operation.

## Limits and verification

Resources commit independently; later failures retain completed resource commits
and pending logical membership. The aggregate can be `PARTIAL` while a resource
is `APPLIED` or has `INCOMPLETE` recovery. No source bundle is needed to locate
recorded claims or recover an already journaled transition.

Another application's edit to a destination is invisible to this store's
records, so only the revision rechecks protect it. Editing the destination by
path at each content move was measured to retain the foreign bytes in the
destination or in retained staging or backup evidence: before and inside the
move to backup, before and after the exclusive publish move (for document and
tree creation, a foreign creation makes that move fail), before and after the
receipt commit, and inside each rollback move. Every case before the commit
reports `INCOMPLETE`. One window remains for a writer holding a descriptor
opened before the move to backup: its writes land in the backup, and those made
after cleanup's final observation of the backup, including after the operation
returns, are lost when cleanup unlinks it. Rename-based publication cannot
close that window.

Locks and full-revision checks coordinate cooperating writers. State is not
authenticated against a hostile same-user process. The protocol handles process
interruption and adds no fsync ordering of its own: store records are durable
across process crashes, and durability across power loss depends on the
platform's fsync semantics.

`fixtures/configurations.json` contains production-shaped tagged journals and
observed recovery transitions, including ownership-only adoption. Implementations
must additionally exercise real process admission races and native failures at
publication, security, receipt, cleanup and rollback boundaries on each supported
operating system.
