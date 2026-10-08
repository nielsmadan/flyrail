# Bundled hooks

Prepare a `HookRuntime` snapshot explicitly, attach it to a render context, then
render and install the detached bundle:

```rust
use flyrail::{
    Agent, Artifact, Bundle, BundleIdentity, Command, HookArtifact, HookEvent,
    HookOutcome, HookRuntime, Platform, RenderContext, Target, render,
};
use std::path::PathBuf;

let root = PathBuf::from("project").canonicalize()?;
let bundle = Bundle::from_artifacts(
    BundleIdentity::new("project-hooks", "one")?,
    [Artifact::Hook(HookArtifact::new(
        "audit",
        HookEvent::BeforeTool,
        Command::new(["audit-command", "--json"], Vec::<(String, String)>::new(), None)?,
        10_000,
        [HookOutcome::Continue],
        ["Bash"],
    )?)],
    [],
    [],
)?;
let runtime = HookRuntime::new(Some(PathBuf::from("/opt/node/bin/node")), None)?;
let context = RenderContext::new(Target::project(Agent::Claude, &root)?, Platform::Linux)?
    .with_hook_runtime(runtime);
let rendered = render(&bundle, &context)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`HookRuntime::new` snapshots the three packaged runtime source files. It validates
an explicitly supplied Node route but does not probe or execute the interpreter.
Native hosts require an absolute Node 24+ executable. OpenCode and Pi can use
their existing host runtimes with no Node path. Codex and Cursor shell
declarations also require an appropriate `HookShell`.

Installation writes configuration and support assets only. Trust, reload,
discovery, user-interface availability, and activation remain host-controlled.
Check `RenderedBundle::notices` for prerequisites and activation guidance.

Commands receive canonical protocol JSON and must return one of their declared
`HookOutcome` values. Handler IDs determine order; tool names match exactly.
The runtime bounds input, output, stderr, JSON depth, handler count, per-handler
timeouts, and total event time. It validates output before combining effects and
terminates descendant processes after completion or failure.

Rust exposes typed `HookRequest`, `HookResponse`, `HookJson`, `HookText`, and
`HookEffects` values for protocol integrations. `HookText` can retain JavaScript
UTF-16 strings that are not valid Rust scalar `String` values; use `as_str` when a
scalar representation exists or `units` for exact code units. `HookFailure`
formats stable `flyrail-hook:CODE` errors.

Unsupported event/outcome/host combinations block the entire logical rendering.
An application can instead supply a native authored hook with an exact `Audience`;
the renderer never silently substitutes or drops portable behavior.

See the shared [hook protocol and host matrix](../../spec/hook-protocol.md) for
payloads, outcome behavior, process cleanup, platform limits, and runtime
prerequisites.
