# Installation, update and removal

Flyrail synchronizes an immutable bundle and explicit rendering into a logical
`InstallationTarget`, backed by the application's `StateStore`. One resource
receipt owns a physical document, tree or skill container across callers and
bundles, within that store. A separate private index, held in the same store,
locates the resources belonging to one installation, including interrupted or
retired claims. Authority records are permanent within a store: nothing ever
deletes one, so a failed child admission keeps excluding an enclosing tree, and
a failed tree admission keeps excluding every descendant it already reserved.

## Planning and applying

`preview` and `preview_removal` are read-only. They capture desired content,
context, complete resource bytes, identities, security metadata, ancestors,
receipts, index and recovery state. `applicable` reports whether the proposal has
known errors or pending recovery.

`apply_preview` rejects any changed precondition, including an edit outside an
owned section. It never silently refreshes an old proposal. `sync` and `remove`
instead recover recognized interrupted transactions under locks, then plan afresh.

Applications that need to select content from observed files can call source-free
`recover_installation(bundle_id, target, *, lock_timeout=0)` first. It recovers
recognized interrupted resource transactions without publishing new desired
content or removing healthy claims. Re-read file existence, imports and other
selection conditions afterward, then construct and apply a new preview.
Successful recovery can leave pending/residual logical membership for that next
mutation to reconcile. It reports `APPLIED` for recovery work or `UNCHANGED` when
none was needed; an absent index creates no metadata. Unknown preparations retain
their evidence and return an error.

All known conflicts in a logical target are checked before content publication.
Physical resources commit independently. Once publication starts, a failure stops
the remaining resource sequence and retains pending membership for retry or
source-free removal. Successful resource commits are preserved.

`RenderedArtifact.require_existing` prevents an absent destination from being
created and binds the observed file through publication. It closes the gap
between an application's existence check and the later write.

## Ownership and replacement

| Acquisition | Existing unowned selection | Removal while intact |
| --- | --- | --- |
| `CONFLICT` (default) | Refuse even a matching selection. | Remove content created by this installation. |
| `TAKEOVER` | Record the original selected content and position. | Restore that first baseline. |
| `ADOPT` | Require the desired selection to match. | Remove adopted content without restoring it. |

Acquisition cannot take another bundle's claim, and this holds within one store
only: content recorded in a different store is foreign to flyrail, which
recognizes neither its claims nor its authority records as fences. Applications
sharing one store share its receipts and fences.
Content installed through a store the application has since lost or replaced
becomes foreign the same way, even though flyrail itself wrote it: default
`CONFLICT` acquisition refuses it, and explicit `Acquisition.ADOPT` re-owns
content that matches exactly, per the table above. A later edit by another
application to content this store still records as owned is reported as a
modification like any other change, not overwritten without
`replace_modified=True`. Sections use exact boundary lines; structured
selections use keys or unambiguous stable array members. The
[editor contract](../../spec/document-editing.md) defines matching and restoration.

A changed or missing owned unit blocks its resource by default, including a
retired unit. `replace_modified=True` authorizes replacement of the safely
observed same-owner unit; it retains the first takeover baseline. It cannot repair
malformed boundaries, follow unsafe links or override another owner's claim.

An artifact rename at the same physical selector retains ownership and baseline.
Moving to another destination or selector acquires a new claim and retires the
old one. Unresolved retirements remain indexed and owned. Dependencies publish
support assets before references and retire references before their assets;
partially updated references keep their old required resources discoverable.
Changing the bytes of a referenced asset requires a distinct revision destination,
so the old reference can continue to use its original bytes after a partial update.

Compatible artifact aliases at one physical claim share ownership. Dependencies
on several aliases of the same required claim are recorded once, including
stable references. Aliases of a dependent claim must agree on which physical
requirements are stable references.

## Version and current state

Version labels are compared for equality, without ordering. A new label with the
same content can update metadata without rewriting a resource; changed generated
or rendered bytes under the same label still require synchronization.

Source-free inspection reports whether actual owned content agrees with the
completed index generation. `observation.matches(bundle, rendered)` additionally
checks bundle identity, version, bundle/render digests and supported rendering.
Current state requires all desired
claims and retirements to reconcile, with no pending membership or recovery.
It does not assert that an agent has loaded the configuration.

## Results

| Status | Meaning |
| --- | --- |
| `APPLIED` | The requested change committed. Check resource cleanup and observation errors too. |
| `UNCHANGED` | The requested state already holds, possibly after recovery. |
| `PARTIAL` | Some resource commits succeeded but another resource or index step failed. |
| `FAILED` | The requested operation failed without a successful resource commit or reported incomplete resource recovery. |
| `INCOMPLETE` | Recovery data remains unresolved; retain the reported paths. |

A write stages in the store's staging directory when it is on the same
mount as the destination, and otherwise in `P/.flyrail-<hash>-<transaction-id>.txn`
beside destination `P/N`. A nonempty `recovery_paths` names the transaction
directory recorded by the resource's preparation — in the store's staging
directory or beside the destination, wherever that write staged — left behind
when a write was interrupted; the next successful recovery removes it, not the
caller. A write records its preparation
before it creates that directory, so an interruption at any point leaves the
directory attributed to the store. A write that fails before its journal is
written removes its own staged data, transaction directory and preparation in
the same call. A transaction directory that no record in the store names
belongs to another store, or outlived the store that created it: flyrail never
reports, removes or waits on it, and deleting it is the host's decision.

Inspect the aggregate error, final observation and each resource result. A resource
whose receipt committed stays `APPLIED` if cleanup later fails. A subsequent
mutation performs cleanup only for that committed transaction.

Removal is source-free. Intact created/adopted claims are deleted and intact
takeovers restored; foreign content survives. Empty resource receipts and permanent
authority records may remain in the store. Applications must keep the store's
location ignored independently of their disposable configuration files.

Flyrail never deletes an authority record, so a resource keeps fencing its
ancestors and descendants after removal; see
[resource authority](../../spec/resource-authority.md). To clear one, delete its
record from the store directly while no flyrail operation runs against it:

```python
import hashlib
import unicodedata

from flyrail import ResourceAuthority

destination = ResourceAuthority(path).destination.as_posix()
digest = hashlib.sha256(unicodedata.normalize("NFC", destination.casefold()).encode()).hexdigest()
key = f"resource/{digest}/authority"
if (data := store.read(key)) is not None:
    store.delete(key, expected=data)
```

Recovery deletes an interrupted preparation whose recorded transaction directory
no longer exists, as happens when a store is moved and its staging directory
moves with it. When the recorded directory still exists outside the store's
current staging location, for example after the store was copied rather than
moved, recovery reports `RECOVERY_NEEDED` and touches nothing. Clear it by hand
while holding the resource lock: delete the journal and preparation records,
then remove the stale directory. A directory whose journal survives can hold the
only copy of the original content in `backup/resource`; reconcile the
destination from it first. Continuing the recipe above:

```python
import json
import shutil
from pathlib import Path

with store.lock(f"resource/{digest}", 10):
    preparation = store.read(f"resource/{digest}/preparation")
    if preparation is not None:
        stale = Path(json.loads(preparation)["fields"]["directory"]["path"])
        for record in ("journal", "preparation"):
            key = f"resource/{digest}/{record}"
            if (data := store.read(key)) is not None:
                store.delete(key, expected=data)
        shutil.rmtree(stale)
```

A resource referenced by the committed current or previous generation requires
its ownership receipt. A missing receipt reports `INVALID_STATE` and blocks
update, removal and recovery while preserving the index and payload. An index
cannot reconstruct lost ownership or baselines. A pending first-install resource
may legitimately have no receipt before its first commit. Valid retained receipts
with no matching claims also remain usable, including after another index
completed retirement at the shared authority.

## Filesystem boundary

Full bytes, identity, mode/security evidence and ancestors are rechecked around
publication, including moved backups and staged data. A partial section/key claim
preserves the current file security metadata, including later restrictive chmod
changes; its original content baseline does not own file-wide permissions.
Whole file/tree modes follow their explicit content contract.

Private state, baselines and recovery payloads stay on the resource volume.
The unsupported Windows adapter's document publication journals its private-DACL
transition; interruption can leave the public file restricted until recovery
restores its original descriptor. Unsupported metadata fails without a weaker
in-place fallback. See [platform support](support.md).

Rollback requires complete recognized public revisions. Private cleanup can
resume partway through deleting a tree when every remaining node matches the
recorded deletion order, bytes, identity and security metadata. Unexpected owned
or foreign edits retain the journal and backups with `INCOMPLETE`; there is no
automatic selective inverse edit or force-recovery API. A destination that
matches a recorded revision but was modified (or, on macOS, created) after the
transaction was prepared is retained the same way, since a replacement can reuse
the original's inode. After the caller reconciles the resource to a recognized
revision, including its earlier modification time when it is restored in place,
a source-free retry can recover. A destination whose modification time already
lies in the future when the transaction is prepared, or a clock that steps
backwards, makes recovery refuse the same way until the destination's
modification time is no later than the time the transaction was prepared. The
[transaction protocol](../../spec/transaction-protocol.md) defines the states.

Locks coordinate Flyrail writers, not hostile same-user processes. There is no
cross-resource atomicity, simultaneous multi-file visibility or power-loss
durability guarantee.
