# Configuration snapshots and document editing

`Bundle` accepts immutable skills, instructions, MCP registrations, hooks, native
payloads, support assets, and dependency edges. Schema-1 skill bundles and
schema-2 configuration bundles feed the same resource lifecycle.

```rust
use flyrail::{Artifact, Bundle, BundleIdentity, InstructionArtifact};

let bundle = Bundle::from_artifacts(
    BundleIdentity::new("project-guide", "autumn")?,
    [Artifact::Instruction(InstructionArtifact::new(
        "guide",
        "Run the project checks before committing.\n",
    )?)],
    [],
    [],
)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Bundle::from_memory` accepts manifest bytes and `BundleEntry` values.
`Bundle::from_memory_value` accepts the same manifest as a typed `Value`.
`Bundle::from_directory` and `Bundle::from_zip` load explicit sources. Every
constructor returns a detached snapshot that remains valid after source changes.

`SkillArtifact` contains `TreeContent`; `InstructionArtifact` contains text;
`McpArtifact` contains `McpTransport::Command` or `McpTransport::Http`;
`HookArtifact` declares an event, command, timeout, outcomes, and optional exact
tool names. `NativeArtifact` combines an `Audience`, relative destination, family,
and a `Content` value.

`CommandArgument` can hold literal text or an `AssetRef`; `EnvironmentValue` can
hold literal text or an `EnvRef`. References preserve names without reading the
environment. A support asset must have an explicit dependent, and a referenced
asset path must exist in its detached tree. Relative ordinary command working
directories are anchored by the render context; asset working directories use
`AssetRef`.

`Content` is closed over:

- `FileContent`: complete bytes and `FileMode`;
- `TreeContent`: portable files, directories, and executable intent;
- `SectionContent`: text plus default or caller-supplied boundary lines;
- `StructuredContent`: JSON, JSONC, or TOML selector, typed value, and optional
  array placement.

`RenderedArtifact` adds an absolute destination, optional tree subtree,
`require_existing`, and optional JSON/JSONC `DocumentSchema` scaffolds.
`RenderedBundle` snapshots artifacts, dependency edges, notices, routing context,
and its render digest.

## Pure shared-document edits

`edit_document` accepts optional existing bytes, requests, complete prior
ownership, and provenance. It reads or writes no files:

```rust
use flyrail::{
    Acquisition, Content, DocumentProvenance, DocumentRequest, SectionContent,
    edit_document,
};

let request = DocumentRequest::set(
    Content::Section(SectionContent::new(
        "guide",
        "Run the project checks.\n",
        None,
    )?),
    Acquisition::Conflict,
)?;
let created = edit_document(
    Some(b"# Project\n".to_vec()),
    &[request],
    &[],
    &DocumentProvenance::default(),
)?;
assert!(created.applicable());

let retired = edit_document(
    created.after().map(<[u8]>::to_vec),
    &[DocumentRequest::retire(
        created.ownership()[0].claim().clone(),
    )?],
    created.ownership(),
    created.provenance(),
)?;
assert_eq!(retired.after(), Some(b"# Project\n".as_slice()));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Acquisition::Conflict` refuses an unowned selection, even when bytes match.
`Takeover` records a first-acquisition baseline for restoration. `Adopt` requires
the desired selection to match and later removes it without restoring a baseline.

Pass the complete `ownership` and `provenance` returned by one edit into the next.
`DocumentEdit::applicable` is false when a request conflicts or owned content has
changed. Malformed or ambiguous documents and invalid selectors return
`ValueError`.

JSONC and TOML preserve foreign comments, spelling, and newline bytes outside
owned syntax. JSON compares structured values semantically. Array members use
stable native keys or unique exact typed identities. Persist every
`SelectionSnapshot`, including its layout bytes.

See the shared [configuration format](../../spec/configuration-format.md) and
[document editing contract](../../spec/document-editing.md).
