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
