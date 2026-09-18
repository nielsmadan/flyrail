import json
from dataclasses import FrozenInstanceError, replace
from pathlib import Path
from typing import Any

import pytest
from conformance import FIXTURES
from test_translation import bundle_of, codes, context

from flyrail import (
    Agent,
    AssetRef,
    Audience,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    Dependency,
    EnvRef,
    Family,
    FileContent,
    HookArtifact,
    HookEvent,
    HttpTransport,
    InstallationTarget,
    InstructionArtifact,
    McpArtifact,
    NativeArtifact,
    Notice,
    NoticeKind,
    OperationStatus,
    Platform,
    RenderContext,
    RenderedBundle,
    SupportAsset,
    Surface,
    Target,
    TargetScope,
    TreeContent,
    capabilities,
    inspect_installation,
    instruction_destination,
    mcp_destination,
    preview,
    remove,
    render,
    render_many,
    sync,
)


@pytest.mark.parametrize(
    ("agent", "variable", "skills", "instructions", "mcp"),
    [
        ("claude", "CLAUDE_CONFIG_DIR", "skills", "CLAUDE.md", ".claude.json"),
        ("codex", "CODEX_HOME", None, "AGENTS.md", "config.toml"),
        (
            "opencode",
            "XDG_CONFIG_HOME",
            "opencode/skills",
            "opencode/AGENTS.md",
            "opencode/opencode.json",
        ),
        ("pi", "PI_CODING_AGENT_DIR", "skills", "AGENTS.md", "mcp.json"),
        ("copilot", "COPILOT_HOME", "skills", "copilot-instructions.md", "mcp-config.json"),
    ],
)
def test_family_specific_relocations(
    tmp_path: Path, agent: str, variable: str, skills: str | None, instructions: str, mcp: str
) -> None:
    selected = RenderContext(
        Target.user(agent, home=tmp_path, env={variable: "~/relocated", "SECRET": "never persist"}),
        Platform.LINUX,
    )
    assert instruction_destination(selected) == tmp_path / "relocated" / instructions
    assert mcp_destination(selected) == (
        None if agent == "claude" else tmp_path / "relocated" / mcp
    )
    assert selected.skills == (
        tmp_path / ".agents/skills" if skills is None else tmp_path / "relocated" / skills
    )
    assert {key for key, _ in selected.target.environment} == {variable}
    assert selected.installation(tmp_path / "index").routing_context == selected.routing_context


def test_surface_relocation_and_explicit_alternates(tmp_path: Path) -> None:
    cursor = RenderContext(
        Target.user(
            "cursor",
            home=tmp_path,
            env={
                "CURSOR_CONFIG_DIR": str(tmp_path / "unused"),
                "XDG_CONFIG_HOME": str(tmp_path / "xdg"),
            },
        ),
        Platform.LINUX,
    )
    assert mcp_destination(cursor) == tmp_path / ".cursor/mcp.json"
    assert instruction_destination(cursor) is None
    vscode = RenderContext(
        Target.user("copilot", home=tmp_path, env={"COPILOT_HOME": str(tmp_path / "cli")}),
        Platform.WINDOWS,
        Surface.VSCODE,
    )
    assert instruction_destination(vscode) == tmp_path / ".copilot/instructions"
    assert vscode.skills == tmp_path / ".copilot/skills"
    assert mcp_destination(vscode) is None
    assert codes(render(bundle_of(McpArtifact("tools", "tools", Command(["server"]))), vscode)) == {
        "mcp-profile-path"
    }
    assert not next(item for item in capabilities(vscode) if item.family is Family.MCP).portable
    override = replace(vscode, mcp_path=tmp_path / "profiles/chosen/mcp.json")
    assert mcp_destination(override) == tmp_path / "profiles/chosen/mcp.json"
    assert next(item for item in capabilities(override) if item.family is Family.MCP).portable
    opencode = RenderContext(
        Target.user(
            "opencode",
            home=tmp_path,
            env={
                "OPENCODE_CONFIG": str(tmp_path / "chosen.jsonc"),
                "OPENCODE_CONFIG_DIR": str(tmp_path / "unused"),
            },
        ),
        Platform.MACOS,
    )
    assert mcp_destination(opencode) == tmp_path / "chosen.jsonc"
    assert instruction_destination(opencode) == tmp_path / ".config/opencode/AGENTS.md"
    assert render(bundle_of(McpArtifact("tools", "tools", Command(["server"]))), opencode).supported
    custom = replace(cursor, instruction_path=tmp_path / "caller-selected/AGENTS.md")
    assert (
        render(bundle_of(InstructionArtifact("guide", "Custom.")), custom).artifacts[0].destination
        == custom.instruction_path
    )


@pytest.mark.parametrize(
    ("agent", "surface", "path"),
    [
        ("cursor", "ide", ".cursor/rules/custom.mdc"),
        ("copilot", "vscode", ".github/instructions/custom.instructions.md"),
        ("claude", "cli", "CLAUDE.md"),
        ("codex", "cli", "AGENTS.override.md"),
    ],
)
def test_native_payloads_keep_authored_bytes_and_project_anchor(
    tmp_path: Path, agent: str, surface: str, path: str
) -> None:
    text = b'---\napplyTo: "src/**"\n---\nAuthored rules.\n'
    authored = NativeArtifact(
        "native",
        Family.INSTRUCTIONS,
        Audience(Agent(agent), TargetScope.PROJECT, Surface(surface), Platform.LINUX),
        path,
        FileContent(text),
    )
    bundle = bundle_of(authored)
    selected = context(tmp_path, agent, surface=surface)
    (item,) = render(bundle, selected).artifacts
    assert item.destination == tmp_path / path
    assert item.content == FileContent(text)
    installation = InstallationTarget(tmp_path / "index")
    assert sync(bundle, render(bundle, selected), installation).status is OperationStatus.APPLIED
    assert (tmp_path / path).read_bytes() == text
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED
    assert render(bundle, replace(selected, platform=Platform.WINDOWS)).artifacts == ()
    assert render(bundle, context(tmp_path, "pi")).artifacts == ()
    user = RenderContext(Target.user(agent, home=tmp_path), Platform.LINUX, Surface(surface))
    assert render(bundle, user).artifacts == ()


def test_native_surface_scope_and_user_root_selection(tmp_path: Path) -> None:
    native = NativeArtifact(
        "rule",
        Family.INSTRUCTIONS,
        Audience(Agent.CURSOR, TargetScope.USER, Surface.IDE),
        "selected.mdc",
        FileContent(b"Native"),
    )
    bundle = bundle_of(native)
    user = RenderContext(Target.user("cursor", home=tmp_path), Platform.LINUX, Surface.IDE)
    assert render(bundle, user).artifacts[0].destination == tmp_path / "selected.mdc"
    assert (
        render(bundle, replace(user, native_root=tmp_path / "explicit")).artifacts[0].destination
        == tmp_path / "explicit/selected.mdc"
    )
    assert render(bundle, replace(user, surface=Surface.CLI)).artifacts == ()
    assert next(item for item in capabilities(user) if item.family is Family.INSTRUCTIONS).native
    hook = NativeArtifact(
        "native-hook",
        Family.HOOKS,
        Audience(Agent.CURSOR, TargetScope.USER, Surface.IDE),
        "hooks.json",
        FileContent(b"{}"),
    )
    assert render(bundle_of(hook), user).supported
    assert not next(item for item in capabilities(user) if item.family is Family.HOOKS).portable
    portable = HookArtifact("hook", HookEvent.STOP, Command(["observe"]))
    assert codes(render(bundle_of(portable), user)) == {"hook-runtime"}


def test_modular_portable_rules_require_host_metadata(tmp_path: Path) -> None:
    bundle = bundle_of(InstructionArtifact("guide", "Always."))
    selected = replace(context(tmp_path, "cursor"), instruction_path=tmp_path / "custom.mdc")
    assert codes(render(bundle, selected)) == {"native-instruction-required"}
    selected = RenderContext(
        Target.user("copilot", home=tmp_path),
        Platform.LINUX,
        Surface.VSCODE,
        instruction_path=tmp_path / "bad.md",
    )
    assert codes(render(bundle, selected)) == {"instruction-extension"}
    selected = replace(selected, instruction_path=tmp_path / "always.instructions.md")
    assert render(bundle, selected).artifacts[0].content == FileContent(
        b'---\napplyTo: "**"\n---\n\nAlways.'
    )


def test_selected_dependencies_cannot_disappear_by_audience(tmp_path: Path) -> None:
    native = NativeArtifact(
        "native",
        Family.MCP,
        Audience(Agent.CLAUDE, TargetScope.PROJECT),
        "native.json",
        FileContent(b"{}"),
    )
    bundle = Bundle.from_artifacts(
        BundleIdentity("dependency", "one"),
        [InstructionArtifact("guide", "Use it."), native],
        dependencies=[Dependency("guide", "native")],
    )
    assert codes(render(bundle, context(tmp_path, "codex"))) == {"dependency-audience"}
    native_only = Bundle.from_artifacts(
        BundleIdentity("native-only", "one"),
        [native],
        assets=[
            SupportAsset(
                "support", Family.MCP, TreeContent([BundleEntry("reference.txt", b"Info")])
            )
        ],
        dependencies=[Dependency("native", "support")],
    )
    assert render(native_only, context(tmp_path, "codex")).artifacts == ()
    assert {item.id for item in render(native_only, context(tmp_path, "claude")).artifacts} == {
        "native",
        "support",
    }


def asset_bundle(data: bytes = b'console.log("MCP");') -> Bundle:
    manifest = json.loads((FIXTURES / "translations.json").read_text(encoding="utf-8"))[
        "asset_manifest"
    ]
    return Bundle.from_memory(
        manifest, [BundleEntry("runtime/server.js", data), BundleEntry("runtime/data")]
    )


@pytest.mark.parametrize("agent", ["codex", "copilot"])
def test_asset_manifest_snapshot_reference_and_revision_lifecycle(
    tmp_path: Path, agent: str
) -> None:
    bundle = asset_bundle()
    tools = McpArtifact(
        "tools",
        "tools",
        Command(["node", AssetRef("runtime", "server.js")], cwd=AssetRef("runtime", "data")),
    )
    asset = SupportAsset(
        "runtime",
        Family.MCP,
        TreeContent([BundleEntry("server.js", b'console.log("MCP");'), BundleEntry("data")]),
    )
    assert bundle == Bundle.from_artifacts(
        bundle.identity, [tools], assets=[asset], dependencies=[Dependency("tools", "runtime")]
    )
    selected = replace(context(tmp_path, agent), asset_root=tmp_path / "assets")
    first = render(bundle, selected)
    runtime = next(item for item in first.artifacts if item.id == "runtime")
    assert runtime.destination.parent == tmp_path / "assets/support-demo/runtime"
    assert len(runtime.destination.name) == 64
    installation = selected.installation(tmp_path / "index")
    plans = preview(bundle, first, installation)
    assert plans.resources[0].resource.destination == runtime.destination
    assert sync(bundle, first, installation).status is OperationStatus.APPLIED
    assert (runtime.destination / "server.js").read_bytes() == b'console.log("MCP");'
    if agent == "copilot":
        record = json.loads((tmp_path / ".mcp.json").read_text())["mcpServers"]["tools"]
        assert record == {
            "command": "node",
            "args": [str(runtime.destination / "server.js")],
            "env": {},
            "cwd": str(runtime.destination / "data"),
        }
        assert Path(record["cwd"]).is_dir()
    updated = asset_bundle(b'console.log("new");')
    next_render = render(updated, selected)
    next_runtime = next(item for item in next_render.artifacts if item.id == "runtime")
    assert next_runtime.destination != runtime.destination
    assert next_render.content_digest != first.content_digest
    assert updated.version == bundle.version
    assert sync(updated, next_render, installation).status is OperationStatus.APPLIED
    assert not runtime.destination.exists()
    assert inspect_installation(bundle.id, installation).render_digest == next_render.content_digest
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED
    assert not next_runtime.destination.exists()


def test_reference_failure_preserves_old_asset_revision(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from flyrail._resource_transaction import Journal
    from flyrail._resource_transaction import publish as original_publish

    bundle = asset_bundle()
    selected = context(tmp_path)
    installation = selected.installation(tmp_path / "index")
    first = render(bundle, selected)
    assert sync(bundle, first, installation).status is OperationStatus.APPLIED

    def fail_configuration(journal: Journal) -> None:
        if journal.resource.destination.name == "config.toml":
            raise OSError("synthetic publication failure")
        original_publish(journal)

    monkeypatch.setattr("flyrail._lifecycle.publish", fail_configuration)
    updated = asset_bundle(b"next")
    next_render = render(updated, selected)
    result = sync(updated, next_render, installation)
    assert result.status is OperationStatus.PARTIAL
    assert all(
        item.destination.exists()
        for item in (*first.artifacts, *next_render.artifacts)
        if item.id == "runtime"
    )
    monkeypatch.setattr("flyrail._lifecycle.publish", original_publish)
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED
    assert not (tmp_path / ".codex/config.toml").exists()
    assert all(
        not item.destination.exists()
        for item in (*first.artifacts, *next_render.artifacts)
        if item.id == "runtime"
    )


@pytest.mark.parametrize(
    ("reference", "is_cwd", "edge", "message"),
    [
        (AssetRef("missing"), False, True, "unknown"),
        (AssetRef("runtime", "missing"), False, True, "does not exist"),
        (AssetRef("runtime", "server.js"), True, True, "directory"),
        (AssetRef("runtime", "server.js"), False, False, "direct dependency"),
        (AssetRef("runtime"), False, True, "file"),
    ],
)
def test_asset_reference_validation(
    reference: AssetRef, is_cwd: bool, edge: bool, message: str
) -> None:
    command = Command(["node"], cwd=reference) if is_cwd else Command([reference])
    asset = SupportAsset("runtime", Family.MCP, TreeContent([BundleEntry("server.js", b"code")]))
    with pytest.raises(ValueError, match=message):
        Bundle.from_artifacts(
            BundleIdentity("bad", "one"),
            [McpArtifact("tools", "tools", command)],
            assets=[asset],
            dependencies=[Dependency("tools", "runtime")] if edge else [],
        )


def test_asset_root_and_command_file_references(tmp_path: Path) -> None:
    runtime = SupportAsset(
        "runtime", Family.MCP, TreeContent([BundleEntry("server", b"executable", True)])
    )
    command = Command([AssetRef("runtime", "server"), AssetRef("runtime")], cwd=AssetRef("runtime"))
    bundle = Bundle.from_artifacts(
        BundleIdentity("runtime", "one"),
        [McpArtifact("tools", "tools", command)],
        assets=[runtime],
        dependencies=[Dependency("tools", "runtime")],
    )
    assert render(bundle, context(tmp_path)).supported
    with pytest.raises(ValueError):
        AssetRef("runtime", "../outside")
    manifest = json.loads((FIXTURES / "translations.json").read_text(encoding="utf-8"))[
        "asset_manifest"
    ]
    manifest["artifacts"][0]["transport"]["command"]["argv"][1]["extra"] = True
    with pytest.raises(ValueError):
        Bundle.from_memory(
            manifest, [BundleEntry("runtime/server.js", b"code"), BundleEntry("runtime/data")]
        )


def test_render_is_source_and_environment_independent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = asset_bundle()
    selected = context(tmp_path, "opencode")
    expected = render(bundle, selected)

    def forbidden(*args: object, **kwargs: object) -> Any:
        raise AssertionError("renderer observed external context")

    for method in ["read_bytes", "read_text", "stat", "resolve", "cwd", "home"]:
        monkeypatch.setattr(Path, method, forbidden)
    monkeypatch.setattr("os.getenv", forbidden)
    assert render(bundle, selected) == expected


def test_public_render_input_guards_and_context_immutability(tmp_path: Path) -> None:
    selected = context(tmp_path)
    with pytest.raises(FrozenInstanceError):
        selected.platform = Platform.WINDOWS  # type: ignore[misc]
    with pytest.raises(ValueError, match="agent Target"):
        RenderContext(Target.directory(tmp_path), Platform.LINUX)
    with pytest.raises(TypeError, match="Platform"):
        RenderContext(Target.project("codex", tmp_path), "linux")  # type: ignore[arg-type]
    for path in [Path("relative"), tmp_path / "..", Path(str(tmp_path) + "/nul\0")]:
        with pytest.raises(ValueError):
            replace(selected, mcp_path=path)
    with pytest.raises(TypeError):
        render("bundle", selected)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        capabilities("context")  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        render_many(bundle_of(InstructionArtifact("guide", "text")), [])
    with pytest.raises(TypeError):
        RenderedBundle([], routing_context=[("field", 1)])  # type: ignore[list-item]
    with pytest.raises(ValueError):
        RenderedBundle([], routing_context=[("field", "one"), ("field", "two")])
    with pytest.raises(TypeError):
        Notice("guide", NoticeKind.ACTIVATION, "message", False)  # type: ignore[arg-type]


def test_rendered_drift_and_explicit_instruction_notices(tmp_path: Path) -> None:
    bundle = bundle_of(InstructionArtifact("guide", "Useful."))
    selected = context(tmp_path)
    installation = selected.installation(tmp_path / "index")
    first = render(bundle, selected)
    assert "codex-instruction-discovery" in {notice.code for notice in first.notices}
    assert sync(bundle, first, installation).status is OperationStatus.APPLIED
    second = render(bundle, replace(selected, instruction_path=tmp_path / "AGENTS.override.md"))
    assert first.content_digest != second.content_digest
    assert sync(bundle, second, installation).status is OperationStatus.APPLIED
    assert not (tmp_path / "AGENTS.md").exists()
    assert (tmp_path / "AGENTS.override.md").read_text().count("Useful.") == 1
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED


def test_reserved_mcp_name_and_cwd_interpolation(tmp_path: Path) -> None:
    assert codes(
        render(
            bundle_of(McpArtifact("tools", "workspace", Command(["server"]))),
            context(tmp_path, "claude"),
        )
    ) == {"reserved-server-name"}
    selected = replace(context(tmp_path, "opencode"), execution_root=tmp_path / "{env:ROOT}")
    assert codes(
        render(bundle_of(McpArtifact("tools", "tools", Command(["server"], cwd="work"))), selected)
    ) == {"literal-interpolation"}


def test_unsupported_preview_and_pending_target_never_mutate(tmp_path: Path) -> None:
    from flyrail import apply_preview

    selected = context(tmp_path, "copilot")
    bundle = bundle_of(
        McpArtifact("tools", "tools", HttpTransport("https://example.test", EnvRef("TOKEN")))
    )
    installation = selected.installation(tmp_path / "index")
    rendered = render(bundle, selected)
    blocked = preview(bundle, rendered, installation)
    assert not blocked.applicable
    assert apply_preview(blocked).error is not None
    assert list(tmp_path.iterdir()) == []
    valid = bundle_of(InstructionArtifact("guide", "Retain this."))
    assert sync(valid, render(valid, selected), installation).status is OperationStatus.APPLIED
    pending = installation.index_root / "index.json.next"
    pending.write_bytes((installation.index_root / "index.json").read_bytes())
    before = {path: path.read_bytes() for path in tmp_path.rglob("*") if path.is_file()}
    result = sync(bundle, rendered, installation)
    assert result.error is not None and result.error.code == "unsupported"
    assert result.notices == rendered.notices
    assert {path: path.read_bytes() for path in tmp_path.rglob("*") if path.is_file()} == before


def test_claude_relocated_mcp_requires_explicit_document(tmp_path: Path) -> None:
    selected = RenderContext(
        Target.user("claude", home=tmp_path, env={"CLAUDE_CONFIG_DIR": "~/selected"}),
        Platform.LINUX,
    )
    bundle = bundle_of(McpArtifact("tools", "tools", Command(["server"])))
    assert codes(render(bundle, selected)) == {"mcp-explicit-path"}
    assert render(bundle, replace(selected, mcp_path=tmp_path / "chosen.json")).supported
    vscode = replace(
        context(tmp_path, "copilot", surface="vscode"), mcp_path=tmp_path / ".mcp.json"
    )
    assert codes(render(bundle, vscode)) == {"mcp-profile-schema"}


@pytest.mark.parametrize("literal", ["${TOKEN}", "$env:TOKEN", "{env:TOKEN}"])
def test_pi_all_interpolation_syntaxes_are_checked(tmp_path: Path, literal: str) -> None:
    bundle = bundle_of(McpArtifact("tools", "tools", Command(["server", literal])))
    assert codes(render(bundle, context(tmp_path, "pi"))) == {"literal-interpolation"}


def test_capability_summaries_snapshot_details(tmp_path: Path) -> None:
    from flyrail import Capability

    details = ["External prerequisite"]
    result = Capability(Family.MCP, True, True, prerequisites=details)  # type: ignore[arg-type]
    details.append("later")
    assert result.prerequisites == ("External prerequisite",)
    with pytest.raises(TypeError):
        Capability("mcp", True, True)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        Capability(Family.MCP, True, True, limitations="text")  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        Capability(Family.MCP, True, True, prerequisites=(" ",))
    pi = capabilities(context(tmp_path, "pi"))
    assert next(item for item in pi if item.family is Family.MCP).prerequisites == (
        "pi-mcp-adapter@2.32.1",
    )
