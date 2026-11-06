# Translate bundles for explicit agents

`Render` turns a detached `*Bundle` into owned resources for one explicit
`RenderContext`. It performs no filesystem or environment reads and takes no
context. `Target` constructors take the home directory and a `lookupEnv`
function as arguments, so nothing is read from the process unless you pass it.

```go
identity, err := flyrail.NewBundleIdentity("project-tools", "autumn")
if err != nil {
	return err
}
guide, err := flyrail.NewInstructionArtifact("guide", "Run the project checks before committing.\n")
if err != nil {
	return err
}
token, err := flyrail.NewEnvRef("TOKEN")
if err != nil {
	return err
}
command, err := flyrail.NewCommand(
	[]flyrail.CommandArgument{flyrail.ArgText("project-server")},
	map[string]flyrail.EnvironmentValue{"TOKEN": token},
	nil,
)
if err != nil {
	return err
}
tools, err := flyrail.NewMCPArtifact("tools", "tools", command)
if err != nil {
	return err
}
bundle, err := flyrail.NewBundle(identity, []flyrail.Artifact{guide, tools}, nil, nil)
if err != nil {
	return err
}
target, err := flyrail.ProjectTarget(flyrail.AgentCodex, root)
if err != nil {
	return err
}
rc, err := flyrail.NewRenderContext(target, flyrail.PlatformLinux, flyrail.RenderOptions{})
if err != nil {
	return err
}
desired, err := flyrail.Render(bundle, rc)
```

`root` is an absolute project directory. `desired.Supported()` is true, and the
rendering holds a Codex `AGENTS.md` section and an `mcp_servers.tools` TOML
selection. The environment variable is forwarded by name; rendering does not
read its value.

`NewRenderContext` uses the CLI surface unless `RenderOptions.Surface` selects a
supported IDE or VS Code route. The other `RenderOptions` fields set explicit
absolute execution, instruction, MCP, native, asset, and hook-runtime routes; an
empty field takes the documented default. `InstructionDestination`,
`MCPDestination` and `RenderContext.Skills()` expose routing without reads.
The platform is an argument; a CLI chooses `PlatformMacOS` or `PlatformLinux`
from `runtime.GOOS` itself.

An unavailable preset returns `""`; rendering a requested artifact then produces
an unsupported notice. `Capabilities` reports portable and native family
availability, limitations, and prerequisites, but request-specific validation
still happens in `Render`.

Droid is detectable but not translatable. It has no instruction or MCP
destination, all four capabilities report unsupported, and rendering returns one
`agent-unsupported` notice per selected artifact with no output. A Droid context
also blocks every output from a combined `RenderMany` request.

Use `RenderMany` for one logical installation consumed by several explicit
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
construct the desired alternative explicitly. Where Rust and Python differ in
notice wording or order, Go follows Rust: it adds no family activation notice
for native artifacts.

Support trees are routed under the selected asset root. `AssetRef` in argv/cwd and
explicit `Dependency` edges bind references to installed revisions. Installation
does not run scripts, install external adapters, contact servers, or activate an
agent.

The rendering digest covers artifacts and dependencies, not informational notices
or routing context. Changed rendered bytes or destinations under an unchanged
bundle label still compare as a different desired generation.

See the dated shared [capability and destination matrix](../../spec/translation.md)
for exact host schemas and limitations.
