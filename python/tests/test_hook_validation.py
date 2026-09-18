import json
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from test_hook_lifecycle import schema_artifact
from test_hooks import bundle_of, context, make_hook

from flyrail import (
    Dependency,
    DependencyMode,
    DocumentFormat,
    DocumentSchema,
    Family,
    FileContent,
    HookRuntime,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    Scalar,
    inspect_installation,
    remove,
    render,
    sync,
)


@pytest.mark.parametrize("node_path", [Path("relative"), Path("/"), Path("/node/../bin"), "node"])
def test_native_runtime_path_must_be_explicit_and_unambiguous(node_path: Any) -> None:
    with pytest.raises(ValueError, match="absolute Node"):
        HookRuntime(node_path)


def test_runtime_configuration_rejects_untyped_shell_and_snapshot(tmp_path: Path) -> None:
    invalid: Any = "posix"
    with pytest.raises(TypeError, match="HookShell"):
        HookRuntime(shell=invalid)
    with pytest.raises(TypeError, match="HookRuntime"):
        replace(context(tmp_path, "claude"), hook_runtime=invalid)


@pytest.mark.parametrize("kind", ["format", "value", "duplicate", "mismatch", "whole-file", "type"])
def test_document_schema_requirements_reject_unrepresentable_inputs(
    tmp_path: Path, kind: str
) -> None:
    invalid: Any = []
    schema = DocumentSchema(DocumentFormat.JSONC, "version", Scalar(1))
    artifact = schema_artifact(tmp_path / "config.json")
    with pytest.raises((TypeError, ValueError)):
        if kind == "format":
            DocumentSchema(DocumentFormat.TOML, "version", Scalar(1))
        elif kind == "value":
            DocumentSchema(DocumentFormat.JSONC, "version", invalid)
        elif kind == "duplicate":
            replace(artifact, schema_requirements=(schema, schema))
        elif kind == "mismatch":
            replace(artifact, schema_requirements=(replace(schema, format=DocumentFormat.JSON),))
        elif kind == "whole-file":
            replace(artifact, content=FileContent(b"{}"))
        else:
            replace(artifact, schema_requirements=(invalid,))


@pytest.mark.parametrize("kind", ["mode", "duplicate", "structured", "transitive"])
def test_stable_references_require_a_single_whole_file_routing_boundary(
    tmp_path: Path, kind: str
) -> None:
    invalid: Any = "stable-reference"
    entry = RenderedArtifact("entry", Family.HOOKS, tmp_path / "entry", FileContent(b"entry"))
    config = schema_artifact(tmp_path / "config.json")
    asset = replace(entry, id="asset", destination=tmp_path / "asset")
    stable = Dependency("handler", "entry", DependencyMode.STABLE_REFERENCE)
    with pytest.raises((TypeError, ValueError)):
        if kind == "mode":
            Dependency("handler", "entry", invalid)
        elif kind == "duplicate":
            RenderedBundle([config, entry], [stable, Dependency("handler", "entry")])
        elif kind == "structured":
            RenderedBundle([config, entry], [Dependency("entry", "handler", stable.mode)])
        else:
            RenderedBundle(
                [config, entry, asset], [stable, Dependency("entry", "asset", stable.mode)]
            )


@pytest.mark.parametrize(
    "damage",
    [
        "stable-outside",
        "schema-duplicate",
        "schema-format",
        "scaffold-takeover",
        "scaffold-duplicate",
        "scaffold-empty",
    ],
)
def test_corrupt_schema_authority_preserves_document_and_receipt(
    tmp_path: Path, damage: str
) -> None:
    path = tmp_path / "config.json"
    target = InstallationTarget(tmp_path / "index")
    bundle = bundle_of(make_hook())
    rendered = RenderedBundle([schema_artifact(path)])
    assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
    state = ResourceAuthority(path).state_root / "receipt.json"
    raw = json.loads(state.read_text())
    owned = raw["fields"]["claims"][0]["fields"]
    provenance = raw["fields"]["provenance"]["fields"]
    if damage == "stable-outside":
        owned["stable_requirements"] = [[{"path": str(tmp_path / "foreign")}, "a" * 64]]
    elif damage == "schema-duplicate":
        owned["schema_requirements"] *= 2
    elif damage == "schema-format":
        owned["schema_requirements"][0]["fields"]["format"]["value"] = "json"
    elif damage == "scaffold-takeover":
        provenance["schema_fields"][0]["fields"]["disposition"]["value"] = "adopted"
    elif damage == "scaffold-duplicate":
        provenance["schema_fields"] *= 2
    else:
        provenance["schema_empty_document"] = {"bytes": b"foreign".hex()}
    state.write_text(json.dumps(raw))
    original_document, original_receipt = path.read_bytes(), state.read_bytes()
    assert not inspect_installation(bundle.id, target).is_current
    assert remove(bundle.id, target).status is OperationStatus.FAILED
    assert path.read_bytes() == original_document
    assert state.read_bytes() == original_receipt


@pytest.mark.parametrize("name", ["runner.mts", "bridge.mts", "process.mts"])
def test_snapshot_remains_detached_from_changed_package_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    import flyrail.hooks as preparation

    ctx = context(tmp_path, "pi")
    assert ctx.hook_runtime is not None
    package = tmp_path / "package"
    source = package / "runtime"
    source.mkdir(parents=True)
    for entry in ctx.hook_runtime.resources:
        assert entry.data is not None
        (source / entry.path).write_bytes(entry.data)
    monkeypatch.setattr(preparation, "files", lambda package_name: package)
    runtime = HookRuntime()
    ctx = replace(ctx, hook_runtime=runtime)
    bundle = bundle_of(make_hook())
    expected = render(bundle, ctx)
    (source / name).write_bytes(b"changed package source")
    assert render(bundle, ctx) == expected
    assert (
        render(bundle, replace(ctx, hook_runtime=HookRuntime())).content_digest
        != expected.content_digest
    )
