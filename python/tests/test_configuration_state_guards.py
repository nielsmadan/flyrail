import copy
import json
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    Acquisition,
    Ancestor,
    ErrorCode,
    Family,
    FileContent,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    ResourceKind,
    Target,
    inspect,
    install,
    remove,
    sync,
)
from flyrail._codec import encode
from flyrail._resource_models import ResourceRef
from flyrail.authority import state_path
from flyrail.observations import (
    Conflict,
    Installation,
    Modification,
    ModificationKind,
    Observation,
    ObservationState,
    TargetError,
    TargetInspection,
    TargetResult,
)


@pytest.mark.parametrize(
    "damage",
    [
        "node-path",
        "node-data",
        "node-mode",
        "node-security",
        "revision-root",
        "revision-parent",
        "revision-order",
        "authority-version",
        "authority-kind",
        "authority-root",
        "claim-kind",
        "disposition",
        "installed-absent",
        "baseline-absent",
        "baseline-empty",
        "installed-selection",
        "receipt-resource",
        "receipt-provenance",
        "requirements-duplicate",
        "empty-claim-revision",
    ],
)
def test_invalid_complete_ownership_records_preserve_every_byte(
    tmp_path: Path, damage: str
) -> None:
    destination = tmp_path / "resource"
    destination.write_bytes(b"original baseline")
    target = InstallationTarget(tmp_path / "index")
    content = RenderedBundle(
        [RenderedArtifact("file", Family.HOOKS, destination, FileContent(b"owned"))]
    )
    assert (
        sync(bundle(), content, target, acquisition=Acquisition.TAKEOVER).status
        is OperationStatus.APPLIED
    )
    path = ResourceAuthority(destination).state_root / "receipt.json"
    record = json.loads(path.read_bytes())
    fields = record["fields"]
    owned = fields["claims"][0]["fields"]
    revision = owned["installed"]["fields"]
    node = revision["nodes"][0]["fields"]
    if damage == "node-path":
        node["path"] = False
    elif damage == "node-data":
        node["data"] = []
    elif damage == "node-mode":
        node["mode"] = 0o4755
    elif damage == "node-security":
        node["security"] = []
    elif damage == "revision-root":
        revision["nodes"] *= 2
    elif damage == "revision-parent":
        child = copy.deepcopy(revision["nodes"][0])
        child["fields"]["path"] = "child"
        revision["nodes"].append(child)
    elif damage == "revision-order":
        node["data"] = None
        for name in ["z", "a"]:
            child = copy.deepcopy(revision["nodes"][0])
            child["fields"]["path"] = name
            revision["nodes"].append(child)
    elif damage == "authority-version":
        fields["resource"]["fields"]["schema_version"] = True
    elif damage == "authority-kind":
        fields["resource"]["fields"]["kind"] = "document"
    elif damage == "authority-root":
        fields["resource"]["fields"]["destination"] = {"path": destination.anchor}
    elif damage == "claim-kind":
        owned["claim"] = []
    elif damage == "disposition":
        owned["disposition"] = "created"
    elif damage == "installed-absent":
        owned["installed"] = None
    elif damage == "baseline-absent":
        owned["baseline"] = None
    elif damage == "baseline-empty":
        owned["baseline"]["fields"]["nodes"] = []
    elif damage == "installed-selection":
        owned["selection"] = []
    elif damage == "receipt-resource":
        fields["resource"] = []
    elif damage == "receipt-provenance":
        fields["provenance"] = []
    elif damage == "requirements-duplicate":
        owned["requirements"] = [[{"path": str(tmp_path / "dependency")}, "a" * 64]] * 2
    else:
        revision["nodes"] = []
    path.write_text(json.dumps(record))
    rejected = remove("team", target)
    assert rejected.status is OperationStatus.FAILED
    assert rejected.error is not None and rejected.error.code is ErrorCode.INVALID_STATE
    assert destination.read_bytes() == b"owned"
    assert json.loads(path.read_bytes()) == record


@pytest.mark.parametrize(
    "damage",
    [
        "membership-ref",
        "duplicate-id",
        "index-generation",
        "residual-type",
        "unknown-index-entry",
        "stale-generation",
    ],
)
def test_index_generation_is_verified_without_authorizing_new_ownership(
    tmp_path: Path, damage: str
) -> None:
    destination = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(destination), target).status is OperationStatus.APPLIED
    path = target.index_root / "index.json"
    raw = json.loads(path.read_bytes())
    fields = raw["fields"]
    membership = fields["current"]["fields"]["membership"]
    if damage == "membership-ref":
        membership[0][0] = []
    elif damage == "duplicate-id":
        membership[0][1] *= 2
    elif damage == "index-generation":
        fields["previous"] = []
    elif damage == "residual-type":
        fields["residual"] = [[]]
    elif damage == "unknown-index-entry":
        (target.index_root / "other").write_bytes(b"retained")
    else:
        fields["current"]["fields"]["version"] = "stale"
    path.write_text(json.dumps(raw))
    from flyrail import inspect_installation

    observation = inspect_installation("team", target)
    assert not observation.is_current
    assert b"generated" in destination.read_bytes()
    if damage != "stale-generation":
        assert observation.error is not None


@pytest.mark.parametrize(
    "kind,field",
    [
        ("error", "code"),
        ("error", "message"),
        ("error", "path"),
        ("error", "errno"),
        ("installation", "entries"),
        ("installation", "transaction_id"),
        ("modification", "path"),
        ("modification", "kind"),
        ("conflict", "owner"),
        ("observation", "state"),
        ("observation", "installed"),
        ("observation", "error"),
        ("observation", "version_matches"),
        ("observation", "modifications"),
        ("inspection", "target"),
        ("inspection", "observation"),
        ("inspection", "root"),
        ("inspection", "alias_of"),
        ("result", "status"),
        ("result", "recovery_paths"),
        ("ancestor", "path"),
        ("ancestor", "device"),
        ("ancestor", "security"),
    ],
)
def test_public_status_values_cannot_retain_mutable_caller_data(
    tmp_path: Path, kind: str, field: str
) -> None:
    target = Target.directory(tmp_path / "skills")
    observed = Observation(ObservationState.ABSENT)
    values: dict[str, Any] = {
        "error": TargetError(ErrorCode.CONFLICT, "conflict"),
        "installation": Installation("team", "1", "a" * 64, "b" * 32, ()),
        "modification": Modification("team", "skill", ModificationKind.MISSING),
        "conflict": Conflict("skill", None),
        "observation": observed,
        "inspection": TargetInspection(target, target.root, tmp_path / "state", None, observed),
        "result": TargetResult(
            target, target.root, tmp_path / "state", None, OperationStatus.UNCHANGED, observed
        ),
        "ancestor": Ancestor(tmp_path, 1, 2, 0o755, 3, 4),
    }
    with pytest.raises((TypeError, ValueError)):
        replace(values[kind], **{field: [[]]})


def test_skill_context_aliases_and_corrupt_receipts_have_typed_results(tmp_path: Path) -> None:
    from skill_helpers import make_bundle

    b = make_bundle(tmp_path / "bundle")
    target = Target.directory(tmp_path / "skills")
    alias = Target.user("claude", home=tmp_path, env={"CLAUDE_CONFIG_DIR": str(tmp_path)})
    assert install(b, [target])[0].status is OperationStatus.APPLIED
    results = inspect(b, [target, alias])
    assert results[1].alias_of == 0 and results[1].observation.is_current
    assert results[0].observation.installed is not None
    assert any(entry.is_directory for entry in results[0].observation.installed.entries)
    state = ResourceAuthority(target.root).state_root
    (state / "receipt.json").write_bytes(b"{}")
    rejected = inspect(b, [target])[0]
    assert rejected.observation.state is ObservationState.UNKNOWN
    assert install(b, [target])[0].status is OperationStatus.FAILED


@pytest.mark.parametrize("invalid", [[], ["not a target"]])
def test_skill_empty_or_invalid_target_set_is_rejected(tmp_path: Path, invalid: Any) -> None:
    with pytest.raises((TypeError, ValueError)):
        install(bundle(), invalid)


def test_codec_refuses_mutable_values_and_noncanonical_destination(tmp_path: Path) -> None:
    with pytest.raises(TypeError):
        encode({"borrowed": []})
    ref = ResourceRef(
        tmp_path / "resource",
        ResourceKind.DOCUMENT,
        ResourceAuthority(tmp_path / "resource").state_root,
    )
    moved = tmp_path / "directory"
    moved.mkdir()
    (moved / "link").symlink_to(tmp_path / "resource")
    redirected = replace(
        ref,
        destination=moved / "link",
        state_root=state_path(moved / "link"),
    )
    with pytest.raises(ValueError):
        _ = redirected.authority


@pytest.mark.parametrize("field", ["path", "size", "sha256", "executable"])
def test_inventory_summary_rejects_mutable_scalars(field: str) -> None:
    from flyrail import InventoryEntry

    value = InventoryEntry("skill/file", 4, "a" * 64, False)
    invalid: Any = []
    with pytest.raises((TypeError, ValueError)):
        replace(value, **{field: invalid})
