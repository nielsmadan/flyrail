# Translate a bundle for explicit agents



`render` turns a detached `Bundle` into owned resources for the existing lifecycle.
It performs no filesystem or environment reads. Construct explicit `Target` values
first; their convenience constructors can snapshot the process cwd/home and only
retain documented routing variables. Supply `home` and `env={}` for isolation.

```python
from pathlib import Path

from flyrail import (
    Bundle,
    BundleIdentity,
    Command,
    EnvRef,
    InstructionArtifact,
    McpArtifact,
    Platform,
    RenderContext,
    Target,
    inspect_installation,
    remove,
    render,
    sync,
)

root = Path("project").resolve()
bundle = Bundle.from_artifacts(
    BundleIdentity("project-tools", "autumn"),
    [
        InstructionArtifact("guide", "Run the project checks before committing.\n"),
        McpArtifact("tools", "tools", Command(["project-server"], {"TOKEN": EnvRef("TOKEN")})),
    ],
)
context = RenderContext(Target.project("codex", root), Platform.LINUX)
desired = render(bundle, context)
target = context.installation(root / ".cache" / "project-tools")
result = sync(bundle, desired, target)
observed = inspect_installation(bundle.id, target)
configured_current = observed.matches(bundle, desired)
removed = remove(bundle.id, target)
```

The renderer emits Codex `AGENTS.md` and a named `mcp_servers.tools` table, forwarding
`TOKEN` by name. Neither rendering nor installation reads its value. Every
mutation result must be checked for errors, partial publication and recovery.
`desired.supported` is false when an exact request cannot be represented; its
`NoticeKind.UNSUPPORTED` notices carry stable `code` values and readable reasons.
Submitting that result to `sync` or `apply_preview` makes no writes, including no
locks or recovery cleanup. Existing installed content remains owned.

`capabilities(context)` returns immutable family summaries with separate `portable`
and `native` flags, limitations and prerequisites. Portable availability means the
family has a route, not that every transport, value or cwd is supported. `render`
checks the actual request. See the dated shared
[capability and destination matrix](../../spec/translation.md) for precise limits
and primary-source links.

`RenderContext` requires an explicit platform and supports Cursor `Surface.IDE`
and Copilot `Surface.VSCODE`; the default is `Surface.CLI`. Optional absolute paths
are `execution_root`, `instruction_path`, `mcp_path`, `native_root` and `asset_root`.
The last two default to the selected project/user root and its `.flyrail-assets`
directory. A relative `Command.cwd` anchors to `execution_root`, defaulting to the
project root/user home. A bundled cwd uses `AssetRef` instead.

Use `instruction_destination(context)`, `mcp_destination(context)` and
`context.skills` to inspect routing without reads. An unavailable preset returns
`None`; rendering a request for it produces an unsupported notice. Explicit paths
represent a destination the caller has already selected, with that surface's
schema. For example, choose an absolute profile `mcp.json` for VS Code user MCP.
Copilot CLI also accepts `mcp_path=root / ".github/mcp.json"`, subject to the
precedence of `.mcp.json` at the same root. No active-profile or existing-alternate
discovery happens implicitly.

Use `render_many(bundle, contexts)` for one installation consumed by several
explicit agents, and a single `InstallationTarget` index for that combined result.
Order is deterministic. The lifecycle canonicalizes shared physical resources and
deduplicates identical claims. Claude and Copilot CLI share compatible stdio
records in `.mcp.json`; Copilot's optional local type is omitted. Incompatible MCP
records at the same named entry produce a collision. For selected Copilot project
routes, different named `mcpServers` records in `.mcp.json` and `.github/mcp.json`
also produce a precedence collision: the lower-priority file cannot isolate the
server. Opaque native payloads receive a discovery notice for caller review.
Disjoint structured claims in one document must also agree on JSON, JSONC or TOML
format; differing formats produce an unsupported result before lifecycle writes.
Any unsupported context blocks this combined logical target;
applications wanting independent installations may render and synchronize them
separately after inspecting all collisions.

Copilot CLI env values accept `EnvRef` as `${NAME}` without resolving the variable.
Literal env strings containing `$NAME`, `${NAME}` or `${NAME:-default}` are
unsupported because the host expands them. Copilot also accepts anchored ordinary
and asset cwd. These env rules do not establish interpolation in argv, URL, cwd
or bearer headers; the exact conservative limits are in the shared matrix.

A rendering snapshots routing context into the immutable preview. The rendering
digest covers destinations, contents and dependencies; informational context and
notices do not change that digest. Reuse the same private index for retargeting,
inspection and source-free removal. An unchanged version label with changed
rendered bytes or destinations is an update.

Native artifacts select their exact agent/scope/surface/platform audience and route
relative to project root or user home, independent of the skill container. They
retain authored `.mdc`, `.instructions.md`, `CLAUDE.md`, `AGENTS.md` and other native
payload semantics. To use a native alternative to unsupported portable content,
the caller constructs its desired bundle with that alternative; the renderer does
not silently drop a portable artifact because native content was also supplied.

Support trees use explicit `AssetRef` values in argv/cwd and direct `Dependency`
edges. The renderer binds each reference to a content-addressed installed revision.
See the shared contract for validation and failure ordering. No install step runs
scripts, installs Pi's required external MCP adapter, contacts servers or activates
an agent. Current registration does not prove the host loaded it.
