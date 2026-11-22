# Native implementation notes

Python is the first implementation. Rust, Swift and Go ports exist. TypeScript
is not a port but a binding whose engine is the Rust crate, so its durable
formats are Rust's by construction. Keep implementation and tooling under the
corresponding language directory.

## Shared contracts

| Reference | Compatibility requirement |
| --- | --- |
| [Skill bundle format](bundle-format.md) | Schema-1 input validation and exact content framing. |
| [Configuration format](configuration-format.md) | Schema-2 artifacts, typed semantic values and rendering digests. |
| [Resource authority](resource-authority.md) | Canonical destinations, store-held authority records, physical claim identity and store-scoped hierarchy admission. |
| [Document editing](document-editing.md) | Scoped syntax, baseline disposition, array position and resource provenance. |
| [State encoding](receipt-format.md) | Strict tagged records, complete revisions and all editor evidence. |
| [Installation indexes](configuration-state.md) | Pending/current/previous/residual membership and source-free reconciliation. |
| [Transactions](transaction-protocol.md) | Shared locks, immutable journals, publication and exact recovery. |
| [State store](state-store.md) | Host-supplied store contract: fixed keys, compare-and-set, locks and busy/concurrent-change errors. |
| [Translation](translation.md) | Explicit destinations, supported semantic mappings and prerequisite notices. |
| [Hooks](hook-protocol.md) | Versioned command protocol, host outcomes, asset dependencies and runtime bounds. |

Consume `fixtures/bundles.json`, `fixtures/configurations.json` and
`fixtures/edits.json`, `fixtures/translations.json` and `fixtures/hooks.json`.
They include portable source bytes, semantic/digest vectors,
ownership and production-shaped receipt/recovery records. Bind synthetic native
identity tokens to isolated real filesystem identities, then exercise the actual
reader/remover/recoverer. Matching fixture hashes alone does not prove compatible
state validation or behavior.

All bundle inputs use the shared resource kernel; do not introduce a parallel
skill receipt engine or an implicit legacy migration. Public names can be idiomatic,
but immutable snapshots, explicit targets, source-free removal and independent
resource outcomes must agree.

Expose a pure desired-generation comparison alongside source-free inspection.
It must check recorded integrity, supported rendering, bundle ID/version/content
digest and render digest together. An intact recorded generation may differ from
the current app bundle. Keep configuration state distinct from host activation.

Python packages three authored runtime modules (`runner.mts`, `bridge.mts`,
`process.mts`) verbatim. Its wheel/sdist/Git builds need no JavaScript compiler or
runtime. Ports can reuse the protocol while providing idiomatic packaging; native
host execution requires explicit Node, while Pi/OpenCode glue uses existing host
runtimes. Never fetch or activate prerequisites while installing a bundle.

## Encodings and inputs

Reject duplicate JSON keys and unknown fields before a generic map drops them.
Distinguish booleans from integers. Preserve exact integer ranges and finite
binary64/date/time semantics; JavaScript needs a lossless strategy for values
beyond its safe integer range. Keep state tags and semantic encoding separate.

Versions are opaque Unicode scalar strings, compared verbatim. Use NFC of full
Unicode case folding for portable collision keys, not locale-sensitive lowercase.
Digest framing counts UTF-8 bytes and uses the ordering specified by each format.
Bundle, rendering, state and schema versions are independent contracts.

Snapshot complete source trees/resources before returning; never retain borrowed
ZIP/package handles. Source locations supply overlap guards and do not determine
configuration content identity. Keep environment references unresolved and never
execute bundled commands during loading or installation.

## Native behavior

Ports must take the same store lock, by name, through the store's `lock`
operation — not a bespoke lock file. Write the resource's authority record
before ancestor/descendant validation, as in
[resource authority](resource-authority.md). Independent callers must converge
on the same authority without a global registry: the same destination-key and
transaction-path hashes, computed the same way, are what let independent
processes and languages agree.

Use native exclusive publication primitives and private state on the resource
volume. Capture supported identity/security metadata, or refuse precisely.
Do not substitute shell commands, check-then-rename or generic copy helpers for
native guarantees. Windows shared-document DACL transitions require explicit
journal/recovery states; see the protocol and Python support boundary.

Bind full preimages, ancestors, receipts and index to immutable previews. Unknown
foreign edits during recovery are not selectively reconstructed. Retain old asset
revisions required after a partial reference update.

Implementations never share a state store. Before claiming interoperability, run
the same scenarios through each implementation with its own store on every
supported OS and compare results, destination bytes and modes, and store records
with transaction ids, node identities and absolute roots normalized. Include
equal-byte adoption, takeover restoration, retargeting, index-write failure,
interrupted-writer recovery, native security transitions and interrupted
cleanup/rollback, plus two applications in different implementations sharing one
document through separate stores. None of the
ports may infer cross-resource atomicity or power-loss durability from shared
receipt formats.

## Bindings

A binding reuses another implementation's engine instead of reimplementing the
contracts, so it needs no differential or format port of its own. The TypeScript
binding runs two sanity checks instead: a TypeScript installation and a Rust
`interop` driver operate on one store in both directions (including an
interrupted update and recovery), and TypeScript and Python applications share
one document through separate stores. It also relocates the shared state-record
fixtures and requires the engine's canonical encoding to be a fixed point.

## Interop drivers

Each implementation ships a command-line driver that performs one lifecycle
operation per invocation against a state directory and prints one JSON record,
so a harness can run the same scenario through every language. There is no
separate protocol document: the subcommands and options are the shared contract,
defined by the drivers and exercised by the Python harness in
[`swift/scripts/harness.py`](../swift/scripts/harness.py), whose `REGISTRY` names
them. The shared contract is the set of subcommands that
`python/scripts/interop_driver.py` and `rust/examples/interop.rs` both
implement: `encode`, `write`, `install`, `update`, `inspect`, `remove`,
`recover`, `records` and `hold`. `encode` has no single invocation both
reference drivers accept: Python's takes no options, and Rust's requires
`--type TYPE`. Go and Swift accept both forms, and the harness passes `--type`
only to Rust. Add a new language by providing a driver with those subcommands,
registering it there, and adding it to the scripts that compare it with the
others.

Some drivers carry extensions beyond that contract: `observe` (Go and Swift),
`differential` (Go and Swift), `render-hooks` and `decode` (Swift) and `edits`
(Go). Swift's `records` also reads path and byte records from stdin. Python and
Rust run the value and document-editing comparisons through the separate
`differential_driver.py` and `differential.rs` entry points instead.

| Language | Driver |
| --- | --- |
| Python | `python/scripts/interop_driver.py`, `python/scripts/differential_driver.py` |
| Rust | `rust/examples/interop.rs`, `rust/examples/differential.rs` |
| Swift | `swift/Sources/FlyrailInterop` |
| Go | `go/internal/cmd/flyrail-interop`, driven by the scripts in `go/scripts/` |
| TypeScript | none; it drives the Rust and Python drivers (see [Bindings](#bindings)) |

`spec/fixtures/interop-source-options.json` lists the options every driver's
`write --source` must reject before touching the store.
