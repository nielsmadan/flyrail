# Architecture

Flyrail has three stages: snapshot a bundle, resolve/observe explicit destinations,
then perform an optional receipt-backed transaction per physical target. All
runtime code uses Python's standard library. The host application owns command
parsing, user consent, presentation and process exit status.

| Area | Modules | Responsibility |
| --- | --- | --- |
| Public values | `models`, `observations`, `targets`, `bundle` | Immutable metadata, snapshots, destination attribution and results. `flyrail.__init__` defines public exports. |
| Source boundary | `_validation`, `_manifest`, `_sources` | Portable names/paths, strict schema parsing, safe filesystem/package/ZIP reading and complete snapshots. |
| Recorded and actual state | `_inventory`, `_receipts`, `_observation`, `inspection` | Deterministic digests, receipt validation/reconciliation, actual filesystem comparisons and read-only results. |
| Mutation boundary | `lifecycle`, `_intent`, `_transaction`, `_filesystem` | Request orchestration, transaction records, staging/publication/recovery, locks and native filesystem operations. |

Bundle construction copies bytes and executable intent before any mutation.
Retained canonical source roots prevent a later request from overwriting its own
source. JSON schemas and digest encodings are language-neutral; Python dataclasses
and internal helpers are implementation choices.

Every complete request is validated and canonicalized before its first write.
Physical target aliases are deduplicated while retaining each original request's
agent/scope attribution and position. Target and sibling state directories cannot
overlap one another, another destination, or the source. Inspection performs
best-effort reads and detects observable concurrent changes without locking.

A mutation holds one permanent advisory lock for the entire physical skill
container, across all bundles. It reconciles ownership receipts, recovers known
abandoned work, observes the requested bundle, then stages complete new trees and
an immutable intent. Exclusive directory moves publish individual skills; receipt
replacement commits the transaction. Before commit, safe recovery rolls back.
After commit, it only cleans up. Uninstall commits a removal tombstone and can run
without source bytes. Unexpected recovery data remains available to the caller.

The lock and recovery state live in sibling `.<container>.flyrail`, outside skill
discovery. Ownership belongs to whole named skill roots at that physical target,
including missing files. There is no machine-wide registry, logical consumer
reference count or transaction spanning targets.

`_filesystem` isolates OS locking and exclusive rename behavior. POSIX locks use
`fcntl.flock`; Windows locks one byte through `msvcrt`. Linux and macOS exclusive
renames use narrow `ctypes` calls to their native APIs; Windows uses its exclusive
`os.rename` contract. Unsupported native/filesystem behavior returns a typed
error. The implementation never substitutes an unsafe check-then-rename sequence.

The public [API](api.md), [bundle](../../spec/bundle-format.md), [receipt](../../spec/receipt-format.md)
and [transaction](../../spec/transaction-protocol.md) documents are the contract references.
[Support limits](support.md) describe the filesystem and concurrency boundary;
[porting notes](../../spec/porting.md) cover compatible future implementations.
