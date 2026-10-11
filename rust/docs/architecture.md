# Architecture

Flyrail separates detached bundle snapshots, pure rendering and document edits,
filesystem observation, and transactional persistence. The host application owns
agent selection, command parsing, presentation, and replacement decisions.
Library operations do not run bundled commands or access the network.

| Boundary | Rust modules | Responsibility |
| --- | --- | --- |
| Snapshot | `bundle`, `artifacts`, `content`, `values` | Strict portable inputs and immutable byte/value snapshots. |
| Rendering | `translation`, `destinations`, `rendered` | Explicit agent presets, notices, routes, and dependency edges. |
| Hook protocol | `hooks` and packaged runtime assets | Typed protocol values and detached Node/Bun adapter sources. |
| Editing | `editors` | Pure section, JSON, JSONC, and TOML proposals with scoped baselines. |
| Observation and planning | `configuration`, `inspection`, `lifecycle`, `summary` | Resource summaries, immutable previews, and logical index membership. |
| Agent evidence | `discovery` | Explicit executable and macOS application discovery without target selection. |
| Persistence | private codec, resource-model, and transaction modules | Closed state records, complete revisions, journals, and receipt commit. |
| Native I/O | authority, filesystem, resource-I/O, and security modules | Canonical destinations, safe observation, locks, metadata, and exclusive publication. |

The skill convenience API adapts named subtrees into the same kernel. Every
physical document, tree, or skill container has one adjacent authority, lock,
receipt, and journal across owners. A logical installation index records
membership; it is not a second ownership authority. Disjoint sections or keys in
one document can therefore have independent owners.

Contenders register permanent boundaries before ancestor/descendant validation.
Cooperating writers acquire the index lock first, then resource locks in
deterministic order. Flyrail does not scan a whole home directory for ownership.

Previews bind complete public preimages, ancestors, security evidence, receipts,
and index state. Mutation preflights known conflicts, records pending membership,
then commits resources in dependency order. Receipt publication is the
per-resource commit boundary. Before it, recovery restores a recognized complete
revision; after it, recovery performs cleanup only.

Full resource verification is **O(tree bytes)** for a managed tree: Flyrail must
read and hash the complete owned revision to distinguish an intact tree from a
modified one. This cost is parity-inherent across implementations and is not an
extra Rust-only scan. Document resources similarly bind complete relevant bytes
and metadata even when ownership is limited to a section or structured selection.

Document editing owns selected content and syntax while preserving current
unselected bytes and file metadata. Whole-file and tree claims use explicit mode
contracts. On supported POSIX platforms, unsupported metadata is refused instead
of being silently weakened.

[Resource authority](../../spec/resource-authority.md),
[state encoding](../../spec/receipt-format.md),
[index semantics](../../spec/configuration-state.md), and
[transactions](../../spec/transaction-protocol.md) define the shared durable
contracts. [Porting guidance](../../spec/porting.md) describes the required
cross-language behavior.
