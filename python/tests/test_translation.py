import json
import tomllib
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from conformance import FIXTURES

from flyrail import (
    Agent,
    Audience,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    DocumentFormat,
    EnvRef,
    Family,
    FileContent,
    HttpTransport,
    InstallationTarget,
    InstructionArtifact,
    Key,
    McpArtifact,
    NativeArtifact,
    NoticeKind,
    ObjectValue,
    OperationStatus,
    Platform,
    RenderContext,
    RenderedBundle,
    SectionContent,
    Selector,
    StructuredContent,
    Surface,
    Target,
    TargetScope,
    apply_preview,
    freeze_value,
    inspect_installation,
    preview,
    remove,
    render,
    render_many,
    render_skills,
    sync,
)

VECTORS = json.loads((FIXTURES / "translations.json").read_text(encoding="utf-8"))


def context(root: Path, agent: str = "codex", *, surface: str = "cli") -> RenderContext:
    return RenderContext(Target.project(agent, root), Platform.LINUX, Surface(surface))


def bundle_of(*artifacts: Any) -> Bundle:
    return Bundle.from_artifacts(BundleIdentity("translation-demo", "unchanged-label"), artifacts)


def codes(rendered: RenderedBundle) -> set[str]:
    return {notice.code for notice in rendered.notices if notice.kind is NoticeKind.UNSUPPORTED}


def assert_unsupported_without_writes(
    root: Path, bundle: Bundle, rendered: RenderedBundle, target: InstallationTarget
) -> None:
    before = {
        path.relative_to(root): path.read_bytes() if path.is_file() else None
        for path in root.rglob("*")
    }
    assert not rendered.supported
    assert rendered.artifacts == ()
    assert rendered.dependencies == ()
    planned = preview(bundle, rendered, target)
    assert not planned.applicable
    for result in [apply_preview(planned), sync(bundle, rendered, target)]:
        assert result.error is not None and result.error.code == "unsupported"
    assert {
        path.relative_to(root): path.read_bytes() if path.is_file() else None
        for path in root.rglob("*")
    } == before


@pytest.mark.parametrize("case", VECTORS["combined_cases"], ids=lambda case: case["name"])
def test_neutral_combined_translation_vectors(tmp_path: Path, case: dict[str, Any]) -> None:
    bundle = Bundle.from_memory(
        {"schema_version": 2, "id": "combined", "version": "same", "artifacts": case["artifacts"]}
    )
    contexts = [context(tmp_path, item["agent"]) for item in case["contexts"]]
    rendered = render_many(bundle, contexts)
    assert rendered == render_many(bundle, reversed(contexts))
    target = InstallationTarget(tmp_path / "index")
    expected = case["expected"]
    if "unsupported" in expected:
        assert codes(rendered) == {expected["unsupported"]}
        assert_unsupported_without_writes(tmp_path, bundle, rendered, target)
        assert list(tmp_path.iterdir()) == []
        if len(contexts) == 1:
            direct = render(bundle, contexts[0])
            assert codes(direct) == codes(rendered)
            assert_unsupported_without_writes(tmp_path, bundle, direct, target)
        return
    assert rendered.supported
    assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
    for destination, document in expected["documents"].items():
        assert json.loads((tmp_path / destination).read_text()) == document
    observed = inspect_installation(bundle.id, target)
    assert observed.is_current
    assert sum(len(resource.claims) for resource in observed.resources) == expected["claims"]
    assert remove(bundle.id, target).status is OperationStatus.APPLIED


def vector_context(
    tmp_path: Path, case: dict[str, Any], platform: Platform
) -> tuple[RenderContext, Bundle, RenderedBundle]:
    target = (
        Target.project(case["agent"], tmp_path)
        if case["scope"] == "project"
        else Target.user(case["agent"], home=tmp_path, env={})
    )
    selected = RenderContext(
        target,
        platform,
        Surface(case["surface"]),
        execution_root=tmp_path / case["execution_root"] if "execution_root" in case else None,
        mcp_path=tmp_path / case["mcp_path"] if "mcp_path" in case else None,
    )
    bundle = Bundle.from_memory(
        {
            "schema_version": 2,
            "id": "translation-demo",
            "version": "same",
            "artifacts": [case["artifact"]],
        }
    )
    return selected, bundle, render(bundle, selected)


def assert_rendered_vector(
    tmp_path: Path, case: dict[str, Any], rendered: RenderedBundle
) -> tuple[Any, dict[str, Any] | None] | None:
    expected = case["expected"]
    if "unsupported" in expected:
        assert codes(rendered) == {expected["unsupported"]}
        return None
    assert rendered.supported
    (artifact,) = rendered.artifacts
    if "directory" in expected:
        assert artifact.destination.parent == tmp_path / expected["directory"]
        assert artifact.destination.name.endswith(expected["suffix"])
    else:
        assert artifact.destination == tmp_path / expected["destination"]
    expected_value = expected.get("value")
    if "cwd_from_root" in expected:
        expected_value = {**expected_value, "cwd": str(tmp_path / expected["cwd_from_root"])}
    if isinstance(artifact.content, StructuredContent):
        assert artifact.content.format == DocumentFormat(expected["format"])
        assert artifact.content.value == freeze_value(expected_value)
        assert [part.name for part in artifact.content.selector.parts] == [expected["key"], "tools"]  # type: ignore[union-attr]
    elif isinstance(artifact.content, SectionContent):
        assert artifact.content.text == expected["text"]
    else:
        assert isinstance(artifact.content, FileContent)
        assert artifact.content.data.decode() == expected["text"]
    return artifact, expected_value


@pytest.mark.parametrize("platform", list(Platform))
@pytest.mark.parametrize("case", VECTORS["cases"], ids=lambda case: case["name"])
def test_neutral_translation_vectors(
    tmp_path: Path, case: dict[str, Any], platform: Platform
) -> None:
    _selected, _bundle, rendered = vector_context(tmp_path, case, platform)
    assert_rendered_vector(tmp_path, case, rendered)


@pytest.mark.integration
@pytest.mark.parametrize("platform", list(Platform))
@pytest.mark.parametrize("case", VECTORS["cases"], ids=lambda case: case["name"])
def test_neutral_translation_and_lifecycle_vectors(
    tmp_path: Path, case: dict[str, Any], platform: Platform
) -> None:
    selected, bundle, rendered = vector_context(tmp_path, case, platform)
    installation = selected.installation(tmp_path / "private-index")
    expected = case["expected"]
    if "unsupported" in expected:
        assert codes(rendered) == {expected["unsupported"]}
        assert_unsupported_without_writes(tmp_path, bundle, rendered, installation)
        return
    assert rendered.supported
    (artifact,) = rendered.artifacts
    if "directory" in expected:
        assert artifact.destination.parent == tmp_path / expected["directory"]
        assert artifact.destination.name.endswith(expected["suffix"])
    else:
        assert artifact.destination == tmp_path / expected["destination"]
    original: bytes | None = None
    expected_value = expected.get("value")
    if "cwd_from_root" in expected:
        expected_value = {**expected_value, "cwd": str(tmp_path / expected["cwd_from_root"])}
    if isinstance(artifact.content, StructuredContent):
        assert artifact.content.format == DocumentFormat(expected["format"])
        assert artifact.content.value == freeze_value(expected_value)
        assert [part.name for part in artifact.content.selector.parts] == [expected["key"], "tools"]  # type: ignore[union-attr]
        original = (
            b"# foreign\n[other]\nactive = true\n"
            if artifact.content.format is DocumentFormat.TOML
            else b'{"foreign": true}\n'
        )
    elif isinstance(artifact.content, SectionContent):
        assert artifact.content.text == expected["text"]
        original = b"# User instructions\r\n"
    else:
        assert isinstance(artifact.content, FileContent)
        assert artifact.content.data.decode() == expected["text"]
    if original is not None:
        artifact.destination.parent.mkdir(parents=True, exist_ok=True)
        artifact.destination.write_bytes(original)
    planned = preview(bundle, rendered, installation)
    assert planned.applicable
    assert planned.rendered.routing_context == selected.routing_context
    installed = sync(bundle, rendered, installation)
    assert installed.status is OperationStatus.APPLIED
    assert inspect_installation(bundle.id, installation).is_current
    if isinstance(artifact.content, StructuredContent):
        raw = artifact.destination.read_text()
        parsed = (
            tomllib.loads(raw)
            if artifact.content.format is DocumentFormat.TOML
            else json.loads(raw)
        )
        assert parsed[expected["key"]]["tools"] == expected_value
        assert parsed["other" if artifact.content.format is DocumentFormat.TOML else "foreign"] == (
            {"active": True} if artifact.content.format is DocumentFormat.TOML else True
        )
    assert sync(bundle, rendered, installation).status is OperationStatus.UNCHANGED
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED
    if original is None:
        assert not artifact.destination.exists()
    else:
        assert artifact.destination.read_bytes() == original


@pytest.mark.parametrize("agent", list(Agent))
@pytest.mark.parametrize("scope", ["project", "user"])
def test_skills_reuse_the_existing_presets(tmp_path: Path, agent: Agent, scope: str) -> None:
    bundle = Bundle.from_memory(
        {
            "schema_version": 1,
            "id": "skills",
            "version": "one",
            "skills": [{"name": "review", "path": "review"}],
        },
        [
            BundleEntry("review/SKILL.md", b"---\nname: review\n---\nCheck it.\n"),
            BundleEntry("review/empty"),
        ],
    )
    target = (
        Target.project(agent, tmp_path) if scope == "project" else Target.user(agent, home=tmp_path)
    )
    rendered = render(bundle, RenderContext(target, Platform.LINUX))
    assert rendered.artifacts == render_skills(bundle, target).artifacts
    installation = InstallationTarget(tmp_path / "index")
    assert sync(bundle, rendered, installation).status is OperationStatus.APPLIED
    assert (target.root / "review/SKILL.md").read_bytes() == b"---\nname: review\n---\nCheck it.\n"
    assert (target.root / "review/empty").is_dir()
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED


@pytest.mark.parametrize(
    ("agent", "surface", "token"),
    [
        ("claude", "cli", "${SECRET}"),
        ("opencode", "cli", "{env:SECRET}"),
        ("cursor", "cli", "${env:SECRET}"),
        ("copilot", "cli", "${SECRET}"),
        ("copilot", "vscode", "${env:SECRET}"),
    ],
)
def test_environment_references_are_preserved(
    tmp_path: Path, agent: str, surface: str, token: str
) -> None:
    selected = context(tmp_path, agent, surface=surface)
    bundle = bundle_of(
        McpArtifact(
            "tools", "tools", Command(["server"], [("TOKEN", EnvRef("SECRET")), ("MODE", "local")])
        )
    )
    (item,) = render(bundle, selected).artifacts
    assert isinstance(item.content, StructuredContent)
    assert isinstance(item.content.value, ObjectValue)
    value = dict(item.content.value.items)
    assert value["environment" if agent == "opencode" else "env"] == freeze_value(
        {"TOKEN": token, "MODE": "local"}
    )


def test_codex_forwarding_and_alias_boundary(tmp_path: Path) -> None:
    selected = context(tmp_path)
    rendered = render(
        bundle_of(
            McpArtifact(
                "tools",
                "tools",
                Command(
                    ["server", "${literal}"], [("TOKEN", EnvRef("TOKEN")), ("MODE", "${literal}")]
                ),
            )
        ),
        selected,
    )
    assert rendered.supported
    (item,) = rendered.artifacts
    assert isinstance(item.content, StructuredContent)
    assert item.content.value == freeze_value(
        {
            "command": "server",
            "args": ["${literal}"],
            "env": {"MODE": "${literal}"},
            "env_vars": ["TOKEN"],
        }
    )
    invalid = bundle_of(
        McpArtifact("tools", "tools", Command(["server"], {"ALIAS": EnvRef("TOKEN")}))
    )
    assert codes(render(invalid, selected)) == {"environment-alias"}


@pytest.mark.parametrize(
    ("agent", "surface", "literal"),
    [
        ("claude", "cli", "${TOKEN}"),
        ("cursor", "ide", "${workspaceFolder}"),
        ("opencode", "cli", "{file:secret}"),
        ("copilot", "cli", "${TOKEN}"),
        ("copilot", "vscode", "${input:secret}"),
        ("pi", "cli", "${TOKEN}"),
    ],
)
@pytest.mark.parametrize("field", ["argv", "env", "url"])
def test_literal_interpolation_hazards_are_exact_blockers(
    tmp_path: Path, agent: str, surface: str, literal: str, field: str
) -> None:
    transport = (
        HttpTransport("https://tools.example.test/" + literal)
        if field == "url"
        else Command(
            ["server", literal] if field == "argv" else ["server"],
            {"MODE": literal} if field == "env" else {},
        )
    )
    rendered = render(
        bundle_of(McpArtifact("tools", "tools", transport)),
        context(tmp_path, agent, surface=surface),
    )
    expected = (
        set()
        if agent == "pi" and field == "env"
        else {"literal-interpolation-unverified"}
        if agent == "copilot" and surface == "cli" and field != "env"
        else {"literal-interpolation"}
    )
    assert codes(rendered) == expected


@pytest.mark.parametrize(
    ("agent", "surface"),
    [
        ("codex", "cli"),
        ("opencode", "cli"),
        ("pi", "cli"),
        ("copilot", "cli"),
        ("copilot", "vscode"),
    ],
)
def test_cwd_has_an_explicit_execution_anchor(tmp_path: Path, agent: str, surface: str) -> None:
    selected = replace(
        context(tmp_path, agent, surface=surface), execution_root=tmp_path / "execution root"
    )
    rendered = render(
        bundle_of(McpArtifact("tools", "tools", Command(["server"], cwd="nested/work"))), selected
    )
    (item,) = rendered.artifacts
    assert isinstance(item.content, StructuredContent)
    assert isinstance(item.content.value, ObjectValue)
    assert dict(item.content.value.items)["cwd"] == freeze_value(
        str(tmp_path / "execution root/nested/work")
    )


@pytest.mark.parametrize("agent", ["claude", "cursor"])
def test_missing_cwd_field_does_not_drop_authored_cwd(tmp_path: Path, agent: str) -> None:
    rendered = render(
        bundle_of(McpArtifact("tools", "tools", Command(["server"], cwd="work"))),
        context(tmp_path, agent),
    )
    assert codes(rendered) == {"command-cwd"}


def test_pi_adapter_literal_environment_and_reference_boundary(tmp_path: Path) -> None:
    selected = context(tmp_path, "pi")
    literal = bundle_of(
        McpArtifact(
            "tools",
            "tools",
            Command(
                ["!literal-command", "argument"],
                {"A": "!do-not-run", "B": "${literal}", "C": "{env:LITERAL}"},
            ),
        )
    )
    rendered = render(literal, selected)
    assert rendered.supported
    (item,) = rendered.artifacts
    assert isinstance(item.content, StructuredContent)
    assert item.content.value == freeze_value(
        {
            "command": "!literal-command",
            "args": ["argument"],
            "env": {"A": "!do-not-run", "B": "${literal}", "C": "{env:LITERAL}"},
            "literalEnv": True,
        }
    )
    assert any(notice.code == "pi-mcp-adapter-v2-32-1" for notice in rendered.notices)
    mixed = bundle_of(
        McpArtifact(
            "tools", "tools", Command(["server"], [("A", "!literal"), ("B", EnvRef("TOKEN"))])
        )
    )
    assert codes(render(mixed, selected)) == {"portable-env-reference-unverified"}


def test_windows_environment_collisions_and_unsupported_update_do_not_write(tmp_path: Path) -> None:
    selected = replace(context(tmp_path), platform=Platform.WINDOWS)
    initial = bundle_of(InstructionArtifact("guide", "Keep this.\n"))
    installation = selected.installation(tmp_path / "index")
    assert sync(initial, render(initial, selected), installation).status is OperationStatus.APPLIED
    before = {
        path.relative_to(tmp_path): path.read_bytes()
        for path in tmp_path.rglob("*")
        if path.is_file()
    }
    invalid = bundle_of(
        McpArtifact("tools", "tools", Command(["server"], {"Mode": "one", "MODE": "two"}))
    )
    rendered = render(invalid, selected)
    assert codes(rendered) == {"windows-env-collision"}
    result = sync(invalid, rendered, installation)
    assert result.error is not None and result.error.code == "unsupported"
    assert {
        path.relative_to(tmp_path): path.read_bytes()
        for path in tmp_path.rglob("*")
        if path.is_file()
    } == before
    assert render(invalid, replace(selected, platform=Platform.LINUX)).supported


def test_shared_blocks_deduplicate_through_the_lifecycle(tmp_path: Path) -> None:
    bundle = bundle_of(InstructionArtifact("guide", "Shared guidance.\n"))
    contexts = [context(tmp_path, name) for name in ["codex", "pi", "opencode", "cursor"]]
    combined = render_many(bundle, contexts)
    assert combined == render_many(bundle, reversed(contexts))
    assert len(combined.artifacts) == 4
    assert len({item.content for item in combined.artifacts}) == 1
    installation = InstallationTarget(tmp_path / "index")
    assert sync(bundle, combined, installation).status is OperationStatus.APPLIED
    assert (tmp_path / "AGENTS.md").read_text().count("Shared guidance.") == 1
    assert len(inspect_installation(bundle.id, installation).resources[0].claims) == 1
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED
    assert not (tmp_path / "AGENTS.md").exists()
    assert render_many(bundle, [contexts[0], contexts[0]]) == render_many(bundle, [contexts[0]])


@pytest.mark.parametrize("alternate", [False, True])
@pytest.mark.parametrize("environment", [{"MODE": "local"}, {"TOKEN": EnvRef("SOURCE_TOKEN")}])
def test_shared_mcp_records_preserve_the_effective_selection(
    tmp_path: Path, alternate: bool, environment: dict[str, str | EnvRef]
) -> None:
    bundle = bundle_of(McpArtifact("tools", "tools", Command(["server"], environment)))
    claude, copilot = context(tmp_path, "claude"), context(tmp_path, "copilot")
    if alternate:
        copilot = replace(copilot, mcp_path=tmp_path / ".github/mcp.json")
    combined = render_many(bundle, [claude, copilot])
    assert combined.supported
    assert combined == render_many(bundle, [copilot, claude])
    installation = InstallationTarget(tmp_path / "index")
    assert sync(bundle, combined, installation).status is OperationStatus.APPLIED
    effective = json.loads((tmp_path / ".mcp.json").read_text())["mcpServers"]["tools"]
    assert effective == {
        "command": "server",
        "args": [],
        "env": {"TOKEN": "${SOURCE_TOKEN}"} if "TOKEN" in environment else {"MODE": "local"},
    }
    if alternate:
        assert json.loads((tmp_path / ".github/mcp.json").read_text())["mcpServers"]["tools"] == (
            effective
        )
    observed = inspect_installation(bundle.id, installation)
    assert observed.is_current
    assert len(observed.resources) == (2 if alternate else 1)
    assert all(len(resource.claims) == 1 for resource in observed.resources)
    assert remove(bundle.id, installation).status is OperationStatus.APPLIED


@pytest.mark.parametrize("agent", ["codex", "copilot"])
def test_invalid_copilot_env_update_preserves_previous_installation(
    tmp_path: Path, agent: str
) -> None:
    selected = context(tmp_path, agent)
    original = bundle_of(InstructionArtifact("guide", "Keep this."))
    target = selected.installation(tmp_path / "index")
    assert sync(original, render(original, selected), target).status is OperationStatus.APPLIED
    invalid = bundle_of(
        InstructionArtifact("guide", "Changed."),
        McpArtifact("tools", "tools", Command(["server"], {"MODE": "$FLYRAIL_EXAMPLE"})),
    )
    rendered = render(invalid, context(tmp_path, "copilot"))
    assert codes(rendered) == {"literal-interpolation"}
    assert_unsupported_without_writes(tmp_path, invalid, rendered, target)
    assert inspect_installation(original.id, target).is_current


@pytest.mark.parametrize("format", [DocumentFormat.JSON, DocumentFormat.JSONC, DocumentFormat.TOML])
def test_disjoint_structured_claims_require_one_document_format(
    tmp_path: Path, format: DocumentFormat
) -> None:
    selected = context(tmp_path, "codex")
    original = bundle_of(McpArtifact("tools", "tools", Command(["server"])))
    target = selected.installation(tmp_path / "index")
    assert sync(original, render(original, selected), target).status is OperationStatus.APPLIED
    bundle = bundle_of(
        *original.artifacts,
        NativeArtifact(
            "native",
            Family.MCP,
            selected.audience,
            ".codex/config.toml",
            StructuredContent(format, Selector([Key("other")]), freeze_value({"enabled": True})),
        ),
    )
    rendered = render(bundle, selected)
    if format is DocumentFormat.TOML:
        assert rendered.supported
        assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
        assert tomllib.loads((tmp_path / ".codex/config.toml").read_text()) == {
            "mcp_servers": {"tools": {"command": "server", "args": [], "env": {}}},
            "other": {"enabled": True},
        }
    else:
        assert codes(rendered) == {"document-format-collision"}
        assert_unsupported_without_writes(tmp_path, bundle, rendered, target)
    assert inspect_installation(bundle.id, target).is_current


@pytest.mark.parametrize("destination", [".mcp.json", ".github/mcp.json"])
def test_incompatible_native_records_block_selected_copilot_routes(
    tmp_path: Path, destination: str
) -> None:
    records = [
        NativeArtifact(
            agent,
            Family.MCP,
            Audience(Agent(agent), TargetScope.PROJECT),
            ".mcp.json" if agent == "claude" else destination,
            StructuredContent(
                DocumentFormat.JSON,
                Selector([Key("mcpServers"), Key("tools")]),
                freeze_value({"command": agent + "-tools", "args": []}),
            ),
        )
        for agent in ["claude", "copilot"]
    ]
    bundle = bundle_of(*records)
    contexts = [context(tmp_path, "claude"), context(tmp_path, "copilot")]
    rendered = render_many(bundle, contexts)
    assert rendered == render_many(bundle, reversed(contexts))
    expected = (
        "destination-collision"
        if destination == ".mcp.json"
        else "copilot-mcp-precedence-collision"
    )
    assert codes(rendered) == {expected}
    assert_unsupported_without_writes(
        tmp_path, bundle, rendered, InstallationTarget(tmp_path / "index")
    )


def test_copilot_opaque_alternate_reports_discovery_precedence(tmp_path: Path) -> None:
    selected = context(tmp_path, "copilot")
    bundle = bundle_of(
        McpArtifact("tools", "tools", Command(["server"])),
        NativeArtifact(
            "opaque",
            Family.MCP,
            selected.audience,
            ".github/mcp.json",
            FileContent(b'{"mcpServers": {"authored": {"command": "authored", "args": []}}}'),
        ),
    )
    rendered = render(bundle, selected)
    assert rendered.supported
    notice = next(
        item for item in rendered.notices if item.code == "copilot-mcp-precedence-unverified"
    )
    assert notice.kind is NoticeKind.ACTIVATION
    assert str(tmp_path / ".mcp.json") in notice.message
    assert str(tmp_path / ".github/mcp.json") in notice.message
