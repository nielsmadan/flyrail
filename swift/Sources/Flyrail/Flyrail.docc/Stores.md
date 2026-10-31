# State stores

The host supplies every index, receipt, preparation, journal and lock through a
``StateStore``. There is no default store and no default location: Flyrail never
chooses where state lives, and nothing is written beside managed resources except
transient `.flyrail-*.txn` transaction directories.

## Contract

``StateStore`` has `read`, `write(_:value:expected:)`, `delete(_:expected:)`,
`keys(prefix:)` and `lock(_:timeout:)`. Writes and deletes are compare-and-set: an
absent `expected` means the key must be absent, and a mismatch returns `false`
and leaves the value unchanged. `keys` is a plain UTF-8 prefix match in ascending
order. `lock` waits up to `timeout`, throws ``StoreError/busy(name:)`` when it
expires and never treats a held lock as abandoned. Release the returned
``StoreLock`` explicitly. Values are opaque bytes and round-trip exactly,
including empty values.

Optional `location` and `stagingDirectory` describe the store's filesystem
footprint; both default to `nil`. A staging directory on the destination's mount
hosts transaction directories (see <doc:Transactions>).

Keys are slash-separated segments of `[a-z0-9][a-z0-9._-]*`, at most 200 bytes;
``validateStoreKey(_:label:)`` rejects anything else before I/O. Flyrail uses
`index/<bundle-id>/<context-digest>` and
`resource/<destination-key>/{authority,receipt,preparation,journal}`; locks use
`index/…` and `resource/<destination-key>`.

## Built-in stores

``MemoryStore`` is process-local and has no footprint. ``DirectoryStore`` keeps
`records/`, `locks/`, a `mutation` lock file and `staging/` under its root. Every
file and directory is created owner-only (0700 and 0600) and an existing location
that is not private to its owner is refused, before any record is read or written,
rather than repaired. Two handles on one directory share records and locks across
processes.

## Conformance and trust

``checkStore(_:)`` opens a store twice and verifies compare-and-set, round trips,
key listing and validation, and lock exclusion with bounded waits. It does not
verify cross-process exclusion or durability, and it leaves its `conformance/`
keys behind, so use a fresh store.

A store is a trust root: its records decide what Flyrail deletes and restores, and
where. Keep it private to the user and out of any repository. Stores that share a
location share receipts and locks; content recorded in another store is foreign.
