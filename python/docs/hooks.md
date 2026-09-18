# Bundled hooks

Prepare a runtime snapshot explicitly, then render and install the detached bundle:

```python
from pathlib import Path
from flyrail import (
    Bundle,
    BundleIdentity,
    Command,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HookRuntime,
    Platform,
    RenderContext,
    Target,
    render,
    sync,
)

bundle = Bundle.from_artifacts(
    BundleIdentity("project-hooks", "one"),
    [
        HookArtifact(
            "audit",
            HookEvent.BEFORE_TOOL,
            Command(["audit-command", "--json"]),
            outcomes=[HookOutcome.CONTINUE],
            tools=["Bash"],
        ),
    ],
)
context = RenderContext(
    Target.project("claude", Path.cwd()),
    Platform.LINUX,
    hook_runtime=HookRuntime(Path("/opt/node/bin/node")),
)
rendered = render(bundle, context)
result = sync(bundle, rendered, context.installation(Path.cwd() / ".hook-index"))
```

`HookRuntime` reads packaged resources once and holds immutable bytes. It does not
check the interpreter or run anything. Select the actual Node 24+ executable for
native hosts. OpenCode/Pi use their host runtime with `HookRuntime()`; Codex/Cursor
shell declarations additionally require a compatible `HookShell`. Installation
writes configuration/support assets only. Trust, reload, UI availability and hook
activation remain under the host's control.

Commands receive one canonical JSON input and must return a declared, validated
outcome. IDs determine handler order; tool matching is exact. `CONTINUE` and
`MODIFY_INPUT` preserve normal permission flow. Unsupported semantic combinations
block the complete logical target before writes, including Codex/Cursor portable
argument replacement. An application may supply a native authored hook when its
behavior differs from this protocol.

See the [protocol and host matrix](../../spec/hook-protocol.md) for complete payloads,
error behavior, bounded execution, process cleanup, interpreter/shell prerequisites,
platform boundaries, asset lifecycle, and schema scaffolding.
