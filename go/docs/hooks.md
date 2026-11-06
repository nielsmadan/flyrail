# Bundled hooks

Prepare a `HookRuntime` snapshot explicitly, attach it to a render context, then
render and install the detached bundle:

```go
identity, err := flyrail.NewBundleIdentity("project-hooks", "one")
if err != nil {
	return err
}
command, err := flyrail.NewCommand([]flyrail.CommandArgument{flyrail.ArgText("audit-command"), flyrail.ArgText("--json")}, nil, nil)
if err != nil {
	return err
}
audit, err := flyrail.NewHookArtifact(flyrail.HookArtifactParams{
	ID:            "audit",
	Event:         flyrail.EventBeforeTool,
	Command:       command,
	TimeoutMillis: 10000,
	Outcomes:      []flyrail.HookOutcome{flyrail.OutcomeContinue},
	Tools:         []string{"Bash"},
})
if err != nil {
	return err
}
bundle, err := flyrail.NewBundle(identity, []flyrail.Artifact{audit}, nil, nil)
if err != nil {
	return err
}
runtime, err := flyrail.NewHookRuntime("/opt/node/bin/node", "")
if err != nil {
	return err
}
target, err := flyrail.ProjectTarget(flyrail.AgentClaude, root)
if err != nil {
	return err
}
rc, err := flyrail.NewRenderContext(target, flyrail.PlatformLinux, flyrail.RenderOptions{HookRuntime: runtime})
if err != nil {
	return err
}
rendered, err := flyrail.Render(bundle, rc)
```

`NewHookRuntime` snapshots the three packaged runtime source files (`bridge.mts`,
`process.mts` and `runner.mts`, embedded in the library). It validates an
explicitly supplied Node route but does not probe or execute the interpreter.
Native hosts require an absolute Node 24+ executable. OpenCode and Pi can use
their existing host runtimes with an empty Node path. Codex and Cursor shell
declarations also require an appropriate `HookShell` (`ShellPOSIX` or
`ShellCmd`).

Installation writes configuration and support assets only. Trust, reload,
discovery, user-interface availability, and activation remain host-controlled.
Check `RenderedBundle.Notices()` for prerequisites and activation guidance.

Commands receive canonical protocol JSON and must return one of their declared
`HookOutcome` values. Handler IDs determine order; tool names match exactly.
The runtime bounds input, output, stderr, JSON depth, handler count, per-handler
timeouts, and total event time (`HookInputLimit`, `HookOutputLimit`,
`HookErrorLimit`, `HookTextLimit`, `HookJSONDepthLimit`, `HookHandlerLimit`,
`HookEventTimeoutLimitMillis`). It validates output before combining effects and
terminates descendant processes after completion or failure. The Go library only
writes these files; the Node and Bun runtime that executes them is separate.

Go exposes typed `HookRequest`, `HookResponse`, `HookJSON`, `HookText`, and
`HookEffects` values for protocol integrations. `HookText` can retain JavaScript
UTF-16 strings that are not valid Go strings; use `UTF8()` when a string
representation exists or `Units()` for exact code units. Protocol functions
report every failure as `*HookError`, whose text is the stable
`flyrail-hook:<code>` form.

Unsupported event/outcome/host combinations block the entire logical rendering.
An application can instead supply a native authored hook with an exact `Audience`;
the renderer never silently substitutes or drops portable behavior.

A CLI that spawns Node or a hook command from the same process as a Flyrail
operation should read the note on
[lock timeouts](lifecycle.md#contexts-and-lock-timeouts).

See the shared [hook protocol and host matrix](../../spec/hook-protocol.md) for
payloads, outcome behavior, process cleanup, platform limits, and runtime
prerequisites.
