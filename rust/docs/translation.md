# Translate bundles for explicit agents

`render` turns a detached `Bundle` into owned resources for one explicit
`RenderContext`. It performs no filesystem or environment reads. `Target`
constructors may snapshot selected process values before rendering; use explicit
home/environment constructors for isolation.

```rust
use flyrail::{
    Agent, Artifact, Bundle, BundleIdentity, Command, EnvRef, InstructionArtifact,
    McpArtifact, McpTransport, Platform, RenderContext, Target, render,
};
use std::path::PathBuf;

let root = PathBuf::from("project").canonicalize()?;
let bundle = Bundle::from_artifacts(
    BundleIdentity::new("project-tools", "autumn")?,
    [
        Artifact::Instruction(InstructionArtifact::new(
            "guide",
            "Run the project checks before committing.\n",
        )?),
        Artifact::Mcp(McpArtifact::new(
            "tools",
            "tools",
            McpTransport::Command(Command::new(
                ["project-server"],
                [("TOKEN", EnvRef::new("TOKEN")?)],
                None,
            )?),
        )?),
    ],
    [],
    [],
)?;
let context = RenderContext::new(Target::project(Agent::Codex, &root)?, Platform::Linux)?;
let desired = render(&bundle, &context)?;
assert!(desired.supported());
# Ok::<(), Box<dyn std::error::Error>>(())
```

This emits a Codex `AGENTS.md` section and `mcp_servers.tools` TOML selection.
The environment variable is forwarded by name; rendering does not read its value.

`RenderContext::new` uses the CLI surface. `with_surface` selects supported IDE or
VS Code routes. Builder methods set explicit absolute execution, instruction,
MCP, native, asset, and hook-runtime routes. `instruction_destination`,
`mcp_destination`, and `RenderContext::skills` expose routing without reads.

An unavailable preset returns `None`; rendering a requested artifact then produces
an unsupported notice. `capabilities` reports portable/native family availability,
limitations, and prerequisites, but request-specific validation still happens in
`render`.

Droid is detectable but not translatable. It has no instruction or MCP
destination, all four capabilities report unsupported, and rendering returns one
`agent-unsupported` notice per selected artifact with no output. A Droid context
also blocks every output from a combined `render_many` request.

Use `render_many` for one logical installation consumed by several explicit
contexts. It deduplicates identical physical claims and rejects incompatible
collisions. An unsupported context blocks the combined result; synchronize
separate installations when independent failure is desired.

Structured claims sharing a destination must agree on document format. Claude and
Copilot CLI can share compatible stdio records in `.mcp.json`. Copilot project
routes also account for host precedence between `.mcp.json` and
`.github/mcp.json`. Opaque native records produce activation guidance where the
effective precedence cannot be verified.

Native artifacts match their exact `Audience` and route relative to the selected
project root or user home. They preserve authored native semantics. A native
artifact does not silently replace unsupported portable content; the caller must
construct the desired alternative explicitly.

Support trees are routed under the selected asset root. `AssetRef` in argv/cwd and
explicit `Dependency` edges bind references to installed revisions. Installation
does not run scripts, install external adapters, contact servers, or activate an
agent.

The rendering digest covers artifacts and dependencies, not informational notices
or routing context. Changed rendered bytes or destinations under an unchanged
bundle label still compare as a different desired generation.

See the dated shared [capability and destination matrix](../../spec/translation.md)
for exact host schemas and limitations.

## Global Loadout source mappings

The Loadout preview translates against the effective profile and the selected
declared source. The [Loadout compatibility contract](../../spec/loadout.md)
defines how profile inheritance, overrides, and source order affect routing.

| Bundle content | Loadout source mapping and limits |
| --- | --- |
| Skills | Install the authored tree and a per-skill policy for the selected harnesses with `render = false`. Loadout-filtered files and conflicting skills or overrides block the preview. |
| Instructions | Add a bundle-qualified Markdown fragment and one member per applicable existing instruction order array. Shadowed members and unsupported selectors block the preview. |
| MCP | Add a server table to `mcp.toml` for every configured MCP slice. Stdio supports command, arguments, and literal environment values. A working directory or stdio environment reference blocks the preview. HTTP supports a URL and an optional bearer variable name. Conflicting names in participating sources block the preview. |
| Hooks | Use selected JSON fragments for Claude, Codex, and Cursor, and bridge modules for OpenCode and Pi. Supported events, outcomes, and runtime requirements still apply. A hook selector shared across harnesses with different protocols blocks the preview. Droid hooks are unsupported. |
| Native artifacts | User CLI whole-file JavaScript and TypeScript modules map to OpenCode `plugins/` and Pi `extensions/` under `module-config/<harness>/`. Only top-level `.js`, `.mjs`, and `.ts` files at the matching native destinations are supported. |

Copilot is outside the Loadout harness set. The preview also rejects disabled
slices and incompatible custom routes. Pass an explicit `HookRuntime` in
`LoadoutOptions` for bundles with hooks. See [Bundled hooks](hooks.md) for
runtime details.
