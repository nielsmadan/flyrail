# Configuration snapshots and document editing

`Bundle` accepts immutable skills, instructions, MCP registrations, hooks, native
payloads, support assets, and dependency edges. Schema-1 skill bundles and
schema-2 configuration bundles feed the same resource lifecycle.

```go
identity, err := flyrail.NewBundleIdentity("project-guide", "autumn")
if err != nil {
	return err
}
guide, err := flyrail.NewInstructionArtifact("guide", "Run the project checks before committing.\n")
if err != nil {
	return err
}
bundle, err := flyrail.NewBundle(identity, []flyrail.Artifact{guide}, nil, nil)
```

Bundles also load from a source:

| Constructor | Source |
| --- | --- |
| `LoadBundleFS(fsys)` | An `io/fs.FS`, typically an `embed.FS`; file modes are ignored and there are no source roots. |
| `OpenBundle(ctx, dir)` | A directory on disk. |
| `OpenBundleZip(ctx, path, root)` | A member directory inside a ZIP archive. |
| `NewBundleFromEntries(manifest, entries)` | Manifest bytes and `BundleEntry` values. |
| `NewBundleFromValue(manifest, entries)` | The same manifest as a typed `Value`. |

Every constructor returns a detached snapshot that remains valid after source
changes. Executable bits come only from declared intent (`BundleEntry.Executable`
or the manifest), never from physical file modes. Archives are bounded before
anything is allocated: 256 MiB for the archive, 100,000 members, 64 MiB per
member and 512 MiB in total; an over-limit archive fails with a `*BundleError`.
Strict JSON is limited to 512 levels of nesting.

`SkillArtifact` contains `TreeContent`; `InstructionArtifact` contains text;
`MCPArtifact` contains a `Command` or an `HTTPTransport`; `HookArtifact`
declares an event, command, timeout, outcomes, and optional exact tool names.
`NativeArtifact` combines an `Audience`, relative destination, family, and a
`Content` value.

`CommandArgument` holds literal text (`ArgText`) or an `AssetRef`;
`EnvironmentValue` holds literal text (`EnvText`) or an `EnvRef`. References
preserve names without reading the environment. A support asset must have an
explicit dependent, and a referenced asset path must exist in its detached tree.
Relative ordinary command working directories are anchored by the render
context; asset working directories use `AssetRef`.

`Content` is a sealed interface over:

- `FileContent`: complete bytes and `FileMode`;
- `TreeContent`: portable files, directories, and executable intent;
- `SectionContent`: text plus default or caller-supplied boundary lines;
- `StructuredContent`: JSON, JSONC, or TOML selector, typed value, and optional
  array placement.

`RenderedArtifact` adds an absolute destination, optional tree subtree,
`RequireExisting`, and optional JSON/JSONC `DocumentSchema` scaffolds.
`RenderedBundle` snapshots artifacts, dependency edges, notices, routing context,
and its render digest.

Snapshot types have unexported fields, constructors that validate and deep-copy
their inputs, and accessors that return copies. See the
[API conventions](api.md#conventions).

## Pure shared-document edits

`EditDocument` accepts the existing bytes and whether they exist, requests,
complete prior ownership, and provenance. It reads or writes no files:

```go
content, err := flyrail.NewSectionContent("guide", "Run the project checks.\n", nil)
if err != nil {
	return err
}
request, err := flyrail.NewSetRequest(content, flyrail.AcquireConflict)
if err != nil {
	return err
}
created, err := flyrail.EditDocument([]byte("# Project\n"), true, []flyrail.DocumentRequest{request}, nil, flyrail.DocumentProvenance{})
if err != nil {
	return err
}
retire, err := flyrail.NewRetireRequest(created.Ownership[0].Claim())
if err != nil {
	return err
}
retired, err := flyrail.EditDocument(created.After, created.AfterExists, []flyrail.DocumentRequest{retire}, created.Ownership, created.Provenance)
```

After the second edit, `retired.After` is `# Project\n` again. `AcquireConflict`
refuses an unowned selection, even when bytes match. `AcquireTakeover` records a
first-acquisition baseline for restoration. `AcquireAdopt` requires the desired
selection to match and later removes it without restoring a baseline.

Pass the complete `Ownership` and `Provenance` returned by one edit into the
next. `DocumentEdit.Applicable()` is false when a request conflicts or owned
content has changed. Malformed or ambiguous documents and invalid selectors
return `*ValueError`.

JSONC and TOML preserve foreign comments, spelling, and newline bytes outside
owned syntax. JSON compares structured values semantically. Array members use
stable native keys or unique exact typed identities. Persist every
`SelectionSnapshot`, including its layout bytes.

TOML parsing and printing follow Python's tomlkit where Rust's parser differs,
so the Go and Python implementations produce identical bytes for the same
edit. Two consequences are worth knowing. Integers wider than 64 bits and offset
datetimes outside the years 1 to 9999 after conversion to UTC are not
representable. For a table that shares its enclosing array-of-tables name and is
split into out-of-order parts, tomlkit drops one table and Go reproduces that;
such shapes do not occur in agent configuration files.

See the shared [configuration format](../../spec/configuration-format.md) and
[document editing contract](../../spec/document-editing.md).
