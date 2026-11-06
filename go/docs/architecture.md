# Architecture

Flyrail separates detached bundle snapshots, pure rendering and document edits,
filesystem observation, and transactional persistence. The host application owns
agent selection, command parsing, presentation, and replacement decisions.
Library operations do not run bundled commands or access the network.

## Packages

| Package | Responsibility |
| --- | --- |
| `flyrail` | Every public type and all domain logic that touches those types. |
| `flyrail/store` | The `Store` interface, its errors, `DirectoryStore` and `MemoryStore`. It imports nothing from `flyrail`. |
| `flyrail/store/storetest` | The conformance kit a custom store runs from its own tests. |
| `internal/*` | Type-free helpers: `jsonx`, `unicodex`, `zipx`, `pathx` (Rust `Path` semantics), `sysx`, `darwin`, `native`, `hookruntime`, `seams`, `interopbridge`. None imports a public package. |

Within `flyrail`, files are grouped by the same boundaries the other
implementations use:

| Boundary | Files | Responsibility |
| --- | --- | --- |
| Snapshot | `bundle`, `sources`, `artifacts`, `content`, `values` | Strict portable inputs and immutable byte/value snapshots. |
| Rendering | `translation`, `destinations`, `render`, `rendered` | Explicit agent presets, notices, routes, and dependency edges. |
| Hook protocol | `hooks`, `hooktranslation`, `internal/hookruntime` | Typed protocol values and the embedded Node/Bun adapter sources. |
| Editing | `editing*`, `toml_*` | Pure section, JSON, JSONC, and TOML proposals with scoped baselines. |
| Observation and planning | `configuration`, `inspection`, `observation*`, `lifecycle*`, `summary` | Resource summaries, immutable previews, and logical index membership. |
| Agent evidence | `discovery` | Explicit executable and macOS application discovery without target selection. |
| Persistence | `state_*`, `transaction`, `recovery`, `resource_*` | Closed state records, complete revisions, journals, and receipt commit. |
| Native I/O | `internal/native`, `internal/sysx`, `internal/darwin` | Canonical destinations, safe observation, metadata, and exclusive publication. |

`store` imports nothing from `flyrail`, and `internal/*` imports neither public
package, so the import graph has no cycles. That is why `flyrail` holds every
type and all logic that touches those types: its sealed interfaces (unexported
marker method) can only be implemented in `flyrail`.

The state records and their codec are unexported: they are a durable format, not
an API, and an export cannot be retracted. The interop driver at
`internal/cmd/flyrail-interop` reaches them through `internal/interopbridge`,
whose variables `flyrail` sets once in `init` and never mutates afterwards.

## Reference behaviour

Rust is the behavioural reference and the shared `spec/` wins over it. Durable
formats, digests, rendered artifacts, edited documents and cross-language error
text match Rust byte for byte, including OS error text of the form
`[Errno 2] No such file or directory: '<path>'`. `internal/sysx` carries errno
text tables generated per OS, because glibc and libSystem word some errors
differently. Parser-detail diagnostics for JSONC, TOML and ZIP syntax are Go's
own and are compared by error code only. Deliberate divergences:

- **TOML** parsing and printing follow Python's tomlkit, not Rust's parser; see
  [configuration](configuration.md). Rust's parser drops non-ASCII characters.
- **Render notices** follow Rust: no family activation notices for native
  artifacts, and per-artifact notices ordered as Rust orders them. Python and
  Swift differ; digests and artifacts agree.
- **Strict JSON** is `internal/jsonx`, a direct port of Rust's reader. The
  standard `jsontext` package is only an acceptance oracle in the fuzz target,
  because it cannot reproduce Rust's messages and their order.
- **Limits** that Rust lacks (resource, ZIP, value nesting, record size) follow
  Swift's values; see [support](support.md#resource-and-input-limits).
- **State store paths** that are not valid UTF-8 are refused up front with
  `invalid_state`, as Python does; Rust records them lossily and leaves the
  store permanently incomplete.
- **Staging** creates every node below the staging root descriptor-relatively
  (`mkdirat`/`openat` with `O_EXCL|O_NOFOLLOW` from a parent reached component
  by component with `O_DIRECTORY|O_NOFOLLOW`), and bounds observation listings
  by the node budget. Directory modes are applied as Rust does, immediately; a
  deepest-first second pass would stage read-only trees that publish and clean
  up could then not process.
- **Case folding** is the generated Unicode 16.0.0 full fold, then NFC from
  `golang.org/x/text` (Unicode 17.0.0, as Rust's `unicode-normalization`);
  tests assert both versions.

## No cgo

The library builds with `CGO_ENABLED=0` for darwin and linux on amd64 and arm64.
Exclusive publication uses `Renameat2` with `RENAME_NOREPLACE` on Linux and
`RenamexNp` with `RENAME_EXCL` on macOS, both from `golang.org/x/sys`. The four
libSystem functions that package lacks, `acl_get_file`, `acl_get_entry`,
`acl_to_text` and `acl_free`, plus `fcntl(F_GETPATH)`, are called through
trampolines in `internal/darwin`: a `//go:cgo_import_dynamic` binding to
`libSystem.B.dylib`, a per-architecture assembly `JMP` stub, and a
`//go:linkname` to the runtime's `syscall` entry points. Every ACL object is
released with `acl_free`. The `//go:linkname` trampolines depend on runtime internals of the pinned Go
toolchain; after a Go upgrade, the darwin tests and `just check` confirm they
still link and run. Every raw system call goes through the EINTR retry in
`internal/sysx`. The race detector needs cgo, so the integration tier runs with
cgo enabled; do not export `CGO_ENABLED=0` globally.

Lint excludes gosec G103 (the trampolines need `unsafe`) and G304 (a filesystem
library takes caller paths) globally instead of annotating every call site.

## Ownership and locking

The skill convenience API adapts named subtrees into the same kernel. Every
physical document, tree, or skill container has one authority, lock, receipt,
and journal across owners, held in the application's `store.Store` rather than
beside the resource. A logical installation index, held in the same store, is
membership bookkeeping; it is not a second ownership authority. Disjoint
sections or keys in one document can therefore have independent owners.

Contenders register permanent boundaries before ancestor/descendant validation.
Cooperating writers acquire the index lock first, then resource locks in
deterministic order. Flyrail does not scan a whole home directory for ownership.

Locks are `flock(LOCK_EX|LOCK_NB)` on a descriptor owned by the lock handle,
opened `O_NOFOLLOW|O_CLOEXEC`. Lock files are never removed, and `Unlock` is
idempotent. Multiple locks are released in reverse order. See
[lifecycle](lifecycle.md#contexts-and-lock-timeouts) for how timeouts and
contexts apply.

Previews bind complete public preimages, ancestors, security evidence, receipts,
and index state. Mutation preflights known conflicts, records pending membership,
then commits resources in dependency order. Receipt publication is the
per-resource commit boundary. Before it, recovery restores a recognized complete
revision; after it, recovery performs cleanup only.

Resource payload and state files are created with
`O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC` and published with an exclusive rename.
Library code never uses a truncating open, `os.WriteFile`, `os.MkdirAll` or
`os.Rename` for resource publication, and never panics on a public path. The
only overwriting rename is `DirectoryStore` record replacement under the
store's mutation lock.

Full resource verification is **O(tree bytes)** for a managed tree: Flyrail must
read and hash the complete owned revision to distinguish an intact tree from a
modified one. Document resources similarly bind complete relevant bytes and
metadata even when ownership is limited to a section or structured selection.

Document editing owns selected content and syntax while preserving current
unselected bytes and file metadata. Whole-file and tree claims use explicit mode
contracts. On supported systems, unsupported metadata is refused instead of
being silently weakened.

[Resource authority](../../spec/resource-authority.md),
[state encoding](../../spec/receipt-format.md),
[index semantics](../../spec/configuration-state.md),
[the state store contract](../../spec/state-store.md), and
[transactions](../../spec/transaction-protocol.md) define the shared durable
contracts. [Porting guidance](../../spec/porting.md) describes the required
cross-language behavior.
