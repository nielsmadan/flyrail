import datetime as dt
import hashlib
import importlib
import json
import shutil
import sys
import zipfile
from dataclasses import FrozenInstanceError, replace
from pathlib import Path
from typing import Any, TypedDict, cast

import pytest
from conformance import FIXTURES
from skill_helpers import snapshot

from flyrail import (
    Agent,
    Audience,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    Dependency,
    DocumentFormat,
    Family,
    InstallationTarget,
    InstructionArtifact,
    Key,
    McpArtifact,
    NativeArtifact,
    ObjectValue,
    OperationStatus,
    Platform,
    RenderContext,
    Scalar,
    SectionBoundaries,
    SectionContent,
    Selector,
    SkillArtifact,
    StructuredContent,
    SupportAsset,
    Target,
    TargetScope,
    TreeContent,
    inspect,
    install,
    render,
    render_skills,
    semantic_bytes,
    sync,
    update,
    value_from_record,
)


class EntryRecord(TypedDict):
    path: str
    data_hex: str | None


class ConfigCase(TypedDict):
    name: str
    manifest: dict[str, object]
    entries: list[EntryRecord]
    content_digest: str


CASES = cast(list[ConfigCase], json.loads((FIXTURES / "configurations.json").read_text())["cases"])


class SemanticCase(TypedDict):
    record: list[object]
    encoding_hex: str
    sha256: str


SEMANTIC_CASES = cast(
    list[SemanticCase],
    json.loads((FIXTURES / "configurations.json").read_text())["semantic_values"],
)


@pytest.mark.parametrize("case", SEMANTIC_CASES)
def test_shared_semantic_value_vectors(case: SemanticCase) -> None:
    encoded = semantic_bytes(value_from_record(case["record"]))
    assert encoded == bytes.fromhex(case["encoding_hex"])
    assert hashlib.sha256(encoded).hexdigest() == case["sha256"]


def entries(case: ConfigCase) -> tuple[BundleEntry, ...]:
    return tuple(
        BundleEntry(
            item["path"], None if item["data_hex"] is None else bytes.fromhex(item["data_hex"])
        )
        for item in case["entries"]
    )


@pytest.mark.parametrize("case", CASES, ids=lambda case: case["name"])
def test_configuration_sources_produce_equivalent_snapshots(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    case: ConfigCase,
) -> None:
    package = tmp_path / "fixture_config_package"
    source = package / "flyrail"
    source.mkdir(parents=True)
    (package / "__init__.py").write_bytes(b"")
    (source / "flyrail.json").write_text(json.dumps(case["manifest"]))
    for entry in entries(case):
        path = source / entry.path
        if entry.data is None:
            path.mkdir(parents=True, exist_ok=True)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(entry.data)
    monkeypatch.syspath_prepend(str(tmp_path))
    imported = importlib.import_module(package.name)
    try:
        loaded = [
            Bundle.from_memory(case["manifest"], entries(case)),
            Bundle.from_directory(source),
            Bundle.from_package(imported),
        ]
        archive = tmp_path / "bundle.zip"
        with zipfile.ZipFile(archive, "w") as stream:
            for path in source.rglob("*"):
                stream.write(path, "flyrail/" + path.relative_to(source).as_posix())
        loaded.append(Bundle.from_zip(archive))
        memory = loaded[0]
        loaded.append(
            Bundle.from_artifacts(
                memory.identity,
                memory.artifacts,
                assets=memory.assets,
                dependencies=memory.dependencies,
            )
        )
        shutil.rmtree(source)
        archive.unlink()
        for bundle in loaded:
            assert bundle.artifacts == memory.artifacts
            assert bundle.assets == memory.assets
            assert bundle.dependencies == memory.dependencies
            assert bundle.content_digest == case["content_digest"]
            assert bundle.schema_version == 2
        assert memory.source_roots == ()
        assert loaded[1].source_roots == (source.resolve(),)
        assert loaded[3].source_roots == (archive.resolve(),)
    finally:
        sys.modules.pop(package.name, None)


def test_generated_snapshot_is_immutable_and_hash_tracks_semantics() -> None:
    artifacts = [InstructionArtifact("guide", "hello")]
    original = Bundle.from_artifacts(BundleIdentity("team", "release"), artifacts)
    artifacts.append(InstructionArtifact("extra", "later"))
    assert original.artifacts == (InstructionArtifact("guide", "hello"),)
    assert {original: True}[original]
    with pytest.raises(FrozenInstanceError):
        original.artifacts = ()  # type: ignore[misc]
    renamed = Bundle.from_artifacts(BundleIdentity("other", "old label"), original.artifacts)
    assert renamed.content_digest == original.content_digest
    changed = Bundle.from_artifacts(original.identity, [InstructionArtifact("guide", "changed")])
    assert changed.content_digest != original.content_digest
    reordered = Bundle.from_artifacts(original.identity, reversed(artifacts))
    assert (
        reordered.content_digest
        == Bundle.from_artifacts(original.identity, artifacts).content_digest
    )


def test_bundle_freezes_caller_owned_environment_and_object_pairs() -> None:
    env_pair: Any = ["MODE", "before"]
    value_pair: Any = ["key", Scalar("before")]
    command = Command(["run"], [env_pair])
    value = ObjectValue([value_pair])
    bundle = Bundle.from_artifacts(
        BundleIdentity("team", "1"),
        [
            McpArtifact("server", "server", command),
            NativeArtifact(
                "native",
                Family.MCP,
                Audience(Agent.CODEX, TargetScope.PROJECT),
                "config.toml",
                StructuredContent(DocumentFormat.TOML, Selector([Key("mcp")]), value),
            ),
        ],
    )
    digest = bundle.content_digest
    snapshot_hash = hash(bundle)
    env_pair[:] = ["CHANGED", "after"]
    value_pair[:] = ["changed", Scalar("after")]
    assert command.env == (("MODE", "before"),)
    assert value.items == (("key", Scalar("before")),)
    assert hash(bundle) == snapshot_hash
    assert Bundle.from_artifacts(bundle.identity, bundle.artifacts).content_digest == digest


def test_schema_one_memory_loading_preserves_skill_content() -> None:
    manifest = {
        "schema_version": 1,
        "id": "team",
        "version": "1",
        "skills": [{"name": "review", "path": "skills/review"}],
    }
    bundle = Bundle.from_memory(manifest, [BundleEntry("skills/review/SKILL.md", b"markdown")])
    assert bundle.entries == (BundleEntry("review"), BundleEntry("review/SKILL.md", b"markdown"))
    assert bundle.artifacts == (
        SkillArtifact("review", "review", TreeContent([BundleEntry("SKILL.md", b"markdown")])),
    )


def test_configuration_cannot_be_silently_installed_by_skill_lifecycle(tmp_path: Path) -> None:
    bundle = Bundle.from_memory(CASES[1]["manifest"])
    for operation in (install, inspect):
        with pytest.raises(ValueError, match="explicit rendering"):
            operation(bundle, [Target.directory(tmp_path / "skills")])
    assert list(tmp_path.iterdir()) == []


def test_skill_convenience_rejects_support_graph_before_mutation(tmp_path: Path) -> None:
    skill = SkillArtifact("review", "review", TreeContent([BundleEntry("SKILL.md", b"review")]))
    helper = SupportAsset(
        "helper", Family.SKILLS, TreeContent([BundleEntry("helper.py", b"helper")])
    )
    bundle = Bundle.from_artifacts(
        BundleIdentity("team", "1"),
        [skill],
        assets=[helper],
        dependencies=[Dependency("review", "helper")],
    )
    target = Target.project("codex", tmp_path / "project")
    before = snapshot(tmp_path)
    with pytest.raises(ValueError, match="explicit rendering"):
        render_skills(bundle, target)
    for operation in (install, update, inspect):
        with pytest.raises(ValueError, match="explicit rendering"):
            operation(bundle, [target])
        assert snapshot(tmp_path) == before

    output = render(bundle, RenderContext(target, Platform.LINUX))
    assert {artifact.id for artifact in output.artifacts} == {"review", "helper"}
    assert output.dependencies == (Dependency("review", "helper"),)
    assert (
        sync(bundle, output, InstallationTarget(tmp_path / "index")).status
        is OperationStatus.APPLIED
    )
    assert (
        next(item.destination for item in output.artifacts if item.id == "helper") / "helper.py"
    ).read_bytes() == b"helper"


@pytest.mark.parametrize(
    "field,value",
    [
        ("schema_version", 2.0),
        ("schema_version", True),
        ("artifacts", []),
        ("artifacts", {}),
        ("permissions", {}),
        ("settings", {}),
        ("plugins", []),
        ("templates", []),
        ("assets", [{"id": "unused", "family": "hooks", "path": "support"}]),
        ("dependencies", [{"dependent": "guide", "required": "missing"}]),
    ],
)
def test_configuration_manifest_rejects_invalid_scope_and_structure(
    field: str, value: object
) -> None:
    manifest = dict(CASES[1]["manifest"])
    manifest[field] = value
    with pytest.raises(ValueError):
        Bundle.from_memory(manifest, [BundleEntry("support")])


@pytest.mark.parametrize(
    "artifact",
    [
        {"kind": "settings", "id": "settings"},
        {"kind": "instruction", "id": "guide"},
        {"kind": "instruction", "id": "guide", "text": "one", "path": "other"},
        {"kind": "instruction", "id": "guide", "path": "../other"},
        {"kind": "instruction", "id": "guide", "path": "missing"},
        {"kind": "instruction", "id": "guide", "text": "hi", "unknown": True},
        {"kind": "skill", "id": "review", "name": "review", "path": "missing"},
        {
            "kind": "skill",
            "id": "review",
            "name": "review",
            "path": "support",
            "executables": ["missing"],
        },
        {
            "kind": "skill",
            "id": "review",
            "name": "review",
            "path": "support",
            "executables": ["a", "a"],
        },
        {"kind": "mcp", "id": "server", "name": "server", "transport": {"kind": "other"}},
        {
            "kind": "mcp",
            "id": "server",
            "name": "server",
            "transport": {"kind": "stdio", "command": {"argv": [], "env": []}},
        },
        {
            "kind": "hook",
            "id": "hook",
            "event": "stop",
            "command": {"argv": ["test"]},
            "protocol_version": True,
        },
        {
            "kind": "hook",
            "id": "hook",
            "event": "stop",
            "command": {"argv": ["test"]},
            "protocol_version": 1,
            "timeout_ms": True,
        },
    ],
)
def test_invalid_artifact_declarations(artifact: dict[str, object]) -> None:
    manifest = dict(CASES[1]["manifest"], artifacts=[artifact])
    with pytest.raises(ValueError):
        Bundle.from_memory(manifest, [BundleEntry("support")])


def test_source_paths_do_not_affect_configuration_content_identity() -> None:
    manifest = dict(
        CASES[1]["manifest"], artifacts=[{"kind": "instruction", "id": "guide", "path": "first.md"}]
    )
    first = Bundle.from_memory(manifest, [BundleEntry("first.md", b"Hello")])
    manifest["artifacts"] = [{"kind": "instruction", "id": "guide", "path": "other.md"}]
    second = Bundle.from_memory(manifest, [BundleEntry("other.md", b"Hello")])
    assert first.content_digest == second.content_digest == CASES[1]["content_digest"]


def test_native_manifests_support_custom_boundaries_and_toml_scalars() -> None:
    native: dict[str, object] = {
        "kind": "native",
        "id": "guide",
        "family": "instructions",
        "audience": {"agent": "codex", "scope": "project"},
        "destination": "AGENTS.md",
        "content": {
            "kind": "section",
            "marker": "guide",
            "text": "check the build",
            "boundaries": {"start": "<!-- >>> guide >>> -->", "end": "<!-- <<< guide <<< -->"},
        },
    }
    manifest = dict(CASES[1]["manifest"], artifacts=[native])
    bundle = Bundle.from_memory(manifest)
    artifact = bundle.artifacts[0]
    assert isinstance(artifact, NativeArtifact)
    assert isinstance(artifact.content, SectionContent)
    assert artifact.content.selector == SectionBoundaries(
        "<!-- >>> guide >>> -->", "<!-- <<< guide <<< -->"
    )
    native["content"] = {
        "kind": "structured",
        "format": "toml",
        "selector": ["records", {"semantic_identity": ["date", "2026-01-02"]}],
        "semantic_value": ["date", "2026-01-02"],
    }
    bundle = Bundle.from_memory(manifest)
    artifact = bundle.artifacts[0]
    assert isinstance(artifact, NativeArtifact)
    assert isinstance(artifact.content, StructuredContent)
    assert artifact.content.value == Scalar(dt.date(2026, 1, 2))
    native["content"] = {
        "kind": "structured",
        "format": "json",
        "selector": ["key"],
        "value": 1,
        "semantic_value": ["integer", "1"],
    }
    with pytest.raises(ValueError, match="exactly one"):
        Bundle.from_memory(manifest)
    native["content"] = {"kind": "yaml"}
    with pytest.raises(ValueError, match="unknown native content"):
        Bundle.from_memory(manifest)


def test_config_snapshot_validates_assets_and_dependencies() -> None:
    identity = BundleIdentity("team", "1")
    instruction = InstructionArtifact("guide", "hello")
    asset = SupportAsset("support", Family.INSTRUCTIONS, TreeContent([]))
    with pytest.raises(ValueError, match="unique"):
        Bundle.from_artifacts(identity, [instruction, instruction])
    with pytest.raises(ValueError, match="explicit dependent"):
        Bundle.from_artifacts(identity, [instruction], assets=[asset])
    with pytest.raises(ValueError, match="cyclic"):
        Bundle.from_artifacts(
            identity,
            [instruction],
            assets=[asset],
            dependencies=[Dependency("guide", "support"), Dependency("support", "guide")],
        )
    with pytest.raises(ValueError, match="duplicate dependency"):
        Bundle.from_artifacts(
            identity,
            [instruction],
            assets=[asset],
            dependencies=[Dependency("guide", "support")] * 2,
        )
    with pytest.raises(ValueError, match="duplicate skill"):
        skill = SkillArtifact("review", "review", TreeContent([BundleEntry("SKILL.md", b"")]))
        Bundle.from_artifacts(identity, [skill, replace(skill, id="second")])


def test_memory_snapshots_reject_mutable_or_unsafe_values() -> None:
    with pytest.raises(TypeError, match="manifest"):
        Bundle.from_memory([])  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="identity"):
        Bundle.from_artifacts("team", [])  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="artifact"):
        Bundle.from_artifacts(BundleIdentity("team", "1"), [object()])  # type: ignore[list-item]
    with pytest.raises(TypeError, match="assets"):
        Bundle.from_artifacts(
            BundleIdentity("team", "1"),
            [InstructionArtifact("guide", "x")],
            assets=[object()],  # type: ignore[list-item]
        )
    with pytest.raises(ValueError, match="duplicate JSON"):
        Bundle.from_memory(b'{"schema_version":2,"schema_version":2}')


@pytest.mark.parametrize(
    "value",
    [
        {1: "coerced"},
        {True: "coerced"},
        {None: "coerced"},
        [{1: "coerced"}],
        {"nested": {1: "coerced"}},
        {"tuple": ("coerced",)},
        {"date": dt.date(2026, 1, 2)},
        {"integer": 2**63},
        {"float": float("nan")},
    ],
)
def test_memory_manifest_rejects_non_json_nested_values(value: object) -> None:
    manifest = dict(
        CASES[1]["manifest"],
        artifacts=[
            {
                "kind": "native",
                "id": "server",
                "family": "mcp",
                "audience": {"agent": "codex", "scope": "project"},
                "destination": "config.toml",
                "content": {
                    "kind": "structured",
                    "format": "toml",
                    "selector": ["mcp_servers", "server"],
                    "value": value,
                },
            }
        ],
    )
    with pytest.raises((TypeError, ValueError)):
        Bundle.from_memory(manifest)


@pytest.mark.parametrize("container", [{}, []])
def test_memory_manifest_rejects_circular_containers(container: Any) -> None:
    if isinstance(container, dict):
        container["self"] = container
    else:
        container.append(container)
    with pytest.raises(ValueError, match="circular"):
        Bundle.from_memory(dict(CASES[1]["manifest"], extra=container))


def test_memory_manifest_accepts_reused_non_circular_objects() -> None:
    command = {"argv": ["run"]}
    artifacts = [
        {
            "id": name,
            "kind": "mcp",
            "name": name,
            "transport": {"kind": "stdio", "command": command},
        }
        for name in ("first", "second")
    ]
    bundle = Bundle.from_memory(dict(CASES[1]["manifest"], artifacts=artifacts))
    assert bundle.artifacts == tuple(
        McpArtifact(name, name, Command(["run"])) for name in ("first", "second")
    )
