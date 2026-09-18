# Architecture

Flyrail separates detached bundle snapshots, rendered resources, pure document
edits and filesystem transactions. The application owns agent selection, command
parsing, presentation and replacement decisions. Runtime library operations do not
execute bundled commands or use the network.

| Boundary | Entry points | Responsibility |
| --- | --- | --- |
| Snapshot | `bundle`, `artifacts`, `content` | Strict portable inputs and immutable byte/value snapshots. |
| Rendering | `translation`, `destinations`, `rendered` | Pure agent presets, explicit destinations, notices and dependency edges. |
| Hook execution | `hooks`, `runtime/` | Detached adapter assets; execution later inside native or existing host runtimes. |
| Editing | `editors` | Pure section/JSON/JSONC/TOML proposals with scoped baselines and creation provenance. |
| Observation/planning | `configuration`, `_resource_plan`, `_lifecycle` | Resource summaries, immutable previews and logical index membership. |
| Persistence | `_codec`, `_resource_models`, `_resource_transaction` | Closed state records, complete revisions, journal and receipt commit. |
| Native I/O | `_resource_io`, `_security`, `_filesystem` | Safe observations, private metadata, security preservation, locks and exclusive publication. |

The skill convenience API adapts named subtrees into this same kernel. Every
physical document/tree/skill container has one adjacent authority, lock, receipt
and journal across all owners. A logical installation index is membership
bookkeeping, not another source of ownership. This permits independent applications
to own disjoint sections or keys in one document.

Each contender registers its permanent boundary before validating ancestor and
descendant boundaries. Incompatible concurrent parent/child admissions can both
fail, but cannot both succeed. Cooperating writers acquire the index lock first,
then resource locks in deterministic order. No home-wide ownership scan is used.

Previews bind complete preimages and private state. Mutations preflight known
target conflicts, persist pending membership before the first publication, then
commit resources in dependency order. Receipt publication is the resource commit
boundary. Before it, recovery restores only recognized complete revisions; after
it, recovery cleans only. Unexpected data remains available for examination.

A document edit owns selected content and syntax, while current file metadata and
unselected bytes remain live. Whole file/tree claims have an explicit mode
contract. POSIX and Windows adapters capture supported native metadata or refuse
the operation. TOML imports pinned `tomlkit` lazily; other core paths use the
standard library.

[Resource authority](../../spec/resource-authority.md),
[state encoding](../../spec/receipt-format.md),
[index semantics](../../spec/configuration-state.md) and
[transactions](../../spec/transaction-protocol.md) define the language-neutral
contracts. [Porting](../../spec/porting.md) explains how another implementation
must verify them.
