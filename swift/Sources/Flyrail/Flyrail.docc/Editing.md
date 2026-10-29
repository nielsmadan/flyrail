# Shared-document editing

Use `editDocument(source:requests:ownership:provenance:)` for marked text or
JSON, JSONC and TOML selections. The editor performs no I/O. Carry the returned
ownership and provenance into the next edit; publish only applicable proposals.
A blocked request retains the complete source, ownership and provenance.

TOML editing preserves foreign bytes, native scalar spelling, dotted and quoted
keys, table ordering, comments, line endings and array-member trivia. Takeover
records the first selected baseline; retirement restores its native syntax and
original array position. A batch plans related array restorations together.
Header-table representation changes and noncontiguous selections are refused.
Inputs are bounded to 16 MiB, one million tokens and 100 key/value nesting levels.
Foreign nonfinite numbers are preserved but cannot be selected.

`SelectionSnapshot`'s layout-taking initializer validates closed TOML restoration
metadata. `OwnedSelection` additionally binds descendant headers to the claim.
Layout bytes are immutable and retain their original encoding.

Use `matchesSchema(source:requirement:)` to inspect a typed top-level JSON/JSONC
schema field without repairing it. `editSchemaDocument` validates and scaffolds
active requirements in the same edit proposal. Schema fields are resource-level
provenance, not app claims. Active requirements reject overlapping app ownership.
Created fields and blank-source evidence survive until safe retirement; edited
schema fields and retained foreign content relinquish cleanup authority.
