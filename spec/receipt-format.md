# Receipt format, schema 1

For a skill container `/chosen/skills`, Flyrail management state lives in the
sibling `/chosen/.skills.flyrail`. It is outside the skill discovery container.
An active receipt owns whole named skill directories, including currently missing
content. Removing files from disk never transfers ownership.

```text
/chosen/skills/review/SKILL.md
/chosen/.skills.flyrail/
    receipts/team.json
    lock
    transaction.json
    staging/
    backup/
```

Every state entry shown is optional during read-only inspection. `lock` is a
regular file reserved for a stable advisory lock; inspection neither creates nor
locks it. `transaction.json` and nonempty staging/backup directories report
recovery needed. Their [lifecycle and recovery protocol](transaction-protocol.md) uses the receipt
transaction ID as its commit boundary.
Unknown management entries, non-directory containers, non-file receipts/locks,
managed symlinks/reparse points, or portable filename collisions fail closed.

Receipt filenames are exactly `<bundle-id>.json`, with the same validated
identifier inside the receipt. All receipts are parsed and reconciled before any
installation's ownership is trusted. Two active receipts cannot own the same
skill, even when it is absent. Bundle IDs and transaction IDs cannot repeat.
Removal receipts reserve no skill ownership.

## Installed receipt

```json
{
  "schema_version": 1,
  "bundle_id": "team",
  "transaction_id": "0123456789abcdef0123456789abcdef",
  "status": "installed",
  "version": "autumn",
  "content_digest": "<64 lowercase hexadecimal characters>",
  "inventory_digest": "<64 lowercase hexadecimal characters>",
  "entries": [
    {"path": "review", "kind": "directory"},
    {
      "path": "review/SKILL.md",
      "kind": "file",
      "size": 6,
      "sha256": "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
      "executable": false
    },
    {"path": "review/empty", "kind": "directory"}
  ]
}
```

The digest placeholders above are explanatory; actual receipts require the exact
hashes. `spec/fixtures/receipts.json` supplies a complete valid record
for `hello\n`, the installed file bytes as hexadecimal, and independently assembled
inventory framing bytes. Its inventory SHA-256 was also checked with `shasum`.

Receipts are UTF-8 JSON without a byte order mark. All displayed top-level fields
are required. Unknown fields, duplicate object keys at any depth, non-JSON numeric
constants, and incorrectly typed values are errors. Field and entry order do not
affect meaning. Writers emit entries sorted by UTF-8 path bytes and readers return
that order. No field is coerced.

| Field | Validation |
| --- | --- |
| `schema_version` | Integer `1`; booleans, floats and unsupported versions are rejected. |
| `bundle_id` | Valid bundle identifier matching the filename. |
| `transaction_id` | Exactly 32 lowercase hexadecimal characters; a unique transaction identity, compatible with UUID hexadecimal representation. |
| `status` | Exactly `installed` or `removed`. |
| `version` | Nonblank Unicode scalar string for installed receipts, preserved verbatim without normalization; surrogate code points are rejected. |
| `content_digest` | Bundle content SHA-256 using [content encoding 1](bundle-format.md#content-digest-encoding-1). |
| `inventory_digest` | SHA-256 using inventory encoding 1 below; recomputed on every read. |
| `entries` | Complete nonempty installed inventory, including every skill root and parent directory. |

Directory entries contain exactly `path` and `kind: "directory"`. File entries
contain exactly `path`, `kind: "file"`, `size`, `sha256`, and `executable`. Size is an
unsigned 64-bit integer, never a boolean or float. `sha256` is 64 lowercase
hexadecimal characters and identifies exact file bytes. `executable` is a JSON
boolean. File executable intent comes from the bundle's explicit declarations;
directory executable intent is not recorded.

Every path follows the bundle's portable relative-path rules. Paths must be
unique under NFC-normalized full Unicode case folding. Every parent directory
must be present with exact spelling; a file cannot be a parent. Top-level entries
are valid skill identifiers and directories, and every skill contains an exact
regular `SKILL.md` file. There are no entries outside the owned skill roots.

Inspection compares actual sizes, file SHA-256, directory presence/type, added
paths, and POSIX executable intent against the inventory. It reads actual file
bytes rather than trusting timestamps. When an installation exactly matches its
inventory, its actual content digest must also match `content_digest`; otherwise
the receipt is contradictory and fails closed. The raw-content digest cannot be
reconstructed from file hashes alone when files are modified or missing. The
separate inventory digest validates the recorded inventory without requiring its
files to still exist. These hashes detect inconsistencies; they do not authenticate
state against a malicious writer who can rewrite the receipts and digests.

## Removal receipt

```json
{
  "schema_version": 1,
  "bundle_id": "team",
  "transaction_id": "0123456789abcdef0123456789abcdef",
  "status": "removed",
  "version": null,
  "content_digest": null,
  "inventory_digest": null,
  "entries": []
}
```

A removal receipt is a transaction-tagged tombstone. It carries no version,
digests, inventory, or ownership. Its transaction ID allows a future recovery
operation to recognize an uninstall commit boundary. Any lingering same-name
content is untracked and conflicts with a requested installation. A tombstone
with inventory or non-null version/digests is invalid.

## Inventory digest, encoding 1

All integers are unsigned 64-bit big-endian. Lengths count bytes. Concatenate:

1. The ASCII prefix `flyrail-inventory-v1` followed by one NUL byte.
2. Entry count as one integer.
3. Every entry sorted by UTF-8 encoded path bytes:
   - ASCII `D` for a directory or `F` for a file.
   - UTF-8 path length as one integer, then the path bytes.
   - For a file only: one executable byte (`00` or `01`), the recorded file size
     as one integer, and the 32 raw SHA-256 bytes decoded from `sha256`.

The inventory digest is the lowercase hexadecimal SHA-256 of that byte stream.
It includes empty directories and exact installed path spelling. Bundle ID,
version, transaction ID, JSON order/whitespace, timestamps, and permission bits
other than logical executable intent are excluded. This framing intentionally
differs from the content digest, whose file records contain the actual file bytes.
