# State store contract

## Purpose

Flyrail keeps no persistent state of its own beside the resources it manages.
The host application supplies a store that holds every index, receipt,
preparation, journal and lock flyrail needs. There is no default store and no
default location: flyrail never chooses where state lives, and a host that
needs persistence must construct and pass a store explicitly.

## Operations

A store exposes a `location`, an optional `staging_directory` and five
operations: `read`, `write`, `delete`, `keys` and `lock`.

`location` names the store's filesystem footprint, or is absent when the store
has none (for example, an in-memory store).

`staging_directory` names a directory, part of the store's footprint, in which
flyrail may create its transaction directories, or is absent (the default).
Flyrail creates it on first use and uses it only for a destination on the same
mount; see the
[transaction protocol](transaction-protocol.md#transaction-directory). A
directory-backed store uses a `staging` directory inside its location; a
single-file store uses a sibling named after its file with a `.staging`
suffix; an in-memory store has none.

`read(key)` returns the key's current value, or absence when the key has never
been written or has been deleted.

`write(key, value, expected)` is a compare-and-set. It succeeds and stores
`value` only when the key's current value equals `expected`; absent `expected`
means the key must currently be absent. It returns whether the write took
effect and never partially applies a value.

`delete(key, expected)` is the conditional counterpart: it removes the key
only when its current value equals `expected`, and returns whether the
deletion took effect.

`keys(prefix)` returns every key that starts with `prefix` as a plain string
comparison, with no separator semantics, in ascending code-point order.

`lock(name, timeout)` is a context manager that holds an exclusive lock named
`name` for its duration, waiting up to `timeout` seconds to acquire it before
giving up.

## Keys

A key matches the grammar `[a-z0-9][a-z0-9._-]*(/[a-z0-9][a-z0-9._-]*)*` and is
at most 200 characters. A key outside this grammar, or over length, is
rejected before any I/O takes place.

Flyrail uses a fixed key scheme:

- `index/<bundle-id>/<context-digest>` for an `Index`.
- `resource/<destination-key>/authority` for a `ResourceRef`.
- `resource/<destination-key>/receipt` for a `Receipt`.
- `resource/<destination-key>/preparation` for a `Preparation`.
- `resource/<destination-key>/journal` for a `Journal`.

Lock names follow the same scheme: `index/<bundle-id>/<context-digest>` for an
index, and `resource/<destination-key>` for a resource.

`destination-key` is `sha256(UTF-8(NFC(full-casefold(canonical destination in
slash form))))`, rendered as lowercase hexadecimal.

## Values

Values are opaque bytes to the store. Flyrail writes the canonical records
defined in [receipt-format.md](receipt-format.md); a store never parses,
validates or otherwise interprets what it is given, and round-trips exactly
the bytes it was handed, including an empty value.

## Guarantees

A `write` or `delete` that returns success is atomic and durable once the call
returns: no partial or torn value is ever visible, and the change survives a
later process crash or restart. Durability across power loss depends on the
platform's fsync semantics. A compare that does not match leaves the stored
value unchanged.

A held lock excludes every other handle attempting to acquire the same name on
the same store, in every process, for as long as it is held. A store never
treats a busy lock as abandoned on its own; only the holder's release, or the
holder's process ending in a way the store's locking mechanism detects,
frees it. Exceeding the requested wait raises the store's busy error rather
than blocking indefinitely or silently granting the lock.

A store backed by the filesystem creates every file and directory it writes
with owner-only access, and never widens an existing directory's permissions
to receive a new entry.

## Trust

A store is a trust root. Its records decide what flyrail deletes and restores,
and where, so anyone able to change them can direct flyrail's writes. The host
keeps its store private to the user running flyrail and never ships one inside
a repository or another shared location. The staging directory is part of the
store and is held to the same rule. The built-in filesystem stores refuse any
existing storage location, staging directory included, that is not private to
its owner, before reading or writing a record, and report it as `io_error`. A
custom store owns the same duty.

## Errors

A store maps an I/O failure to `io_error`. A lock wait that exceeds its
timeout maps to `busy`. A store that must serialize its own writes
internally — one backed by a single file or database, for example — can
raise `busy` from `write` or `delete` too, when it cannot complete promptly,
even though those operations take no timeout of their own. A compare-and-set
miss encountered during a flyrail operation maps to `concurrent_change`.

## Cross-application behaviour

A store normally belongs to one host application. Applications that share a
store share its receipts, fences and locks; flyrail does not tell their records
apart. Content recorded in a different store is foreign: flyrail applies the
same acquisition policies to it that it applies to any unowned content. Flyrail
reads and writes only its fixed key scheme and leaves other keys untouched.

## Conformance

Every store implementation runs the same conformance checks against the
contract above. In Python, `flyrail.testing.check_store` exercises a store
through two handles in one process and raises when a checked guarantee does not
hold: compare-and-set writes and deletes, value round-trips, key listing and
validation, and lock exclusion with bounded waits. It does not verify
cross-process lock exclusion or durability; a store's own tests cover those.
It requires a store with no keys under `conformance/` and leaves the keys it
writes there.
