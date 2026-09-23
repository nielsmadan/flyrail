# Portable agent translation

Verified against the primary sources linked below on 2026-09-12. This contract
covers local skills, instructions, MCP registrations and explicit native payloads.
Portable hooks use the [versioned hook protocol](hook-protocol.md), packaged
runtime snapshots and concrete native/extension bridges. Native hook bytes do not
establish portable support for additional semantic outcomes.

A renderer consumes a detached bundle snapshot and an explicit agent, scope,
surface, platform and routing snapshot. It returns immutable rendered resources,
dependency edges, routing context and structured notices. It must not read files,
resolve environment references, discover installed agents or active profiles, run
commands, contact servers, grant trust or enable tool approvals.

The Python entry points are `render(bundle, RenderContext(...))` and
`render_many(bundle, contexts)`. The latter describes one logical installation
consumed by several explicitly selected agents. A logical installation with any
unsupported requested artifact returns no publishable artifacts and an unsupported
notice. That blocking spans every selected context, not only the refusing one: one
untranslatable agent in the list zeroes the artifacts for the supported agents too.
A host that seeds its context list from agent detection must filter out agents whose
reported capabilities are unsupported before calling `render_many`. The lifecycle
rejects it before locks, recovery or other writes, retaining previous content and
pending recovery evidence. Registration current and host activation remain separate
states.

## Destinations and surfaces

Paths in the project columns are relative to the selected project root. User
paths use the explicitly selected home. The six supported skill presets remain
unchanged apart from the verified Copilot CLI relocation described below.

| Agent / surface | Project instructions | User instructions | Project MCP | User MCP |
| --- | --- | --- | --- | --- |
| Claude CLI | `CLAUDE.md` | `.claude/CLAUDE.md` | `.mcp.json` | `.claude.json` |
| Codex CLI | `AGENTS.md` | `.codex/AGENTS.md` | `.codex/config.toml` | `.codex/config.toml` |
| OpenCode CLI | `AGENTS.md` | `.config/opencode/AGENTS.md` | `opencode.json` | `.config/opencode/opencode.json` |
| Pi CLI | `AGENTS.md` | `.pi/agent/AGENTS.md` | `.pi/mcp.json` | `.pi/agent/mcp.json` |
| Cursor CLI / IDE | `AGENTS.md` | Explicit caller-selected path required | `.cursor/mcp.json` | `.cursor/mcp.json` |
| Copilot CLI | `.github/copilot-instructions.md` | `.copilot/copilot-instructions.md` | `.mcp.json` | `.copilot/mcp-config.json` |
| Copilot VS Code | `.github/copilot-instructions.md` | `.copilot/instructions/<identity>.instructions.md` | `.vscode/mcp.json` | Explicit profile `mcp.json` required |

`droid` is detectable but carries no row here: it has no supported destinations, and
`render`/`render_many` refuse it unconditionally, returning no artifacts and an
`agent-unsupported` notice for every artifact the render context selects. Its
absence from the table is deliberate, not an unlisted destination pending a
fixture. A skill preset may resolve a droid container path — the Python
implementation anchors one at `.factory/skills` so a target constructs — but that
path is not a verified destination and is never used: every skill convenience
entry point refuses a droid target before it resolves, locks, reads or writes
anything below it, so an implementation must leave the droid root untouched in
both scopes. The six presets listed above are the only supported ones.

Claude, Codex, OpenCode, Pi, Cursor and Copilot project skill containers are
`.claude/skills`, `.agents/skills`, `.opencode/skills`, `.pi/skills`,
`.cursor/skills` and `.github/skills`. User containers are `.claude/skills`,
`.agents/skills`, `.config/opencode/skills`, `.pi/agent/skills`, `.cursor/skills`
and `.copilot/skills`. VS Code uses the Copilot containers above. Registration
and filesystem tests support macOS, Linux and Windows path contexts; native host
activation is not inferred from a platform-independent rendering test.

Relocation is specific to content family and surface:

- `CLAUDE_CONFIG_DIR` relocates Claude user skills and instructions. Its effect on
  the global MCP `.claude.json` location is not established by the inspected
  sources; a selected relocation requires explicit `mcp_path` for user MCP.
- `CODEX_HOME` relocates Codex user instructions and MCP, while skills remain at
  the separately documented shared `.agents/skills` location.
- `XDG_CONFIG_HOME/opencode` relocates OpenCode user defaults. `OPENCODE_CONFIG`
  selects the user render's MCP document; `OPENCODE_CONFIG_DIR` is additional
  discovery and does not replace these default containers.
- `PI_CODING_AGENT_DIR` relocates Pi user instructions, skills and adapter MCP.
- `COPILOT_HOME` relocates Copilot CLI user instructions, skills and MCP. VS Code
  Agent Host's documented user skills/instructions remain under `.copilot` in the
  selected home; this renderer does not generalize the CLI relocation to VS Code.
- `CURSOR_CONFIG_DIR` and Cursor's XDG handling are documented for CLI settings,
  not these content families. They do not relocate the presets here.

A caller may select an absolute instruction or MCP document explicitly. This
supports existing OpenCode `opencode.jsonc`, Copilot `.github/mcp.json`, Pi
`.mcp.json`, Codex `AGENTS.override.md` and custom routes known to the caller.
No filesystem existence check chooses between alternates. VS Code routes require
an `mcp.json` filename and retain the VS Code schema. Agent Host's native
`.mcp.json` and `mcp-config.json` use the Copilot CLI schema instead.

Primary destination references:
[Claude memory](https://code.claude.com/docs/en/memory),
[Claude configuration](https://code.claude.com/docs/en/claude-directory),
[Codex instructions](https://developers.openai.com/codex/agent-configuration/agents-md),
[Codex skills](https://developers.openai.com/codex/skills),
[OpenCode rules](https://opencode.ai/docs/rules),
[OpenCode config](https://opencode.ai/docs/config),
[OpenCode skills](https://opencode.ai/docs/skills),
[Pi skills and config](https://github.com/earendil-works/pi/tree/main/packages/coding-agent/docs),
[Cursor CLI](https://cursor.com/docs/cli/using),
[Cursor rules](https://cursor.com/docs/context/rules),
[Cursor CLI settings](https://cursor.com/docs/cli/reference/configuration),
[Copilot CLI configuration](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference),
[Copilot CLI instructions](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-custom-instructions),
[VS Code instructions](https://code.visualstudio.com/docs/copilot/customization/custom-instructions),
[VS Code skills](https://code.visualstudio.com/docs/agent-customization/agent-skills).

## Instruction ownership and activation

Portable instructions become owned managed sections. Their marker identity is the
SHA-256 hex digest of the UTF-8 compact JSON array `[bundle_id, artifact_id]`.
Markers therefore identify bundle/artifact, independently of consuming agent.
Several agents reading the same physical `AGENTS.md` can share an identical claim.
The lifecycle canonicalizes physical paths and deduplicates identical content;
conflicting bodies or overlapping claims block publication.

VS Code user instructions use a dedicated `.instructions.md` file containing
`---`, `applyTo: "**"`, `---`, a blank line, and the instruction text. This
always-on metadata is required for automatic matching. An explicit path ending in
`.mdc` or `.instructions.md` on another route requires native authored content,
so translation does not invent its matching semantics. Native instruction files
preserve their bytes, including frontmatter and imports.

Cursor's documented user rules live in its UI; this renderer supplies no invented
user filesystem preset. Explicit native paths remain possible. Codex consumes
`AGENTS.override.md` before `AGENTS.md`, limits combined instruction bytes to
32 KiB by default, and snapshots instructions for a session. Notices expose these
conditions without changing host settings. VS Code user modular files target Agent
Host sessions; local extension-host sessions can use different discovery settings.

## MCP semantic capabilities

Each registration owns one named server map entry. Unrelated settings remain
foreign; JSONC/TOML comments are preserved by the shared editor. The renderer
sets no tool allowlists, approval rules, trust flags or package installations.

| Host / surface | Map / format | Stdio | Environment references | Authored cwd | HTTP bearer environment |
| --- | --- | --- | --- | --- | --- |
| Claude CLI | `mcpServers` / JSON | `command`, `args`, `env` | `${NAME}` | Unsupported | `Authorization: Bearer ${NAME}` |
| Codex CLI | `mcp_servers` / TOML | `command`, `args`, `env` | `env_vars`, same-name forwarding only | Supported | `bearer_token_env_var` |
| OpenCode CLI | `mcp` / JSONC | `type: local`, command array, `environment` | `{env:NAME}` | Supported | `type: remote`, `Authorization: Bearer {env:NAME}` |
| Pi adapter | `mcpServers` / JSON | `command`, `args`, `env`, `literalEnv: true` | Portable env maps with references unsupported | Supported | `auth: bearer`, `bearerTokenEnv` |
| Cursor CLI / IDE | `mcpServers` / JSON | `type: stdio`, `command`, `args`, `env` | `${env:NAME}` | Unsupported | `type: http`, `Authorization: Bearer ${env:NAME}` |
| Copilot CLI | `mcpServers` / JSON | `command`, `args`, `env`; optional `type` defaults to local | `${NAME}` in env values | Supported | Unverified; unsupported |
| Copilot VS Code | `servers` / JSONC | `type: stdio`, `command`, `args`, `env` | `${env:NAME}` | Supported | `type: http`, `Authorization: Bearer ${env:NAME}` |

Pi requires the external **pi-mcp-adapter v2.32.1 schema**. Flyrail neither installs
nor verifies the running adapter. A prerequisite notice names the version.
`literalEnv: true` bypasses command-secret evaluation and interpolation for literal
environment maps. Arguments and cwd still interpolate, while command itself is
verbatim. Portable environment-reference maps, including mixed literal/reference
maps, are refused. Dedicated `bearerTokenEnv` reads the named variable in the
adapter only when `auth: bearer` is present; both fields are emitted.

Plain strings always mean literal bytes. Codex's static argv/env values remain
literal. For interpolating hosts, exact unsupported boundaries cover `${...}` in
Claude, Cursor and VS Code values, `{env:...}` and `{file:...}` in
OpenCode, and `${...}`, `$env:...`, `{env:...}` in Pi's interpolated fields. No
verified portable escape is assumed. Pi literal env maps bypass those checks.
Native payloads provide an explicit authored alternative. Windows rendering
rejects case-insensitive environment-key collisions such as `Mode` and `MODE`.

Copilot CLI's documented env substitution supports `$NAME`, `${NAME}` and
`${NAME:-default}`. Literal env strings containing these tokens are unsupported;
`EnvRef` emits `${NAME}` and remains unresolved until the host reads it. This
evidence applies to env values. Command/argv, URL and cwd retain a conservative
unsupported boundary for `${...}` with code `literal-interpolation-unverified`;
their substitution semantics have not been established. The renderer does not
infer a bearer-header syntax from env expansion. Ordinary and installed-asset cwd
use the same explicit anchors described below.
[Copilot CLI field reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference#local-server-configuration-fields).

Claude and Copilot CLI can share project `.mcp.json` with identical command,
args and env records. Flyrail omits Copilot's optional local `type`, so compatible
requests deduplicate through the lifecycle. Each host's semantic checks still
apply; unsupported literals or incompatible named entries block the combined target.

Copilot `.github/mcp.json` is an alternate discovery route with lower precedence
than `.mcp.json` at the same selected project root. It does not isolate a
same-name server from the higher-priority file. For a selected Copilot CLI project,
the renderer compares named `mcpServers` structured claims at those two routes.
Different values for the same name return `copilot-mcp-precedence-collision`,
independently of context order. Identical values are compatible. Opaque native
content or other selector shapes receive a discovery notice requiring caller
review of effective definitions. This comparison uses only selected destinations;
existing files, ancestor repositories, plugins and active sessions are not read.
[Copilot per-repository discovery](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers#adding-per-repository-mcp-servers).

Every shared document must use one structured editor format, including disjoint
selectors. JSON, JSONC and TOML claims with differing formats return
`document-format-collision`; no implicit JSON/JSONC normalization occurs. Like
other unsupported results, these renders contain no publishable artifacts and
cannot create lifecycle locks, authority records or other writes.

VS Code forwards its configured servers to Agent Host. Agent Host does not read
`.vscode/mcp.json` directly. User MCP uses an explicit chosen profile document;
the renderer never guesses the active profile. Forwarding, extension presence,
trust, credentials and activation remain caller/host concerns.

Primary MCP references:
[Claude MCP](https://code.claude.com/docs/en/mcp),
[Codex MCP](https://developers.openai.com/codex/mcp),
[OpenCode MCP](https://opencode.ai/docs/mcp-servers),
[Cursor MCP](https://cursor.com/docs/context/mcp),
[Copilot CLI MCP](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers),
[VS Code MCP schema](https://code.visualstudio.com/docs/agents/reference/mcp-configuration),
[VS Code variables](https://code.visualstudio.com/docs/reference/variables-reference),
[VS Code Agent Host](https://code.visualstudio.com/docs/agents/concepts/agent-host),
[Pi adapter v2.32.1 types](https://github.com/nicobailon/pi-mcp-adapter/blob/v2.32.1/types.ts),
[Pi adapter v2.32.1 implementation](https://github.com/nicobailon/pi-mcp-adapter/blob/v2.32.1/server-manager.ts),
[Pi adapter interpolation](https://github.com/nicobailon/pi-mcp-adapter/blob/v2.32.1/utils.ts).

## Command and asset anchors

An ordinary relative `Command.cwd` is relative to the renderer's execution root,
which defaults to the selected project root or explicit user home. The emitted
cwd is absolute. It never points back to the original bundle source and never
uses an unrecorded process cwd. A host without a verified cwd field returns
unsupported rather than discarding the authored value.

`AssetRef(asset_id, path=None)` refers to an installed support tree or an exact
file/directory beneath it. Manifest form is `{"asset":"runtime"}` or
`{"asset":"runtime","path":"server.js"}`. It is accepted in argv and cwd.
References must name snapshotted support assets and direct dependency edges;
paths must exist, cwd must name a directory, and argv[0] must name a file.
Arbitrary strings are never interpreted as asset paths.

Support destination is `<asset_root>/<bundle_id>/<asset_id>/<revision>`, where
`asset_root` defaults to `.flyrail-assets` under the selected project/user root.
Revision is SHA-256 of the semantic encoding of the `TreeContent` model record.
Byte changes and executable intent therefore acquire different destinations.
Commands bind their absolute asset revision paths. Dependency edges publish assets
before referring configuration, retire references before old assets, and preserve
referenced old revisions through partial publication failure and source-free removal.

Native relative destinations anchor to project root or user home, with an explicit
`native_root` override. Audience matching uses agent, scope, surface and optional
platform. Dependencies on unavailable native audiences block the requesting
logical target. Native content remains opaque and carries no automatic asset
placeholder substitution.

`fixtures/translations.json` contains portable source/expected destination/native
value vectors, positive Pi adapter registrations, explicit unsupported cases and
an asset-reference manifest. Consumers call the actual renderer and lifecycle.
`cwd_from_root` expresses an expected absolute cwd relative to the test root.
`combined_cases` checks shared records, selected-route precedence and document
format compatibility with one explicit context list and logical installation.
The asset-reference case in `fixtures/configurations.json` additionally exercises
all snapshot source types. No fixture records live agent configuration.
