# Resource transaction and recovery protocol, schema 2

The [state layout](receipt-format.md) provides one store lock per resource and
at most one preparation and one published journal record. A mutation holds the index lock before
all required resource locks, each acquired in a fixed destination order within
the store. A store's `lock` implements bounded waiting itself: nonblocking
attempts against a monotonic deadline, never interpreting a busy lock as
abandonment. A filesystem-backed store holds its lock with POSIX `flock`, or a
Windows byte-range lock of byte zero with length one; a store with no
filesystem footprint, such as an in-memory store, holds it with a process-local
mutex instead.

Before acquiring the index lock, a mutation creates any missing parent
directories of the store's own filesystem footprint, when the store has one,
and the store's staging directory with its missing parents, when the store has
one, each with an exclusive `mkdir` that fails if another writer already
created it meanwhile. After acquiring each resource's own lock, it creates
missing parent directories of that resource's destination the same way, before
admission runs. Only directories a mutation created itself are later approved
as part of its ancestor evidence; one that appeared through some other writer
is a concurrent change.

## Immutable journal

The journal is a closed tagged `Journal` record with schema version 2, held at
`resource/<destination-key>/journal`. It binds the resource, previous and
intended receipts, complete original, staged, published and private-backup
revisions, and ancestor snapshots. Ancestor evidence includes the destination's
ancestors, the transaction directory with its staging and backup subdirectories,
and the transaction directory's own ancestors. The complete receipt and
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

## Transaction directory

Staging and backups stay under the transaction's directory, which is always on
the destination's mount, so every content move is a rename within one mount. It
is in one of two locations:

- **Store staging.** When the store has a staging directory on the same mount
  as the destination's parent, the transaction directory is
  `<staging-directory>/<destination-key>-<transaction-id>`, with the staging
  directory in its physical spelling. Nothing is created beside the
  destination.
- **Fallback.** Otherwise, it is
  `<destination-parent>/.flyrail-<sha256(portable_path_key(basename))>-<transaction-id>.txn`
  beside the destination.

"Same mount" is decided on the physical staging directory and the physical
destination parent, each through its nearest existing ancestor. Both must have
the same device id. On Linux they must also resolve to the same mount ID in
`/proc/self/mountinfo`: the entry whose mount point (with its octal escapes
such as `\040` decoded) is the longest component prefix of the path, the last
such entry when mounts are stacked. A bind mount shares its filesystem's device
id but not its mount ID, and a rename across bind mounts fails with `EXDEV`, so
it falls back. On macOS the device id alone decides. Any doubt chooses the
fallback: no staging directory, a device id that cannot be read, or mountinfo
that is unreadable, unparsable or has no entry for either path.

The writer chooses per transaction, when it plans, and records the choice in
the preparation. Every later step and recovery uses the recorded directory and
never recomputes it.

Existing management directories must satisfy the supported
private-access contract before they receive payloads. Unsafe directories are
refused without changing their permissions. Newly created payload files are
private before their first bytes are written.

## Preparation and journal

Before it creates any transaction directory, the writer records a closed
tagged `Preparation` record with schema version 2 at
`resource/<destination-key>/preparation`, with compare-and-set, absent
expected. It names the resource, the intended receipt's transaction ID and the
absolute transaction directory. A preparation or journal already in the store
refuses the mutation with `RECOVERY_NEEDED`, naming that directory, until
recovery resolves it. Every transaction directory is therefore named by a
store record before it exists, and a crash leaves no unattributed directory
unless the host loses its store.

A preparation is honoured only when its directory is one of the two locations
above for its resource and transaction ID. Any other directory is refused with
`RECOVERY_NEEDED`, the preparation is retained, and no directory is touched for
it, so a damaged record cannot steer deletion. One case is released instead: a
preparation without a journal whose recorded directory does not exist, as when
the store was moved after a crash and the staging location moved with it.
Recovery deletes that preparation without touching any directory.

The journal record is written to the store with compare-and-set, absent
expected, after staging completes and before the first content move. Unknown
or incomplete transaction data remains recovery evidence; only specifically
recognized, complete transaction data can be resumed or discarded. If
preparation fails after the preparation record is written and before the
journal record is written, including when writing the journal fails, the same
call destroys the staged revision where it still equals what it staged, removes
the transaction directory it created and then deletes the preparation, leaving
the destination and the previous receipt untouched. Whatever it cannot remove
stays named by the preparation, and the next recovery finishes it.

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
3. Record the preparation before creating any transaction directory.
4. Validate the planned ancestor evidence and stage the complete changed resource
   in private storage. Recheck full preimage, receipt, stage and ancestors, set
   the transaction directory's modification time, then write the immutable
   journal record to the store. Only verified directories created by the
   operation may extend its ancestor evidence; preparation cannot refresh it.
5. Publish through exclusive moves or the supported native Windows document
   replacement transition. Recheck moved backups and the published full revision.
6. Recheck the previous receipt, expected published resource, backups, journal
   and recognized private layout, including stage/backup placement. Atomically
   publish the intended receipt. **This is the resource commit.**
7. Remove only recognized staging/backup data and the transaction directory,
   then delete the journal, then the preparation. Complete the installation
   index only after every resource reconciles.

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

Recovery acts only on the transaction directory named by a preparation record
in the host's store, and only when it is one of that transaction's two
locations; a preparation without a journal whose unrecognized directory does
not exist is deleted. A transaction directory that no record in the store
names — another store's live transaction, or one left behind by a host that
lost its store — is not attributed to this store: recovery neither reports nor
removes it, and it does not block mutation. Tree hierarchy probes still treat
it as a fence.

A preparation without a journal means the destination was never touched. A
recorded directory that does not exist needs no cleanup, so recovery deletes
the preparation whether or not the directory is one of the two locations; a
lookup that fails for any reason other than absence counts as existing. If the
named directory exists, it must be one of the two locations and a real private
directory, not a link, holding only `staging` and `backup`, with `backup`
empty; recovery destroys the contents of `staging` without following links and
removes the directories. It then deletes the preparation. Any other layout is
retained with `RECOVERY_NEEDED` naming the directory, and the preparation
stays. A journal without a matching preparation is invalid state; it is
retained as evidence.

| Observed receipt/revisions | Action |
| --- | --- |
| Preparation without journal | Remove the named transaction directory (destination untouched), then delete the preparation; a recorded directory that does not exist needs only the deletion. |
| Previous receipt, exact original destination, recognized staging and no post-preparation replacement evidence | Clean staging and preserve previous ownership. |
| Previous receipt, destination absent, complete recognized original backup | Restore the backup. |
| Previous receipt, exact published destination, original backup and no post-preparation replacement evidence | Return recognized new data to staging and restore the original revision. |
| Previous receipt, equal-byte ownership-only transition | Preserve previous receipt ownership. |
| Intended committed receipt | Cleanup only; preserve subsequent public content edits. |
| Unexpected content, identity, security, ancestors, receipt or backup | Retain evidence and report `INCOMPLETE`. |

A journal whose intended receipt equals its previous receipt is invalid: it
has no recognizable commit. Recovery refuses it with `RECOVERY_NEEDED` and
retains the journal, its preparation, the destination and the transaction
directory as evidence.

Rollback requires complete recognized revisions. It never selectively recomposes
a foreign edit around an owned selection. On Windows, restore a recognized private
backup to the public destination before restoring original security there.
Every recovery transition must itself be recognizable after interruption.

Immediately before writing the journal record, the writer sets the transaction
directory's modification time to the current time; this is the preparation
boundary. Staging is complete by then, and the renames of publication and
rollback happen inside `staging` and `backup`, not in the transaction directory
itself, so the boundary stays fixed until cleanup. Before rolling back, recovery
rejects a destination matching the journal's original, staged or published
revision when any of its nodes has a modification time later than the boundary
or, on macOS only, a birth time later than the boundary. Birth time is not
compared on other platforms, even where the filesystem records it, so every
implementation decides the same on-disk state the same way. Recovery retains
the evidence and reports `RECOVERY_NEEDED`. This detects replacement when a
filesystem reuses an inode; recognized publication remains no newer than the
boundary. The transaction directory is on the destination's mount in both
locations, so the comparison uses one filesystem's clock. Only cleanup changes
the boundary afterwards, by removing `staging` and `backup`, which moves it
later, or by removing the transaction directory; a rollback that finds the
directory already removed skips the comparison, since its earlier attempt
passed it before cleanup began.

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

`fixtures/configurations.json` contains production-shaped tagged preparations,
journals and observed recovery transitions, including ownership-only adoption. Implementations
must additionally exercise real process admission races and native failures at
publication, security, receipt, cleanup and rollback boundaries on each supported
operating system.
