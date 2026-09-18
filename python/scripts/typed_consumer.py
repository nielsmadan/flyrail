import json
import sys
import tomllib
from pathlib import Path
from typing import assert_type

import flyrail
from flyrail import (
    Acquisition,
    AssetRef,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    Dependency,
    DocumentEdit,
    DocumentFormat,
    DocumentRequest,
    EnvRef,
    Family,
    Installation,
    InstallationObservation,
    InstallationResult,
    InstallationTarget,
    InstructionArtifact,
    InventoryEntry,
    Key,
    LifecyclePreview,
    McpArtifact,
    Observation,
    OperationStatus,
    Platform,
    RenderContext,
    RenderedArtifact,
    RenderedBundle,
    Scalar,
    SectionContent,
    SelectionSnapshot,
    Selector,
    SkillSpec,
    StructuredContent,
    SupportAsset,
    Target,
    TargetInspection,
    TargetResult,
    TreeContent,
    apply_preview,
    edit_document,
    freeze_value,
    inspect,
    inspect_installation,
    install,
    preview,
    preview_removal,
    remove,
    render,
    render_many,
    sync,
    uninstall,
    update,
)


def consume_documents() -> None:
    original = b"# Project\r\n"
    section = edit_document(original, [DocumentRequest.set(SectionContent("guide", "Check it."))])
    assert_type(section, DocumentEdit)
    assert_type(section.after, bytes | None)
    removed = edit_document(
        section.after,
        [DocumentRequest.retire(section.ownership[0].claim)],
        ownership=section.ownership,
        provenance=section.provenance,
    )
    assert removed.after == original
    snapshot = SelectionSnapshot(
        freeze_value({"env": {}}),
        b"",
        b"",
        layout=b'{"version":1,"headers":[[["env"],"mcp.example.env"]]}',
    )
    assert snapshot.layout
    assert "tomlkit" not in sys.modules
    for format, document in (
        (DocumentFormat.JSONC, b'{/* foreign */"mcp":{}}'),
        (DocumentFormat.TOML, b"# foreign\r\n[mcp]\r\n"),
    ):
        content = StructuredContent(format, Selector([Key("mcp"), Key("example")]), Scalar(True))
        installed = edit_document(document, [DocumentRequest.set(content)])
        assert installed.applicable
        restored = edit_document(
            installed.after,
            [DocumentRequest.retire(installed.ownership[0].claim)],
            ownership=installed.ownership,
            provenance=installed.provenance,
        )
        assert restored.after == document
    import tomlkit

    assert Path(tomlkit.__file__).is_relative_to(Path(sys.prefix))
    original_mcp = (
        b'[mcp_servers.example.env] # nested\nX="before"\n'
        b'[mcp_servers.example] # user\ncommand="old"\n'
    )
    selector = Selector([Key("mcp_servers"), Key("example")])
    registered = edit_document(
        original_mcp,
        [
            DocumentRequest.set(
                StructuredContent(
                    DocumentFormat.TOML,
                    selector,
                    freeze_value({"command": "cmd", "args": ["serve"], "env": {"X": "after"}}),
                ),
                acquisition=Acquisition.TAKEOVER,
            )
        ],
    )
    assert registered.applicable and registered.after is not None
    assert tomllib.loads(registered.after.decode())["mcp_servers"]["example"] == {
        "command": "cmd",
        "args": ["serve"],
        "env": {"X": "after"},
    }
    updated = edit_document(
        registered.after,
        [
            DocumentRequest.set(
                StructuredContent(
                    DocumentFormat.TOML,
                    selector,
                    freeze_value({"command": "new", "env": {"Y": "next"}}),
                )
            )
        ],
        ownership=registered.ownership,
        provenance=registered.provenance,
    )
    assert updated.applicable and updated.after is not None
    assert tomllib.loads(updated.after.decode())["mcp_servers"]["example"] == {
        "command": "new",
        "env": {"Y": "next"},
    }
    restored = edit_document(
        updated.after,
        [DocumentRequest.retire(updated.ownership[0].claim)],
        ownership=updated.ownership,
        provenance=updated.provenance,
    )
    assert restored.applicable and restored.after == original_mcp


def consume_lifecycle(bundle: Bundle) -> None:
    path = Path.cwd() / "generated-project" / "AGENTS.md"
    path.parent.mkdir()
    original = b"# Private project instructions\r\n"
    path.write_bytes(original)
    target = InstallationTarget(
        "generated-index",
        [("consumer", "typed-example")],
        routing_context=[("surface", "generated-instructions")],
    )
    assert target.routing_context == (("surface", "generated-instructions"),)
    desired = RenderedBundle(
        [
            RenderedArtifact(
                "generated",
                Family.INSTRUCTIONS,
                path,
                SectionContent("application", "Run the checks."),
                require_existing=True,
            )
        ]
    )
    planned = preview(bundle, desired, target)
    assert_type(planned, LifecyclePreview)
    assert planned.applicable and path.read_bytes() == original
    result = apply_preview(planned)
    assert_type(result, InstallationResult)
    assert result.status is OperationStatus.APPLIED
    observed = inspect_installation(bundle.id, target)
    assert_type(observed, InstallationObservation)
    assert observed.is_current
    assert_type(observed.matches(bundle, desired), bool)
    assert observed.matches(bundle, desired)
    assert sync(bundle, desired, target).status is OperationStatus.UNCHANGED
    assert preview_removal(bundle.id, target).applicable
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert path.read_bytes() == original


def consume_translation() -> None:
    root = Path.cwd() / "translated-project"
    bundle = Bundle.from_artifacts(
        BundleIdentity("typed-translation", "same-label"),
        [
            InstructionArtifact("guide", "Run the project checks.\n"),
            McpArtifact("tools", "tools", Command(["node", AssetRef("runtime", "server.js")])),
        ],
        assets=[
            SupportAsset(
                "runtime", Family.MCP, TreeContent([BundleEntry("server.js", b"void 0;\n")])
            )
        ],
        dependencies=[Dependency("tools", "runtime")],
    )
    context = RenderContext(
        Target.project("codex", root),
        Platform.WINDOWS if sys.platform == "win32" else Platform.LINUX,
    )
    desired = render(bundle, context)
    assert_type(desired, RenderedBundle)
    assert desired.supported
    target = context.installation(Path.cwd() / "translated-index")
    assert_type(target, InstallationTarget)
    result = sync(bundle, desired, target)
    assert_type(result, InstallationResult)
    assert result.status is OperationStatus.APPLIED
    record = tomllib.loads((root / ".codex/config.toml").read_text())
    assert record["mcp_servers"]["tools"]["command"] == "node"
    assert Path(record["mcp_servers"]["tools"]["args"][0]).read_bytes() == b"void 0;\n"
    assert inspect_installation(bundle.id, target).is_current
    assert sync(bundle, desired, target).status is OperationStatus.UNCHANGED
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert not (root / "AGENTS.md").exists()


def consume_copilot_translation() -> None:
    root = Path.cwd() / "copilot-project"
    target = InstallationTarget(Path.cwd() / "copilot-index")
    platform = Platform.WINDOWS if sys.platform == "win32" else Platform.LINUX
    contexts = [
        RenderContext(Target.project(agent, root), platform) for agent in ["claude", "copilot"]
    ]
    bundle = Bundle.from_artifacts(
        BundleIdentity("typed-copilot", "same-label"),
        [McpArtifact("tools", "tools", Command(["server"], {"TOKEN": EnvRef("SOURCE_TOKEN")}))],
    )
    shared = render_many(bundle, contexts)
    assert_type(shared, RenderedBundle)
    assert shared.supported
    assert sync(bundle, shared, target).status is OperationStatus.APPLIED
    assert json.loads((root / ".mcp.json").read_text()) == {
        "mcpServers": {
            "tools": {"command": "server", "args": [], "env": {"TOKEN": "${SOURCE_TOKEN}"}}
        }
    }
    observed = inspect_installation(bundle.id, target)
    assert observed.is_current
    assert len(observed.resources) == 1 and len(observed.resources[0].claims) == 1
    invalid = Bundle.from_artifacts(
        bundle.identity,
        [McpArtifact("tools", "tools", Command(["server"], {"MODE": "$FLYRAIL_EXAMPLE"}))],
    )
    blocked = render(invalid, contexts[1])
    assert not blocked.supported
    before = {path: path.read_bytes() for path in Path.cwd().rglob("*") if path.is_file()}
    result = sync(invalid, blocked, target)
    assert result.error is not None and result.error.code == "unsupported"
    assert {path: path.read_bytes() for path in Path.cwd().rglob("*") if path.is_file()} == before
    assert remove(bundle.id, target).status is OperationStatus.APPLIED


def consume(bundle: Bundle, target: Target) -> None:
    assert_type(bundle.identity, BundleIdentity)
    assert_type(bundle.entries, tuple[BundleEntry, ...])
    assert_type(bundle.skills, tuple[SkillSpec, ...])
    observations = inspect(bundle, [target])
    assert_type(observations, tuple[TargetInspection, ...])
    assert_type(observations[0].observation, Observation)
    assert_type(observations[0].observation.installed, Installation | None)
    installed = observations[0].observation.installed
    if installed is not None:
        assert_type(installed.entries, tuple[InventoryEntry, ...])
    results = install(bundle, [target])
    assert_type(results, tuple[TargetResult, ...])
    assert_type(results[0].status, OperationStatus)
    assert results[0].status is OperationStatus.APPLIED
    assert_type(update(bundle, [target], replace_modified=False), tuple[TargetResult, ...])
    assert_type(uninstall(bundle.id, [target]), tuple[TargetResult, ...])
    assert uninstall(bundle.id, [target])[0].status is OperationStatus.UNCHANGED


if __name__ == "__main__":
    assert Path(flyrail.__file__).is_relative_to(Path(sys.prefix))
    assert Path(flyrail.__file__).with_name("py.typed").is_file()
    consume_documents()
    snapshot = Bundle.from_directory("notes-bundle")
    consume(snapshot, Target.directory("typed-skills"))
    consume_lifecycle(snapshot)
    consume_translation()
    consume_copilot_translation()
    print("Built-wheel typed consumer passed.")
