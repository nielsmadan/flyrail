# Shared specifications

These contracts are independent of implementation language:

- [Bundle format](bundle-format.md): manifest validation, snapshots and content digests.
- [Receipt format](receipt-format.md): strict resource/index encoding and complete revisions.
- [Transaction protocol](transaction-protocol.md): locking, commit and recovery.
- [Porting guidance](porting.md): compatibility requirements for native implementations.
- [Configuration format](configuration-format.md): four-family schema-2 snapshots and typed values.
- [Resource authority](resource-authority.md): canonical destinations, permanent fences and claims.
- [Translation](translation.md): destinations, semantic capabilities and explicit asset references.
- [Configuration state](configuration-state.md): resource receipts, indexes and recovery contract.

`fixtures/bundles.json`, `fixtures/edits.json`, `fixtures/configurations.json`
and `fixtures/translations.json`
provide common vectors. They contain synthetic bytes, manifests, expected hashes,
schema-2 claims/authority paths, receipt/recovery transitions and capability contracts;
they are not captured installation state or QA output. Every implementation should
consume the same files and add its own platform/error-path tests.

[Shared document editing](document-editing.md) defines pure section and structured
edit proposals, syntax ownership, acquisition baselines, array restoration and
resource creation provenance. `fixtures/edits.json` supplies portable edit vectors.

Portable hooks and their runtime/ownership boundaries are specified in
[hook-protocol.md](hook-protocol.md); positive wire vectors live in
`fixtures/hooks.json`.
