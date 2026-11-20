# Global Loadout source installation

The compatibility baseline is Loadout 0.11.0 at revision
`057725af7b7c3d162addc88c843fb17b8805429a`. An installation changes inputs
to Loadout. The host application offers this destination and the user selects
it. The result tells the host that Loadout sync is required.

## Discovery and selection

The caller supplies a home directory and an environment snapshot. A nonempty
`XDG_CONFIG_HOME` selects its `loadout/config.toml`. Otherwise the registration
is `.config/loadout/config.toml` beneath home. A caller can supply the
configuration directory directly instead of the environment variable. Only a
missing registration means absence. Unreadable files, invalid TOML, and a
registration whose source or profile cannot be used produce diagnostics.

The registration names an absolute source root or a path beginning with
`~/` and optionally an active profile, defaulting to `default`. Its
`harnesses` entries describe defaults for new projects. Determine active
harnesses, slices, source order, and selectors from the effective manifest.
The global source has a `loadout.toml` marker. The named sibling
`<profile>.toml` declares a profile through `extends`. When the sibling is missing, a
profile named by a legacy target in `loadout.toml` can select that target.
Inheritance resolves from the oldest ancestor through the active profile.
At each level apply `remove` paths before merging fields. Arrays replace
earlier arrays. The fields of each target in `instructions`, `permissions`,
and `skills` merge independently. Reject cycles, non-sibling profile
references, escaped files, and unsupported declarations. Keep the manifest
path that contributed each effective selector.

A declared source's relative path is relative to the global root. Absolute
and home-relative declarations can name shared roots. Below a declared source,
an input or destination must stay within that source. Only the categories
enabled by its `use` participate. A source with no filter offers every
category defined by the compatibility baseline.

The app presents eligible declared sources and the requested harness coverage.
The destination records the chosen source explicitly and uses only currently
configured slices. Selecting a narrower coverage than the source or selector
can represent needs the caller's explicit acceptance of that wider scope.
Changing an inherited selector or a legacy selector used by all profiles also
needs acceptance. The preview
shows those paths and affected harnesses. Other profiles and projects using a
source may receive its content.

## Source edits

- **Skills** write the named subtree under `skills/`, including authored
  assets. A skill policy selects the requested active skills harnesses and
  sets `render = false` to preserve the authored skill. The policy is an
  individually owned manifest entry. Resolve same-name offerings and overrides
  in all sources participating in the skills slice.
- **Instructions** write one stable bundle-qualified Markdown fragment per
  instruction under `instructions/`. Select it by `source/fragment`, without
  the extension, using one owned member per existing instruction order.
  Preserve the existing order and substitutions. The same requirement applies
  to a shared `[all]` selector and legacy `instructions.<name>.order`.
- **MCP** writes one owned top-level server table per server to `mcp.toml`.
  Stdio uses `transport = "stdio"`, `command`, `args`, and literal `env`.
  HTTP uses `transport = "http"`, `url`, and an optional `auth_env_var`
  with the bearer variable name. Reject cwd and environment references whose
  semantics cannot be represented. The definition applies to all configured
  MCP slices, and a collision in any participating source is an error. In
  particular, a later source must never hide a Flyrail definition. Flyrail
  checks proposed OpenCode and Pi server namespaces against MCP rules in
  participating permission sources.
- **Hooks** use a Loadout JSON fragment selected by the applicable hook list
  or a native bridge module in `module-config/<harness>/`. Only mappings
  verified through Loadout rendering and bridge execution are supported.
  Runtime files and required bundle assets use the selected source. Reject
  disabled slices, unverified mappings, and registrations that cannot be
  scoped without breaking their protocol.
- **Native artifacts** require a known source mapping for their exact scoped
  destination and content kind. Unsupported paths and Copilot report a
  capability failure before any source edits.

## Lifecycle

Planning reads the route and its relevant inputs and reports the proposed
source edits, policy and selector changes, and impact. Revalidate the routing
observations in apply before publishing changes. Changed inputs require a fresh
preview. Store the ownership of individual selections and source payloads
in the application's Flyrail state store. A second installation of the same
bundle cannot claim resources already owned through another locator. Preserve the existing transaction
and recovery boundaries per resource.

The destination locator identifies the canonical global root, profile,
declared source name and canonical source path. Retain it alongside the
appropriate state store. Source-free reads and removal address that
installation's index and its recorded resource locations. Successful source
changes, including removal, need Loadout sync before generated destinations
reflect them. Hooks and MCP servers in the last generated configuration can
reference assets that an update or removal already retired. The host should
coordinate the source change and sync before relying on those registrations.
Hook runtime paths and asset references are local to the installation machine.

## Conformance

`fixtures/loadout.json` uses synthetic trees. A test substitutes its root for
`{root}`, writes the files relative to that root, and uses `home/` as the
explicit home. Results report portable status and diagnostic codes and paths
relative to the test root. Diagnostic messages explain the offending path or
setting. The MCP preview cases exercise disabled slices, scope, collisions,
permission rules, and environment references across the four ports.

Run the opt-in interop gate from the Flyrail root with Node 24 and Bun on
`PATH`. The Python integration gate provisions the Flyrail environment and
the hook tooling. Set `LOADOUT_CHECKOUT` to a Loadout checkout containing the
pinned revision, then provision a separate environment for its parser and
renderer.

```sh
loadout_ref=.cache/loadout-interop/reference
mkdir -p "$loadout_ref"
git -C "$LOADOUT_CHECKOUT" archive 057725af7b7c3d162addc88c843fb17b8805429a \
  pyproject.toml uv.lock src/loadout | tar -x -C "$loadout_ref"
UV_CACHE_DIR="$PWD/.cache/uv/cache" UV_PYTHON_INSTALL_DIR="$PWD/.cache/uv/python" \
  timeout 240 uv sync --project "$loadout_ref" --locked --no-dev --no-install-project
run_dir=$(mktemp -d .cache/loadout-interop/run.XXXXXX)
timeout 900 python/.venv/bin/python spec/check_loadout_interop.py \
  --loadout "$loadout_ref" --evidence "$run_dir/evidence" \
  --producer python --producer go --producer rust --producer swift
```

The gate calls Loadout's `render_global` against each producer's disposable
source and home.
It checks generated instructions, skills, MCP, native modules and the hook
bridge responses, followed by the rendered output after source-free removal.
