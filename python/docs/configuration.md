# Configuration snapshot API

`Bundle` also accepts immutable skills, instructions, MCP registrations, hooks and
their supporting assets. Both bundle schemas feed the same resource lifecycle.
The skill convenience functions accept skill-only bundles; other families use
[pure agent translation](translation.md) and `preview`/`sync` with an `InstallationTarget`.

```python
from flyrail import Bundle, BundleIdentity, InstructionArtifact

bundle = Bundle.from_artifacts(
    BundleIdentity("project-guide", "autumn"),
    [InstructionArtifact("guide", "Run the project checks before committing.\n")],
)
```

`Bundle.from_memory(manifest, entries=())` accepts a schema-1 or schema-2 manifest
as UTF-8 bytes or a mapping. Its `BundleEntry` values are source-relative entries;
parent directories are inferred. `Bundle.from_zip(path, resource="flyrail")`
selects one explicit archive directory. Filesystem and package constructors accept
both schemas. Every constructor returns a detached snapshot with immutable typed
artifacts, assets and dependencies, independent of subsequent source changes.

`SkillArtifact` contains a `TreeContent`; `InstructionArtifact` contains text;
`McpArtifact` contains a `Command` or `HttpTransport`; `HookArtifact` declares an
event, command, timeout and required outcomes. Prepare a `HookRuntime` snapshot
before rendering [bundled hooks](hooks.md). `EnvRef` retains a variable name
without reading it. `AssetRef` checks argv/cwd paths against installed support-tree revisions.
Relative ordinary cwd uses the explicit render execution root, never source storage.
`SupportAsset` contains an explicitly referenced tree in one of
the four families. `NativeArtifact` combines `Audience`, relative destination and
one closed content type: `FileContent`, `TreeContent`, `SectionContent`, or
`StructuredContent`. Audience records agent, scope, product surface and optional
platform. No model executes commands, performs network requests or loads an agent.

`SectionBoundaries` allows an application to manage an existing marker convention.
`SectionContent.selector` supplies the actual boundary pair. `Selector` uses `Key`
and `Member` values, with `Placement` for stable array insertion. `Scalar`,
`ObjectValue` and `ArrayValue` preserve semantic types; `freeze_value` snapshots
ordinary JSON/TOML values. `semantic_bytes` and `value_from_record` implement the
portable typed encoding.

`RenderedBundle` snapshots rendered resources, notices and dependency edges.
Each `RenderedArtifact` names an absolute destination; `subtree` selects an owned
tree inside a skill container, and `require_existing` binds an existing-resource
precondition through publication.
`ResourceAuthority` performs read-only canonicalization and exposes the sibling
state path and lock path. `Claim` computes physical ownership identity and
conservative overlap. These are data/observation primitives; construction does not
admit an ownership claim or create management state. The general
[lifecycle](lifecycle.md) owns receipts, indexes, locks, recovery and publication.

See the shared [configuration format](../../spec/configuration-format.md),
[resource authority](../../spec/resource-authority.md) and
[receipt/index protocol](../../spec/configuration-state.md) for the contracts used
by editors, translation and lifecycle implementations.

## Pure shared-document edits

The editor accepts bytes and returns an immutable proposal; it does not read or
write files. Use it for the shared instruction, MCP and hook documents produced by
an application or renderer:

```python
from flyrail import DocumentRequest, SectionContent, edit_document

created = edit_document(
    b"# Project\n",
    [DocumentRequest.set(SectionContent("guide", "Run the project checks.\n"))],
)
removed = edit_document(
    created.after,
    [DocumentRequest.retire(created.ownership[0].claim)],
    ownership=created.ownership,
    provenance=created.provenance,
)
assert removed.after == b"# Project\n"
```

Pass the resource's complete `ownership` and `provenance` into subsequent edits.
`DocumentRequest.set` defaults to conflicting with unowned content. Explicit
`Acquisition.TAKEOVER` preserves a first-acquisition baseline across updates;
`Acquisition.ADOPT` requires exact text bytes or typed structured values and later
removes the adopted unit without restoration. `DocumentRequest.retire` addresses
the physical claim, allowing source-free removal and attribution renames.

`DocumentEdit.applicable` is false if any request conflicts or any owned selection
has changed. Such results retain the original complete bytes and metadata; other
observations describe proposed work only. Malformed/ambiguous documents and
invalid selector paths raise `ValueError`. A lifecycle must bind the full `before`
revision to its publication/preview and apply the complete result under the
resource lock. The editor never performs recovery or selective rollback.

JSONC and TOML protect syntax inside owned selections and preserve foreign
comments, spelling and newline bytes. JSON permits semantic comparison and
whitespace normalization within rewritten values. Array members use stable native
keys or unique exact typed identities, and takeover baselines retain restoration
anchors. Resource creation provenance survives the first creating claim and
prevents removal of foreign empty containers or added comments. Once every claim
retires, retained foreign content relinquishes creation provenance.

Persist each immutable `SelectionSnapshot` completely, including its `layout`
bytes. TOML uses that versioned, portable metadata for descendant header spelling
when restoring nested MCP tables and arrays of tables. It excludes foreign outer
header comments and trailing gaps, which remain taken from the current document.
The shared document specification defines its encoding and ownership boundary.

TOML support loads pinned `tomlkit` only when a TOML document is edited. Importing
Flyrail or editing plain instruction sections does not load the TOML parser. See
[shared document editing](../../spec/document-editing.md) for boundary, syntax,
array-position and provenance contracts and portable vectors.
