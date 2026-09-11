# Transaction and recovery protocol, schema 1

The sibling state directory described in the [receipt format](receipt-format.md)
contains one permanent lock, receipt files, staging/backup containers and at most
one published `transaction.json`. A mutation holds the target's lock for recovery,
observation, preparation, publication and cleanup. All bundles at that physical
target use the same lock inode. The lock is never replaced or deleted.

New state, receipt, staging and backup containers are created with mode `0700`.
Each transaction's fresh staging root, its `skills` container and backup root also
use `0700` before any skill bytes are written or moved there. On POSIX these
transaction roots prevent group/other access even if existing management parents
have broader permissions. Existing directories and lock permissions are preserved;
installed skill files retain their executable/nonexecutable modes.

Windows implementations must create management directories with access limited to
the current user and administrators. The [Python support reference](../python/docs/support.md)
explains its standard-library version requirements.

POSIX uses nonblocking `flock`. Windows uses a nonblocking exclusive lock on byte
zero with length one. Bounded waiting uses repeated nonblocking attempts and a
monotonic deadline. Process termination releases the OS lock. A busy live lock is
never treated as an abandoned transaction; no PID lease or lock stealing is used.

Exclusive moves use macOS `renamex_np(RENAME_EXCL)`, Linux
`renameat2(RENAME_NOREPLACE)`, or a Windows move that refuses an existing
destination. There is no POSIX existence-check-plus-rename fallback. Missing
native support or an unsupported filesystem produces an explicit `UNSUPPORTED`
error. Destination, receipt, staging and backup directories must be on the same
volume. Language bindings are described by each implementation.

## Immutable intent

A transaction has a fresh 32-character lowercase hexadecimal ID, carried by its
`new` receipt. It uses:

```text
.<container>.flyrail/
    lock
    receipts/<bundle-id>.json
    transaction.json
    staging/<transaction-id>/
        skills/<skill-name>/...
        receipt.json
    backup/<transaction-id>/<skill-name>/...
```

`transaction.json` is UTF-8 JSON with exactly these fields:

| Field | Meaning |
| --- | --- |
| `schema_version` | Integer `1`, never a boolean. |
| `previous` | Complete previous receipt, including a previous removal receipt, or `null` when no receipt existed. |
| `new` | Complete intended receipt; an active installation or transaction-tagged removal tombstone. |
| `changes` | Array of `{ "name": ..., "old": [...], "new": [...] }` skill operations. |

Both receipts use the complete [receipt schema](receipt-format.md), including
inventory digests. Their bundle IDs agree and their transaction IDs differ.
Unknown fields, duplicate keys, unsupported schemas and malformed values are
rejected. Names are valid skill identifiers, with one operation per skill.
Inventories use receipt entry records and portable path rules. Each includes
exact parent directories, unique portable paths and one named root when present.

An operation's `old` inventory describes the exact observed safe revision, which
may differ from `previous.entries` only for that receipt's owned skills. It may
be empty, lack `SKILL.md`, contain added files, or have a file in place of the
owned root when explicit replacement was authorized. The `new` inventory must
match that skill's portion of the new receipt. Empty inventories represent
absence. Names outside previous/new ownership cannot appear, and every change
between recorded previous/new inventories must have an operation. Identical
observed old/new inventories require no filesystem moves, even if recording that
revision requires a receipt change. Unchanged skills can be omitted.

The writer emits compact JSON with sorted object keys, a trailing newline,
sorted inventory paths and sorted skill operations. The intent is immutable once
published. Recovery verifies it before cleanup and again before removing it.
JSON whitespace and object order do not affect its meaning. Shared vectors in
`spec/fixtures/transactions.json` cover installation, uninstall and
a label-only update, using the independently framed receipt fixture.

## Publication and commit

1. Read/reconcile receipts and observe managed content under the lock. Recover
   existing work first, then obtain the revision used for this operation.
2. Build the new receipt and per-skill expected inventories. Create the transaction
   private staging and backup directories, write every changed new skill, set executable
   modes, and write the new receipt. Revalidate the observed old trees, staged
   new trees and previous receipt before target publication.
3. Write `staging/<id>/intent.json`, then exclusively move that complete file to
   `transaction.json`. The published intent is never rewritten to advance phases.
4. For each changed skill, exclusively move old content to its backup location,
   revalidate the backup, and exclusively move staged new content to the target.
   Removed skills have no new publication. Recheck changed and desired trees.
5. Revalidate the previous receipt and staged receipt, then atomically replace
   `receipts/<bundle-id>.json` with the staged receipt. **This is the commit
   boundary.** Uninstall commits by publishing its removal tombstone.
6. Revalidate and remove expected backup/staging data, remove the transaction's
   empty directories, then remove `transaction.json` last.

An exception before commit attempts safe rollback under the held lock. An
exception after receipt publication only attempts cleanup. Publishing changed
skills individually does not provide simultaneous visibility of all skills.

## Recovery after interruption

Only a mutation holding the lock interprets intent as abandoned work. It validates
all receipts, the intent, old/new ownership against other receipts, and the
recognized recovery layout. The current receipt must equal either `previous` or
`new`; anything else is uncertain and retained.

If the current receipt equals `new`, the transaction committed. Recovery cleans
only its expected staging and backups, even when installed content has since
been edited. The next ordinary operation observes those edits before deciding
whether it can proceed.

If the current receipt equals `previous` (or both are absent), recovery rolls
back. It recognizes complete expected old backups, expected new destinations,
and staged new content. To restore an old backup it first exclusively returns a
published new directory to its stage, then exclusively restores the backup. A
first installation rolls back to absence. A restored old destination with no
backup is already rolled back. Missing or changed old backups are never treated
as complete recoverable old data. Unexpected destination bytes are retained.

Recovery itself may be interrupted after either move. The same inventory and
placement checks recognize those intermediate states on the next call. Cleanup
accepts a verified subset of the expected inventory: missing entries can have
been deleted by an earlier cleanup attempt, while every remaining file's bytes,
size, executable intent and every remaining directory must still match. Each
cleanup attempt validates the whole remaining inventory once, then revalidates
each entry's content, type and safe ancestors immediately before deletion.
Unchanged cleanup data is read a bounded number of times per attempt, including
large skills. Rollback requires complete old data; subset
rules apply only to deletion of expected cleanup data. This allows interruption
in the middle of either committed cleanup or rollback cleanup.

Unknown entries, edited backups, conflicting destinations, incomplete old data,
or invalid/unreadable intent are retained and reported with `INCOMPLETE` and
recovery paths. No caller needs source files to recover or uninstall.

If a process dies during preparation before publishing a complete intent, its
unrecognized staging/backup data is conservatively retained. This includes a
complete staged `intent.json` that never reached the published location. There
is no automatic age-based deletion or reconstruction from partial preparation.
The caller must preserve and examine the reported paths before resolving that
state. The same bounded behavior applies when preparation fails normally before
intent publication; it does not erase uncertain bytes to make the next call pass.

## Guarantees and limits

Locks coordinate Flyrail writers. Inventories and exclusive moves detect common
concurrent edits and preserve unexpected recovery data. They do not authenticate
receipts or defend against a hostile same-user process changing paths between a
check and a syscall. Inspection is read-only and can observe an active transaction.

There is no cross-target atomicity, simultaneous multi-skill visibility, or
power-loss durability claim. The protocol handles process interruption; it does
not use an fsync ordering protocol. Permanent locks and small management
containers/receipts may remain after uninstall. The skill container and unrelated
content are retained.

Tests kill real subprocesses before/after intent and receipt publication, after
backup and target moves, during partial deletion, and during rollback recovery.
They separately exercise uninstall's tombstone boundary and bounded lock
contention. Platform adapters have contract tests; actual native filesystem and
locking behavior must also run on each supported operating system.
