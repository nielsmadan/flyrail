# Embedding Flyrail in a Rust host CLI

The workspace contains two independent example applications. Each keeps its host
version separate from its configuration bundle label and uses only Flyrail's
public API.

| Crate | Command | Bundle source | Host version | Bundle label |
| --- | --- | --- | --- | --- |
| [`flyrail-example-notes`](../examples/filesystem/) | `flyrail-notes words FILE` | Explicit filesystem directory | `0.4.0` | `notes-2026-09` |
| [`flyrail-example-checksum`](../examples/packaged/) | `flyrail-checksum digest FILE` | `include_bytes!` memory snapshot | `2.1.0` | `checksum-2026-09` |

Build and run them from `rust/`:

```sh
cargo build -p flyrail-example-notes -p flyrail-example-checksum
cargo run -p flyrail-example-notes -- words README.md
cargo run -p flyrail-example-checksum -- digest README.md
demo="$(cd .. && pwd)/.cache"
cargo run -p flyrail-example-notes -- ai install \
  --source examples/filesystem/bundle --target "$demo/demo-notes-skills" \
  --state "$demo/demo-notes-state"
cargo run -p flyrail-example-checksum -- ai install \
  --target "$demo/demo-checksum-skills" --state "$demo/demo-checksum-state"
```

The target passed to `ai` is the skill container itself. Destinations may not
contain `..` components, so the commands spell the checkout's `.cache`
directory as an absolute path. Repeat `--target` for several destinations.
`--state` selects the application's own state directory
and constructs a `DirectoryStore` there, passed explicitly to every call;
flyrail has no default store or location of its own. `status` is read-only,
`install` refuses a different owned generation, `update` can also install, and
`uninstall` is source-free. `--replace-modified` is available only for
update/removal and authorizes bounded same-owner replacement; it does not
acquire foreign ownership.

Each command prints one JSON report. Exit `0` means the requested state or
operation succeeded, exit `1` means status differs or a mutation reported a
failure/incomplete result, and exit `2` is invalid CLI usage or source input. This
JSON is an example presentation format, not a Flyrail serialization API.

## Combined configuration

The checksum example also composes its packaged skill with generated instructions,
stdio and HTTP MCP registrations, a before-tool hook, and support assets:

```sh
cargo run -p flyrail-example-checksum -- config update \
  --agent claude \
  --project ../.cache/combined-project \
  --node "$(command -v node)" \
  --mcp-command checksum-mcp \
  --mcp-url https://checksum.example.invalid/mcp
```

The sample MCP command and URL are placeholders; the example neither supplies nor
starts a server. It writes environment references without reading their values.
The selected Node 24+ executable must remain available when a native hook executes.
Unsupported translations block the complete configuration before writes.

Run `just check-examples` for both end-to-end suites. `just check-package` builds
and extracts the root and example crate archives in a checkout-local vendored
registry, then builds and runs the example binaries without their source trees.
It verifies packaged bytes, executable intent, source-free removal, lifecycle
behavior, and the combined four-family flow.

## Offer a global Loadout source

Offer Loadout alongside direct destinations when discovery returns a usable setup
with eligible sources. The example takes the harnesses the user requested and
uses callbacks for the source choice and preview review. Returning `None` from
the source chooser or declining the preview ends the flow without source writes.

```rust
use flyrail::{
    Agent, Bundle, LoadoutDiscovery, LoadoutOptions, LoadoutPreview, LoadoutResult,
    LoadoutSource, Platform, StateStore, apply_loadout, discover_loadout, preview_loadout,
};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

fn offer_loadout(
    bundle: &Bundle,
    home: &Path,
    environment: &BTreeMap<String, String>,
    harnesses: &[Agent],
    platform: Platform,
    store: Arc<dyn StateStore>,
    choose_source: impl FnOnce(&[&LoadoutSource]) -> Option<String>,
    review: impl FnOnce(&LoadoutPreview) -> bool,
) -> Result<Option<LoadoutResult>, Box<dyn std::error::Error>> {
    let setup = match discover_loadout(home, None, environment)? {
        LoadoutDiscovery::Absent { .. } => return Ok(None),
        LoadoutDiscovery::Invalid { diagnostic, .. } => {
            return Err(format!("{} at {}", diagnostic.message(), diagnostic.path().display()).into());
        }
        LoadoutDiscovery::Available(setup) => setup,
    };
    let sources = setup.eligible_sources(bundle, harnesses);
    if sources.is_empty() {
        return Ok(None);
    }
    let Some(source_name) = choose_source(&sources) else {
        return Ok(None);
    };
    let destination = setup.select(&source_name, harnesses, false)?;
    let plan = preview_loadout(bundle, &destination, store, LoadoutOptions::new(platform))?;
    if review(&plan) && plan.applicable() {
        return Ok(Some(apply_loadout(&plan, Duration::from_secs(2))));
    }
    Ok(None)
}
```

In the review callback, show the planned edits, diagnostics, and scope. Before
confirming an applicable plan, save its `location()` with the bundle ID and
state store identity. If the user accepts a wider scope, select again with
shared scope enabled and ask them to review the new preview.

Use `result.lifecycle().status()` for the source operation and
`result.sync_required()` to tell the user to run `loadout sync` when required.
For uninstall, reopen the same store, pass the saved bundle ID and location to
`preview_loadout_removal`, let the user review it, and call `apply_loadout`.
See the [Loadout lifecycle](lifecycle.md#global-loadout-sources).
