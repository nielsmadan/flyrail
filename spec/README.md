# Shared specifications

These contracts are independent of implementation language:

- [Bundle format](bundle-format.md): manifest validation, snapshots and content digests.
- [Receipt format](receipt-format.md): ownership inventories and removal tombstones.
- [Transaction protocol](transaction-protocol.md): locking, commit and recovery.
- [Porting guidance](porting.md): compatibility requirements for native implementations.

`fixtures/bundles.json`, `fixtures/receipts.json` and `fixtures/transactions.json`
provide common vectors. They contain synthetic bytes, manifests and expected hashes;
they are not captured installation state or QA output. Every implementation should
consume the same files and add its own platform/error-path tests.
