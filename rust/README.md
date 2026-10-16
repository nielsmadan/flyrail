# Flyrail for Rust

The `flyrail` crate is the Rust implementation of Flyrail's synchronous library
for reversible skills, instructions, MCP registrations and hooks at explicit
local destinations.

Flyrail snapshots bundle content, renders it for an explicitly selected agent,
and applies it through per-resource ownership records. It can report installed
agent evidence when asked, but never selects an agent automatically. It does not
prompt, print, start bundled commands, or contact a network service.

## Quickstart

Add the crate from this workspace, then construct or load a bundle and select a
destination:

```rust
use flyrail::{Bundle, DirectoryStore, OperationStatus, StateStore, Target, install, uninstall};
use std::path::Path;
use std::sync::Arc;

let bundle = Bundle::from_directory("path/to/flyrail")?;
let target = Target::directory(Path::new(".cache/demo-skills"))?;
let store: Arc<dyn StateStore> = Arc::new(DirectoryStore::new(".cache/demo-skills-state")?);

let installed = install(&bundle, [&target], &store)?;
assert!(matches!(
    installed[0].status(),
    OperationStatus::Applied | OperationStatus::Unchanged
));

let removed = uninstall(bundle.id(), [&target], &store)?;
assert!(matches!(
    removed[0].status(),
    OperationStatus::Applied | OperationStatus::Unchanged
));
# Ok::<(), Box<dyn std::error::Error>>(())
```

There is no default state store: construct one explicitly (for example a
`DirectoryStore` in the application's own state directory) and pass it to
every lifecycle call. `MemoryStore` suits tests and dry runs. See
[the public API](docs/api.md#state-stores) for the full contract.

The convenience lifecycle accepts only skill-only bundles without support
dependencies. For instructions, MCP registrations, hooks, native payloads, or
support assets, create a [`RenderContext`](docs/translation.md), call `render`,
and use the general configuration lifecycle.

## Documentation

- [Public API](docs/api.md)
- [Bundle and content configuration](docs/configuration.md)
- [Translation and destinations](docs/translation.md)
- [Lifecycle and recovery](docs/lifecycle.md)
- [Inspection](docs/inspection.md)
- [Bundled hooks](docs/hooks.md)
- [Architecture](docs/architecture.md)
- [Examples](docs/examples.md)
- [Platform support](docs/support.md)
- [Development](docs/development.md)

## Development

From this directory, run `just check` for the fast unit gate. Use
`just check-integration` for the full test tier, `just format` to format Rust
code, `just lint` for Clippy, and `just test` for tests.

The workspace includes two host CLI examples:

- [`flyrail-notes`](examples/filesystem/) loads an explicitly selected
  filesystem bundle.
- [`flyrail-checksum`](examples/packaged/) embeds its skill bundle and can
  render a combined four-family project configuration.

Run `just check-examples` for their end-to-end suites, `just package-list`
for quick archive file inspection, and `just check-package` to build and extract
the root and both example crate archives in a checkout-local vendored registry.
The extracted-package check builds and runs both CLIs without their source trees.

Hook runtime integration accepts Node 24+, npm, and Bun 1.4.2+ locally, or
prints a `SKIP` notice when they are unavailable. CI requires the pinned
Node 24.21.0, npm 11.19.0, and Bun 1.4.2 toolchain exactly.

The crate supports macOS and Linux. Windows is unsupported.
