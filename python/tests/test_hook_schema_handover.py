import json
import re
from dataclasses import replace
from pathlib import Path

import pytest
from skill_helpers import snapshot
from test_configuration_lifecycle import receipt
from test_hook_lifecycle import schema_artifact
from test_hooks import bundle_of, context, make_hook

from flyrail import (
    Acquisition,
    Agent,
    Audience,
    Bundle,
    BundleIdentity,
    Disposition,
    DocumentFormat,
    ErrorCode,
    Family,
    InstallationTarget,
    Key,
    Member,
    NativeArtifact,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    Scalar,
    Selector,
    StructuredContent,
    TargetScope,
    freeze_value,
    inspect_installation,
    remove,
    render,
    sync,
)

pytestmark = pytest.mark.integration


def native_cursor(value: int) -> Bundle:
    audience = Audience(Agent.CURSOR, TargetScope.PROJECT)
    return Bundle.from_artifacts(
        BundleIdentity("hook-demo", str(value)),
        [
            NativeArtifact(
                "native-version",
                Family.HOOKS,
                audience,
                ".cursor/hooks.json",
                StructuredContent(DocumentFormat.JSON, Selector([Key("version")]), Scalar(value)),
            ),
            NativeArtifact(
                "native-hook",
                Family.HOOKS,
                audience,
                ".cursor/hooks.json",
                StructuredContent(
                    DocumentFormat.JSON,
                    Selector(
                        [
                            Key("hooks"),
                            Key("beforeShellExecution"),
                            Member(Scalar("authored-command"), ["command"]),
                        ]
                    ),
                    freeze_value({"command": "authored-command"}),
                ),
            ),
        ],
    )


@pytest.mark.parametrize("acquisition", list(Acquisition))
@pytest.mark.parametrize(("current", "desired"), [(1, 1), (1, 2), (2, 2)])
def test_cursor_schema_handover_uses_unowned_acquisition_policy(
    tmp_path: Path, acquisition: Acquisition, current: int, desired: int
) -> None:
    ctx = context(tmp_path, "cursor")
    if ctx.platform == "windows":
        pytest.skip("Cursor native shell platform boundary")
    target = InstallationTarget(tmp_path / "index")
    original = bundle_of(make_hook())
    assert sync(original, render(original, ctx), target).status is OperationStatus.APPLIED
    path = tmp_path / ".cursor/hooks.json"
    if current != 1:
        path.write_text(re.sub(r'("version"\s*:\s*)1', rf"\g<1>{current}", path.read_text()))
    before, previous = snapshot(tmp_path), receipt(path)
    bundle = native_cursor(desired)
    rendered = render(bundle, ctx)
    assert rendered.supported
    result = sync(bundle, rendered, target, acquisition=acquisition)
    if acquisition is Acquisition.CONFLICT or (
        acquisition is Acquisition.ADOPT and current != desired
    ):
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and result.error.code is ErrorCode.CONFLICT
        assert snapshot(tmp_path) == before
        assert receipt(path) == previous
        assert remove(original.id, target).status is OperationStatus.APPLIED
        if current == 1:
            assert not path.exists()
        else:
            assert json.loads(path.read_text()) == {"version": current}
        return
    assert result.status is OperationStatus.APPLIED
    owned = next(item for item in receipt(path).claims if item.artifact_id == "native-version")
    assert owned.selection is not None
    assert owned.selection.disposition is (
        Disposition.ADOPTED if acquisition is Acquisition.ADOPT else Disposition.TAKEN_OVER
    )
    assert (owned.selection.baseline.value if owned.selection.baseline else None) == (
        Scalar(current) if acquisition is Acquisition.TAKEOVER else None
    )
    assert receipt(path).provenance.schema_fields == ()
    assert inspect_installation(bundle.id, target).is_current
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    if acquisition is Acquisition.TAKEOVER:
        assert json.loads(path.read_text()) == {"version": current}
    else:
        assert not path.exists()


@pytest.mark.parametrize("original", [None, b"", b" \n", b"{}", b'{"foreign":[2,1],"other":{}}'])
@pytest.mark.parametrize("acquisition", [Acquisition.ADOPT, Acquisition.TAKEOVER])
def test_cursor_schema_handover_preserves_first_baseline_and_document_provenance(
    tmp_path: Path, original: bytes | None, acquisition: Acquisition
) -> None:
    ctx = context(tmp_path, "cursor")
    if ctx.platform == "windows":
        pytest.skip("Cursor native shell platform boundary")
    path = tmp_path / ".cursor/hooks.json"
    if original is not None:
        path.parent.mkdir()
        path.write_bytes(original)
    target = InstallationTarget(tmp_path / "index")
    portable = bundle_of(make_hook())
    assert sync(portable, render(portable, ctx), target).status is OperationStatus.APPLIED
    bundle = native_cursor(1)
    assert (
        sync(bundle, render(bundle, ctx), target, acquisition=acquisition).status
        is OperationStatus.APPLIED
    )
    first = next(item for item in receipt(path).claims if item.artifact_id == "native-version")
    assert first.selection is not None
    for value in [2, 3]:
        bundle = native_cursor(value)
        assert sync(bundle, render(bundle, ctx), target).status is OperationStatus.APPLIED
        current = next(
            item for item in receipt(path).claims if item.artifact_id == "native-version"
        )
        assert current.selection is not None
        assert current.selection.baseline == first.selection.baseline
        assert receipt(path).provenance.schema_empty_document == (
            original if original is not None and not original.strip() else None
        )
        assert json.loads(path.read_text())["version"] == value
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    if acquisition is Acquisition.TAKEOVER:
        expected = json.loads(original) if original is not None and original.strip() else {}
        assert json.loads(path.read_text()) == {**expected, "version": 1}
    else:
        assert (path.read_bytes() if path.exists() else None) == original


@pytest.mark.parametrize("acquisition", list(Acquisition))
def test_cursor_schema_handover_cannot_capture_another_bundles_requirement(
    tmp_path: Path, acquisition: Acquisition
) -> None:
    ctx = context(tmp_path, "cursor")
    if ctx.platform == "windows":
        pytest.skip("Cursor native shell platform boundary")
    targets = [InstallationTarget(tmp_path / name) for name in ["first", "second"]]
    bundles = [
        bundle_of(make_hook()),
        Bundle.from_artifacts(BundleIdentity("another", "1"), [make_hook()]),
    ]
    for bundle, target in zip(bundles, targets, strict=True):
        assert sync(bundle, render(bundle, ctx), target).status is OperationStatus.APPLIED
    path = tmp_path / ".cursor/hooks.json"
    before, previous = path.read_bytes(), receipt(path)
    native = native_cursor(1)
    result = sync(native, render(native, ctx), targets[0], acquisition=acquisition)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and "schema overlaps" in result.error.message
    assert path.read_bytes() == before
    assert receipt(path) == previous
    assert inspect_installation("another", targets[1]).is_current
    for bundle, target in zip(bundles, targets, strict=True):
        assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert not path.exists()


def test_explicit_jsonc_schema_handover_preserves_foreign_comments(tmp_path: Path) -> None:
    path = tmp_path / "hooks.json"
    original = b'{\n// foreign\n"values":[2,1],\n}\n'
    path.write_bytes(original)
    target = InstallationTarget(tmp_path / "index")
    bundle = bundle_of(make_hook())
    artifact = schema_artifact(path)
    assert sync(bundle, RenderedBundle([artifact]), target).status is OperationStatus.APPLIED
    native = RenderedArtifact(
        "version",
        Family.HOOKS,
        path,
        StructuredContent(DocumentFormat.JSONC, Selector([Key("version")]), Scalar(1)),
    )
    assert (
        sync(bundle, RenderedBundle([native]), target, acquisition=Acquisition.ADOPT).status
        is OperationStatus.APPLIED
    )
    updated = replace(
        native,
        content=StructuredContent(DocumentFormat.JSONC, Selector([Key("version")]), Scalar(2)),
    )
    assert sync(bundle, RenderedBundle([updated]), target).status is OperationStatus.APPLIED
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert path.read_bytes() == original


@pytest.mark.parametrize(
    "original", [b'{// comment\n"version":1}', b'{"version":1,}', b'{"version":']
)
def test_portable_cursor_requires_parseable_ordinary_json(tmp_path: Path, original: bytes) -> None:
    ctx = context(tmp_path, "cursor")
    if ctx.platform == "windows":
        pytest.skip("Cursor native shell platform boundary")
    path = tmp_path / ".cursor/hooks.json"
    path.parent.mkdir()
    path.write_bytes(original)
    target = InstallationTarget(tmp_path / "index")
    bundle = bundle_of(make_hook())
    rendered = render(bundle, ctx)
    result = sync(bundle, rendered, target, acquisition=Acquisition.TAKEOVER)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None
    assert "JSON" in result.error.message or "Expecting" in result.error.message
    assert path.read_bytes() == original
