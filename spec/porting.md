# Native implementation notes

Python is the first implementation. Swift, Rust, Go and TypeScript ports should
embed naturally in their hosts while sharing the contracts in this directory.
Keep implementation and tooling under the corresponding language directory.

## Shared contracts

| Reference | Compatibility requirement |
| --- | --- |
| [Skill bundle format](bundle-format.md) | Schema-1 input validation and exact content framing. |
| [Configuration format](configuration-format.md) | Schema-2 artifacts, typed semantic values and rendering digests. |
| [Resource authority](resource-authority.md) | Canonical destinations, common permanent fences, physical claim identity and hierarchy admission. |
| [Document editing](document-editing.md) | Scoped syntax, baseline disposition, array position and resource provenance. |
| [State encoding](receipt-format.md) | Strict tagged records, complete revisions and all editor evidence. |
| [Installation indexes](configuration-state.md) | Pending/current/previous/residual membership and source-free reconciliation. |
| [Transactions](transaction-protocol.md) | Shared locks, immutable journals, publication and exact recovery. |
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

Ports must lock the same permanent file: POSIX `flock`, Windows byte zero with
length one. Register a resource fence before ancestor/descendant validation.
Independent callers must converge on the same authority without a global registry.

Use native exclusive publication primitives and private state on the resource
volume. Capture supported identity/security metadata, or refuse precisely.
Do not substitute shell commands, check-then-rename or generic copy helpers for
native guarantees. Windows shared-document DACL transitions require explicit
journal/recovery states; see the protocol and Python support boundary.

Bind full preimages, ancestors, receipts and index to immutable previews. Unknown
foreign edits during recovery are not selectively reconstructed. Retain old asset
revisions required after a partial reference update.

Before claiming interoperability, test writer/reader and interrupted-writer/
recoverer pairs across implementations on every supported OS. Include equal-byte
adoption, takeover restoration, retargeting, index-write failure, process admission
races, native security transitions and interrupted cleanup/rollback. None of the
ports may infer cross-resource atomicity or power-loss durability from shared
receipt formats.
