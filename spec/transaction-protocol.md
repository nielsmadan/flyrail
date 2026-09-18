# Resource transaction and recovery protocol, schema 2

The [state layout](receipt-format.md) provides one permanent lock and at most one
published journal per resource. A mutation holds the index lock before all
required resource locks. POSIX locks use `flock`; Windows locks byte zero with
length one. Nonblocking attempts and a monotonic deadline implement bounded waiting.
A busy lock is never interpreted as abandonment.

## Immutable journal

`transaction.json` is a closed tagged `Journal` record with schema version 2.
It binds the resource, previous and intended receipts, complete original,
staged, published and private-backup revisions, and ancestor snapshots.
Ancestor evidence includes the authority, staging and backup directories.
The intended receipt's fresh transaction ID identifies its commit boundary.
The complete receipt and journal must agree on their resource.

Revision equality includes every owned and foreign byte, node identity, mode,
owner/group and supported security evidence. Equal-byte adoption and label-only
changes still journal ownership transitions; they do not need a content move.

Staging and backups stay under the adjacent authority on the resource volume.
Existing management directories must satisfy the supported private-access
contract before they receive payloads. Unsafe directories are refused without
changing their permissions. Newly created payload files are private before their
first bytes are written.
Private metadata preparation uses checked `.next` files before atomic
publication. Unknown or incomplete preparations remain recovery evidence.
Only specifically recognized complete preparations can be resumed or discarded.

A prepared installation index can repeat the current index, prepare a new pending
generation with all prior and desired resource references, or complete the recorded
pending generation. Recovery retains all references: it promotes recognized pending
preparation or discards a repeated/completion preparation only while the current
index still locates its resources. Required receipts, including those evidenced by
a recognized completed preparation, are checked before resource or index cleanup.
Resource recovery succeeds before this index cleanup. Unrecognized index transitions
retain their preparation and fail.

## Publication

1. Register each permanent authority fence before checking hierarchy conflicts.
   Recover recognized prior work under locks and observe a fresh target.
2. Preflight all known target conflicts and persist pending desired/previous
   index membership before the first resource content change.
3. Validate the planned ancestor evidence and stage the complete changed resource
   in private storage. Recheck full preimage, receipt, stage and ancestors, then
   publish the immutable journal. Only verified directories created by the
   operation may extend its ancestor evidence; preparation cannot refresh it.
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

| Observed receipt/revisions | Action |
| --- | --- |
| Previous receipt, exact original destination, recognized preparation | Clean preparation and preserve previous ownership. |
| Previous receipt, destination absent, complete recognized original backup | Restore the backup. |
| Previous receipt, exact published destination and original backup | Return recognized new data to staging and restore the original revision. |
| Previous receipt, equal-byte ownership-only transition | Preserve previous receipt ownership. |
| Intended committed receipt | Cleanup only; preserve subsequent public content edits. |
| Unexpected content, identity, security, ancestors, receipt or backup | Retain evidence and report `INCOMPLETE`. |

Rollback requires complete recognized revisions. It never selectively recomposes
a foreign edit around an owned selection. On Windows, restore a recognized private
backup to the public destination before restoring original security there.
Every recovery transition must itself be recognizable after interruption.

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

Locks and full-revision checks coordinate cooperating writers. State is not
authenticated against a hostile same-user process. The protocol handles process
interruption, not power-loss durability: it provides no fsync ordering guarantee.

`fixtures/configurations.json` contains production-shaped tagged journals and
observed recovery transitions, including ownership-only adoption. Implementations
must additionally exercise real process admission races and native failures at
publication, security, receipt, cleanup and rollback boundaries on each supported
operating system.
