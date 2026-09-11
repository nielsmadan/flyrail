# Native implementation notes

Python is the first implementation. Future Swift, Rust, Go and TypeScript
implementations should embed naturally in their hosts while sharing the on-disk
contracts below. Keep language-specific code and tooling under each language's
own directory.

## Shared contracts

| Reference | Compatibility requirement |
| --- | --- |
| [Bundle schema and content encoding](bundle-format.md) | Strict manifest validation, complete byte snapshots, portable names, explicit executable intent and the exact SHA-256 framing. |
| [Receipt schema and inventory encoding](receipt-format.md) | Same sibling layout, filenames, ownership reconciliation, inventory framing and removal tombstones. |
| [Transaction protocol](transaction-protocol.md) | Same permanent lock, immutable intent, exclusive moves, receipt commit boundary and conservative recovery rules. |
| [API semantics](../python/docs/api.md) and [inspection](../python/docs/inspection.md) | Explicit targets, upfront validation, ordered aliases, read-only inspection, independent observations/outcomes and source-free removal. Native type names may be idiomatic. |

Consume the language-neutral files in `spec/fixtures/`:
`bundles.json`, `receipts.json`, and `transactions.json`. They include binary
payloads as hexadecimal, explicit/empty directories, expected digests and complete
JSON records. Tiny vectors carry independently assembled digest framing bytes;
compare those bytes as well as hash outputs. Add malformed-input and native
filesystem tests beyond the valid fixtures; matching only the happy-path hashes
does not establish compatible validation or recovery.

JSON integers must remain exact unsigned 64-bit values where specified, and JSON
booleans cannot masquerade as integers. JavaScript ports need a lossless strategy
for values beyond `Number.MAX_SAFE_INTEGER`. Reject duplicate object keys and
unknown fields before they disappear into a generic decoded object. Versions are
opaque Unicode scalar strings compared verbatim. Use NFC normalization of full
Unicode case folding for portable collision keys; locale-sensitive lowercasing
does not implement that rule. Order encoded paths by UTF-8 bytes and frame all
lengths in bytes with unsigned 64-bit big-endian integers.

Keep library package version, host application version and each bundle's version
label independent. Schema versions and digest-encoding versions are separate
compatibility contracts too. An unsupported schema must fail closed; silently
ignoring future fields or rewriting another writer's state can lose ownership or
recovery information.

## Native adapters

Prefer standard-library facilities for paths, snapshots, JSON, hashing and
resource access where they meet the contract. Isolate missing OS primitives in
small adapters using direct native calls or a narrowly justified dependency.
Correctness takes precedence over a zero-dependency target. Do not invoke shell
commands or Python subprocesses for routine library behavior, invent cryptographic
implementations, or weaken exclusive moves into check-then-rename operations.

Cooperating ports must lock the same permanent file: `flock` on POSIX, byte zero
with length one on Windows. Preserve inode/handle stability across all bundles.
Use `renameat2(RENAME_NOREPLACE)` on Linux, `renamex_np(RENAME_EXCL)` on macOS and
an exclusive move on Windows. Keep receipt publication atomic and staging/backup
on the destination volume. Report unsupported primitives/filesystems explicitly.
Retain Windows logical executable intent and reject reparse points/junctions.

Resource adapters must snapshot the complete selected tree and preserve binary
bytes before returning. Do not keep borrowed resource handles that expire after
construction. Preserve canonical source roots for overlap checks. Packaging
systems can omit empty directories: include a visible placeholder upstream when
needed, as the [example packaging notes](../python/docs/examples.md#resource-packaging) explain.

Before claiming state interoperability, exercise writer/reader and interrupted
writer/recoverer pairs across implementations on every supported OS. Include
process kills before/after intent publication, backup/target moves, receipt commit,
partial cleanup and interrupted rollback. Keep unknown pre-intent staging and
edited backups. None of the ports should claim global reference counting,
cross-target atomicity, simultaneous multi-skill visibility or power-loss durability
without a separately specified and tested protocol change.
