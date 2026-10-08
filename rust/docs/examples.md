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
cargo run -p flyrail-example-notes -- ai install \
  --source examples/filesystem/bundle --target ../.cache/demo-notes-skills
cargo run -p flyrail-example-checksum -- ai install \
  --target ../.cache/demo-checksum-skills
```

The target passed to `ai` is the skill container itself. Repeat `--target` for
several destinations. `status` is read-only, `install` refuses a different owned
generation, `update` can also install, and `uninstall` is source-free.
`--replace-modified` is available only for update/removal and authorizes bounded
same-owner replacement; it does not acquire foreign ownership.

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
