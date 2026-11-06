# Embedding Flyrail in a Go host CLI

The module contains two independent example applications under `go/examples/`,
each a `main` package that uses only Flyrail's public API and the standard
library. Each keeps its host version separate from its configuration bundle
label.

| Command | Program | Bundle source | Host version | Bundle label |
| --- | --- | --- | --- | --- |
| [`flyrail-notes`](../examples/flyrail-notes/) | `flyrail-notes words FILE` | Explicit filesystem directory (`OpenBundle`) | `0.4.0` | `notes-2026-09` |
| [`flyrail-checksum`](../examples/flyrail-checksum/) | `flyrail-checksum digest FILE` | `go:embed` files assembled with `NewBundleFromEntries` | `2.1.0` | `checksum-2026-09` |

For a smaller program that shows the same calls, see the two runnable `Example`
functions in the [README](../README.md#quickstart);
they use `LoadBundleFS`, which is what a CLI with an `embed.FS` would do.

Build and run them from `go/` (see [development](development.md#repository-local-caches)
for the environment the Justfile sets):

```sh
go build -o ../.cache/bin/ ./examples/flyrail-notes ./examples/flyrail-checksum
../.cache/bin/flyrail-notes words README.md
../.cache/bin/flyrail-checksum digest README.md
demo="$(cd .. && pwd)/.cache"
../.cache/bin/flyrail-notes ai install \
  --source examples/flyrail-notes/bundle --target "$demo/demo-notes-skills" \
  --state "$demo/demo-notes-state"
../.cache/bin/flyrail-checksum ai install \
  --target "$demo/demo-checksum-skills" --state "$demo/demo-checksum-state"
```

The target passed to `ai` is the skill container itself. Repeat `--target` for
several destinations. `--state` selects the application's own state directory and
constructs a `store.DirectoryStore` there, passed explicitly to every call;
Flyrail has no default store or location of its own. `status` is read-only,
`install` refuses a different owned generation, `update` can also install, and
`uninstall` is source-free: it needs only the bundle ID and the state directory.
`--replace-modified` is available only for update and removal and authorizes
bounded same-owner replacement; it does not acquire foreign ownership. Both
programs pass a five-second `LockTimeout`. The first Ctrl-C cancels a lock wait
and a second one exits the program; the next install, update or uninstall
recovers a transaction that the exit interrupted.

Each command prints one JSON report. Exit `0` means the requested state or
operation succeeded, exit `1` means status differs or a mutation reported a
failure or incomplete result, and exit `2` is invalid CLI usage or source input.
This JSON is an example presentation format, not a Flyrail serialization API.

## Combined configuration

The checksum example also composes its embedded skill with generated
instructions, stdio and HTTP MCP registrations, a before-tool hook, and support
assets:

```sh
../.cache/bin/flyrail-checksum config update \
  --agent claude \
  --project ../.cache/combined-project \
  --node "$(command -v node)" \
  --mcp-command checksum-mcp \
  --mcp-url https://checksum.example.invalid/mcp
```

The sample MCP command and URL are placeholders; the example neither supplies nor
starts a server. It writes environment references without reading their values.
The selected Node 24+ executable must remain available when a native hook
executes. Unsupported translations block the complete configuration before
writes. Other options are `--mcp-arg` (repeatable), `--guidance` and
`--asset-root`; `config status` reports without writing and `config uninstall`
removes.

`just check` builds and vets both programs and runs their tests in the fast
tier. `just check-package` extracts the module outside the repository and builds
and runs both CLIs there; see
[development](development.md#package-qa).
