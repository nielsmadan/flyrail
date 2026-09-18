# Resource state encoding, schema 2

All bundle input formats use the same resource receipts and installation indexes.
A physical document, tree or skill container has one adjacent authority; see
[resource authority](resource-authority.md) for canonicalization and naming.
Receipts reconcile every owner's claims at that boundary, including missing content.

## Layout

```text
<resource-parent>/
    <resource>
    .flyrail-<portable-basename-sha256>.state/
        lock
        authority.json
        receipt.json
        transaction.json
        staging/resource
        backup/resource
<caller-selected-index>/
    lock
    index.json
```

The hash uses the portable basename encoding from the authority specification,
not the bundle ID. Resource and index locks are permanent. State entries are
optional before first use. Uninstall may leave empty receipts and bookkeeping;
an empty receipt grants no ownership. Unknown entries, unsafe links, ambiguous
paths and invalid records prevent mutation.

Atomic metadata preparation uses a sibling `.next` file. Its presence is
recovery evidence, not permission to infer a committed record. The
[transaction protocol](transaction-protocol.md) defines recognized preparations.

## Closed tagged records

State is UTF-8 JSON. Writers sort object keys, omit inter-token whitespace and add
one trailing LF. Readers reject duplicate keys, unknown record tags/fields,
noncanonical encoded values and invalid types; no coercion or legacy migration
takes place. JSON booleans are not integers.

| Value | Encoding |
| --- | --- |
| Record | `{"type":"RecordTag","fields":{...}}` with every declared field |
| Enum | `{"enum":"EnumTag","value":"wire-value"}` |
| Bytes | `{"bytes":"<lowercase hexadecimal>"}` |
| Absolute path | `{"path":"<slash-separated absolute path>"}` |
| Semantic JSON/TOML value | `{"value":<semantic encoding 1 record>}` |
| Sequence | JSON array, retaining order |
| Plain scalar | String, integer, boolean or null |

All declared record fields are required even when their values match a model
default. Empty requirement sequences are encoded as `[]`, never omitted.

These tags are wire names; another language may expose idiomatic native types.
Paths reject traversal. Identifiers/digests use their separately validated
contracts. Transaction IDs are 32 lowercase hexadecimal characters; SHA-256
digests are 64. Every schema field is integer `2`.

| Record tag | Required fields |
| --- | --- |
| `ResourceRef` | `destination`, `kind`, `state_root`, `schema_version` |
| `Receipt` | `resource`, `transaction_id`, `claims`, `provenance`, `created_directories`, `schema_version` |
| `OwnedClaim` | `claim`, `claim_id`, `bundle_id`, `artifact_id`, `version`, `bundle_digest`, `render_digest`, `selection`, `installed`, `disposition`, `baseline`, `requirements`, `stable_requirements`, `schema_requirements` |
| `DocumentSchema` | `format`, `key`, `value` |
| `Generation` | `version`, `bundle_digest`, `render_digest`, `membership` |
| `Index` | `bundle_id`, `context_digest`, `current`, `pending`, `previous`, `residual`, `schema_version` |
| `Journal` | `resource`, `previous`, `receipt`, `before`, `staged`, `published`, `backup`, `ancestors`, `schema_version` |
| `Revision` | `nodes` |
| `Node` | `path`, `data`, `mode`, `uid`, `gid`, `device`, `inode`, `security` |
| `Ancestor` | `path`, `device`, `inode`, `mode`, `uid`, `gid`, `security` |

`ResourceKind` is `document`, `tree` or `skill-container`.
`Disposition` is `created`, `taken-over` or `adopted`.
`Claim`, `Ownership` and selector tags follow
[resource authority](resource-authority.md). `OwnedSelection`,
`SelectionSnapshot`, `ArrayPosition`, `CreatedContainer` and
`DocumentProvenance` follow [document editing](document-editing.md);
persist all fields, including selected syntax, separator and TOML layout bytes.
`DocumentSchema.format` uses the `DocumentFormat` enum (`json` or `jsonc`),
`key` is a top-level object-key string, and `value` uses the semantic scalar
encoding above. Its fields match the [configuration model](configuration-format.md#bundle-and-rendering-digests),
with values encoded by the state rules in this document.

## Receipt validation

`authority.json` is a `ResourceRef`. Its state path must be derived from its
canonical destination. The receipt's resource must match that header. Claim IDs
are recomputed from physical destination, ownership kind and selector; artifact
IDs only attribute the content.

Claims must be mutually nonoverlapping and compatible with the resource kind.
Shared section/structured claims use `selection` and leave whole `installed`
and `baseline` revisions null. Whole file/tree/subtree claims use `installed`;
only takeover has a nonempty whole baseline. Both forms retain the first baseline
through updates. `requirements` records required resource/claim pairs for safe
asset retirement. `stable_requirements` is a unique subset of those pairs;
`schema_requirements` contains `DocumentSchema` records with distinct keys and
formats matching the owned structured selection. Their lifecycle is defined in
[configuration state](configuration-state.md#conformance).

A revision is an ordered tree with one root node at empty relative path.
Empty `nodes` means absence; `data:null` identifies a directory and empty bytes
identify an empty file. Every parent exists in the revision. Modes and native
identity/security evidence bind observation and recovery; they are not substituted
with file timestamps. The revision content digest hashes the state encoding of
the ordered `(path, data, mode)` tuples. Full recovery equality also includes
identity, ownership and security evidence.

Resource provenance belongs to the shared document, so the first creator's
retirement does not remove another owner's required container or separator.
For skill containers, `created_directories` retains directory `Node` evidence for
the resource root and namespace parents created outside owned subtrees. Other
resource kinds require an empty sequence. This evidence is shared across owners:
the last removal prunes only recorded empty unchanged directories. Foreign contents
or changed metadata prevent pruning and relinquish that directory's evidence.

## Index validation

Each membership entry pairs a `ResourceRef` with its claim ID sequence.
`residual` contains resource references; traversal uses the union of current,
pending, previous and residual membership. Duplicate IDs, contradictory resource
kinds and inconsistent state paths are invalid.

The context digest binds the caller's canonical index path and sorted string
context pairs through the state encoding. Readers require the requested bundle
and context to match. The index locates claims; it never grants ownership or
overrides a resource receipt. See [index lifecycle](configuration-state.md).

State hashes detect inconsistent data, not a malicious writer able to rewrite
state and hashes together. Full synthetic records and executable recovery cases
live in `fixtures/configurations.json`.

## Security evidence framing

POSIX ancestor security uses compact UTF-8 JSON with sorted object keys and no
whitespace: `acl` is lowercase hex of native ACL text bytes, `flags` is an integer,
`schema_version` is integer `1`, and `xattrs` is an array of raw attribute-name/value
hex pairs sorted by raw name bytes. An absent ACL is empty hex. This portable
framing retains opaque native payloads without Python-specific representations.

Windows security bytes are the native owner/group/DACL descriptor returned by
`GetFileSecurityW`. Resource-level macOS provenance evidence is the ASCII bytes
`com.apple.provenance`, one NUL, then its opaque native value. Unsupported resource
metadata refuses mutation; see the implementation's [support boundary](../python/docs/support.md).
