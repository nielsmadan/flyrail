import hashlib
import json
import os
import shutil
from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest

from flyrail import Bundle, ErrorCode, ModificationKind, ObservationState, Target, inspect
from flyrail._receipts import Receipt, bundle_receipt, receipt_bytes


def make_bundle(
    root: Path,
    identifier: str = "team",
    version: str = "1",
    names: tuple[str, ...] = ("review",),
    data: bytes = b"original",
    executable: bool = False,
) -> Bundle:
    root.mkdir(parents=True)
    specs: list[dict[str, object]] = []
    for name in names:
        skill = root / name
        skill.mkdir()
        (skill / "SKILL.md").write_bytes(data)
        (skill / "empty").mkdir()
        (skill / "run").write_bytes(b"run")
        specs.append({"name": name, "path": name, "executables": ["run"] if executable else []})
    (root / "flyrail.json").write_text(
        json.dumps({"schema_version": 1, "id": identifier, "version": version, "skills": specs}),
        encoding="utf-8",
    )
    return Bundle.from_directory(root)


def materialize(bundle: Bundle, target: Target, *, receipt: bool = True) -> Path:
    for entry in bundle.entries:
        path = target.root / entry.path
        if entry.data is None:
            path.mkdir(parents=True, exist_ok=True)
        else:
            path.write_bytes(entry.data)
            path.chmod(0o755 if entry.executable else 0o644)
    state = target.root.with_name(f".{target.root.name}.flyrail")
    if receipt:
        receipts = state / "receipts"
        receipts.mkdir(parents=True, exist_ok=True)
        transaction_id = hashlib.sha256(bundle.id.encode()).hexdigest()[:32]
        (receipts / f"{bundle.id}.json").write_bytes(
            receipt_bytes(bundle_receipt(bundle, transaction_id))
        )
    return state


def snapshot(root: Path) -> dict[str, tuple[int, int, bytes | None]]:
    return {
        path.relative_to(root).as_posix(): (
            path.stat().st_mode,
            path.stat().st_mtime_ns,
            path.read_bytes() if path.is_file() else None,
        )
        for path in root.rglob("*")
    }


def test_inspect_missing_targets_is_read_only_and_results_are_immutable(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.user("codex", home=tmp_path / "missing")
    before = snapshot(tmp_path)
    (result,) = inspect(bundle, [target])

    assert result.root == target.root
    assert result.state_root == target.root.with_name(".skills.flyrail")
    assert result.target is target
    assert result.alias_of is None
    assert result.observation.state is ObservationState.ABSENT
    assert result.observation.content_matches is False
    assert result.observation.version_matches is None
    assert result.observation.recorded_content_matches is None
    assert snapshot(tmp_path) == before
    with pytest.raises(FrozenInstanceError):
        result.observation.content_matches = True  # type: ignore[misc]
    with pytest.raises(FrozenInstanceError):
        result.root = tmp_path  # type: ignore[misc]


def test_intact_installation_compares_actual_content_and_retains_inventory(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source", executable=True)
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    (state / "lock").touch()
    (state / "staging").mkdir()
    (state / "backup").mkdir()
    (target.root / "unrelated.txt").write_bytes(b"kept")
    before = snapshot(tmp_path)
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.INSTALLED
    assert observed.version_matches is True
    assert observed.recorded_content_matches is True
    assert observed.content_matches is True
    assert observed.modifications == ()
    assert observed.conflicts == ()
    assert observed.error is None
    assert observed.installed is not None
    assert observed.installed == observed.installations[0]
    assert observed.installed.bundle_id == "team"
    assert observed.installed.version == "1"
    assert observed.installed.content_digest == bundle.content_digest
    assert tuple(entry.path for entry in observed.installed.entries) == tuple(
        entry.path for entry in bundle.entries
    )
    assert snapshot(tmp_path) == before
    with pytest.raises(FrozenInstanceError):
        observed.installed.entries[0].path = "other"  # type: ignore[misc]


def test_version_recorded_content_and_local_changes_are_independent(tmp_path: Path) -> None:
    old = make_bundle(tmp_path / "old", version="new-looking")
    relabelled = make_bundle(tmp_path / "relabelled", version="older-looking")
    changed = make_bundle(tmp_path / "changed", version="new-looking", data=b"edited")
    target = Target.directory(tmp_path / "skills")
    materialize(old, target)

    label = inspect(relabelled, [target])[0].observation
    assert (label.version_matches, label.recorded_content_matches, label.content_matches) == (
        False,
        True,
        True,
    )
    content = inspect(changed, [target])[0].observation
    assert (content.version_matches, content.recorded_content_matches, content.content_matches) == (
        True,
        False,
        False,
    )
    (target.root / "review/SKILL.md").write_bytes(b"edited")
    modified = inspect(changed, [target])[0].observation
    assert (
        modified.version_matches,
        modified.recorded_content_matches,
        modified.content_matches,
    ) == (True, False, True)
    assert [(item.path, item.kind) for item in modified.modifications] == [
        ("review/SKILL.md", ModificationKind.CONTENT_CHANGED)
    ]


@pytest.mark.parametrize(
    "change",
    [
        "missing-file",
        "missing-dir",
        "missing-skill",
        "added-file",
        "added-dir",
        "file-to-dir",
        "dir-to-file",
        "executable",
    ],
)
def test_owned_tree_modifications_are_observed(tmp_path: Path, change: str) -> None:
    if change == "executable" and os.name == "nt":
        pytest.skip("Windows retains logical executable intent in receipts")
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    skill = target.root / "review"
    expected = ModificationKind.MISSING
    path = "review/SKILL.md"
    if change == "missing-file":
        (skill / "SKILL.md").unlink()
    elif change == "missing-dir":
        (skill / "empty").rmdir()
        path = "review/empty"
    elif change == "missing-skill":
        shutil.rmtree(skill)
        path = "review"
    elif change == "added-file":
        (skill / "added").write_bytes(b"new")
        expected, path = ModificationKind.ADDED, "review/added"
    elif change == "added-dir":
        (skill / "added").mkdir()
        expected, path = ModificationKind.ADDED, "review/added"
    elif change == "file-to-dir":
        (skill / "SKILL.md").unlink()
        (skill / "SKILL.md").mkdir()
        expected = ModificationKind.TYPE_CHANGED
    elif change == "dir-to-file":
        (skill / "empty").rmdir()
        (skill / "empty").write_bytes(b"replaced")
        expected, path = ModificationKind.TYPE_CHANGED, "review/empty"
    else:
        (skill / "run").chmod(0o755)
        expected, path = ModificationKind.EXECUTABLE_CHANGED, "review/run"
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.INSTALLED
    assert observed.recorded_content_matches is True
    assert observed.content_matches is False
    assert (path, expected) in [(item.path, item.kind) for item in observed.modifications]
    assert all(item.bundle_id == bundle.id for item in observed.modifications)


def test_foreign_ownership_including_missing_skill_stays_a_conflict(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    other = make_bundle(tmp_path / "other", identifier="other")
    target = Target.directory(tmp_path / "skills")
    materialize(other, target)
    first = inspect(bundle, [target])[0].observation

    assert first.state is ObservationState.ABSENT
    assert first.content_matches is True
    assert [(conflict.path, conflict.owner) for conflict in first.conflicts] == [
        ("review", "other")
    ]
    shutil.rmtree(target.root)
    missing = inspect(bundle, [target])[0].observation
    assert [(conflict.path, conflict.owner) for conflict in missing.conflicts] == [
        ("review", "other")
    ]
    assert {change.bundle_id for change in missing.modifications} == {"other"}
    assert {change.kind for change in missing.modifications} == {ModificationKind.MISSING}


def test_untracked_identical_skill_and_retired_owned_skill(tmp_path: Path) -> None:
    old = make_bundle(tmp_path / "old", names=("review", "retired"))
    desired = make_bundle(tmp_path / "desired")
    target = Target.directory(tmp_path / "skills")
    materialize(old, target, receipt=False)
    untracked = inspect(desired, [target])[0].observation

    assert untracked.content_matches is True
    assert [(conflict.path, conflict.owner) for conflict in untracked.conflicts] == [
        ("review", None)
    ]
    materialize(old, target)
    owned = inspect(desired, [target])[0].observation
    assert owned.content_matches is False
    assert owned.recorded_content_matches is False
    assert owned.modifications == ()
    assert owned.conflicts == ()


def test_all_receipts_are_read_and_owned_content_is_checked(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    foreign = make_bundle(tmp_path / "foreign", identifier="foreign", names=("other",))
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    state = materialize(foreign, target)
    (target.root / "other/run").write_bytes(b"foreign edit")
    observed = inspect(bundle, [target])[0].observation

    assert observed.content_matches is True
    assert [item.bundle_id for item in observed.installations] == ["foreign", "team"]
    assert [(item.bundle_id, item.path, item.kind) for item in observed.modifications] == [
        ("foreign", "other/run", ModificationKind.CONTENT_CHANGED)
    ]
    (state / "receipts/foreign.json").write_bytes(b"broken")
    broken = inspect(bundle, [target])[0].observation
    assert broken.state is ObservationState.UNKNOWN
    assert broken.error is not None
    assert broken.error.code is ErrorCode.INVALID_STATE


def test_tombstone_has_no_ownership_and_lingering_files_are_untracked(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    (state / "receipts/team.json").write_bytes(
        receipt_bytes(Receipt("team", "0" * 32, None, None, ()))
    )
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.ABSENT
    assert observed.installed is None
    assert observed.installations == ()
    assert observed.conflicts[0].owner is None
    shutil.rmtree(target.root)
    assert inspect(bundle, [target])[0].observation.conflicts == ()


@pytest.mark.parametrize("pending", ["transaction.json", "staging", "backup"])
def test_inspection_reports_recovery_without_writing_or_recovering(
    tmp_path: Path, pending: str
) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    path = state / pending
    if pending.endswith(".json"):
        path.write_bytes(b"incomplete transaction record")
    else:
        path.mkdir()
        (path / "recoverable").write_bytes(b"keep")
    before = snapshot(tmp_path)
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.RECOVERY_NEEDED
    assert observed.error is not None
    assert observed.error.code is ErrorCode.RECOVERY_NEEDED
    assert observed.recovery_paths == (path,)
    assert observed.content_matches is True
    assert snapshot(tmp_path) == before


@pytest.mark.skipif(os.name == "nt", reason="POSIX filesystem permissions")
def test_unreadable_transaction_record_still_reports_recovery(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    path = state / "transaction.json"
    path.write_bytes(b"incomplete transaction record")
    mode = path.stat().st_mode
    path.chmod(0)
    try:
        if os.access(path, os.R_OK):
            pytest.skip("current user bypasses filesystem permission checks")
        observed = inspect(bundle, [target])[0].observation

        assert observed.state is ObservationState.RECOVERY_NEEDED
        assert observed.error is not None
        assert observed.error.code is ErrorCode.RECOVERY_NEEDED
        assert observed.error.path == path
        assert observed.recovery_paths == (path,)
        assert observed.installed is not None
        assert observed.installed.bundle_id == bundle.id
        assert observed.content_matches is True
        assert path.stat().st_mode & 0o777 == 0
    finally:
        path.chmod(mode)
    assert path.read_bytes() == b"incomplete transaction record"


def test_transaction_record_is_rechecked_for_concurrent_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from flyrail._observation import Observer

    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    path = state / "transaction.json"
    path.write_bytes(b"pending")
    finish = Observer.finish

    def change_before_finish(observer: Observer) -> None:
        if path in observer.observed:
            path.write_bytes(b"changed pending record")
        finish(observer)

    monkeypatch.setattr(Observer, "finish", change_before_finish)
    observed = inspect(bundle, [target])[0].observation

    assert path.read_bytes() == b"changed pending record"
    assert observed.state is ObservationState.UNKNOWN
    assert observed.error is not None
    assert observed.error.code is ErrorCode.CONCURRENT_CHANGE
    assert observed.error.path == path


def test_invalid_receipt_fails_closed_with_transaction_record(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    (state / "transaction.json").write_bytes(b"pending")
    receipt = state / "receipts/team.json"
    receipt.write_bytes(b"invalid receipt")
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.UNKNOWN
    assert observed.error is not None
    assert observed.error.code is ErrorCode.INVALID_STATE
    assert observed.error.path == receipt


@pytest.mark.parametrize(
    "bad",
    [
        "unknown",
        "receipt-extension",
        "receipt-name",
        "receipt-directory",
        "lock-directory",
        "transaction-directory",
        "future",
        "contradiction",
        "digest",
        "receipts-file",
        "state-file",
    ],
)
def test_malformed_or_contradictory_management_state_fails_closed(tmp_path: Path, bad: str) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = materialize(bundle, target)
    path = state / "receipts/team.json"
    if bad == "unknown":
        (state / "unknown").touch()
    elif bad == "receipt-extension":
        (state / "receipts/extra.txt").touch()
    elif bad == "receipt-name":
        path.rename(state / "receipts/TEAM.json")
    elif bad == "receipt-directory":
        path.unlink()
        path.mkdir()
    elif bad == "lock-directory":
        (state / "lock").mkdir()
    elif bad == "transaction-directory":
        (state / "transaction.json").mkdir()
    elif bad == "future":
        raw = json.loads(path.read_bytes())
        raw["schema_version"] = 2
        path.write_text(json.dumps(raw))
    elif bad == "contradiction":
        raw = json.loads(path.read_bytes())
        raw.update(bundle_id="other", transaction_id="f" * 32)
        (state / "receipts/other.json").write_text(json.dumps(raw))
        shutil.rmtree(target.root)
    elif bad == "digest":
        raw = json.loads(path.read_bytes())
        raw["content_digest"] = "f" * 64
        path.write_text(json.dumps(raw))
    elif bad == "receipts-file":
        shutil.rmtree(state / "receipts")
        (state / "receipts").touch()
    else:
        shutil.rmtree(state)
        state.touch()
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.UNKNOWN
    assert observed.error is not None
    assert observed.error.code is (
        ErrorCode.UNSAFE_PATH if bad in {"receipts-file", "state-file"} else ErrorCode.INVALID_STATE
    )


def test_order_alias_attribution_and_physical_deduplication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = make_bundle(tmp_path / "source")
    home = tmp_path / "home"
    original = Target.user("codex", home=home)
    materialize(bundle, original)
    alternate = tmp_path / "alternate"
    alternate.symlink_to(home, target_is_directory=True)
    alias = Target.directory(alternate / ".agents/skills")
    separate = Target.directory(tmp_path / "separate")
    reads: list[Path] = []
    original_open = Path.open

    def track_open(self: Path, mode: str = "r") -> object:
        reads.append(self)
        return original_open(self, mode)

    monkeypatch.setattr(Path, "open", track_open)
    results = inspect(bundle, [original, separate, alias, original])

    assert [item.target for item in results] == [original, separate, alias, original]
    assert [item.alias_of for item in results] == [None, None, 0, 0]
    assert results[0].observation is results[2].observation is results[3].observation
    assert results[0].root == results[2].root
    assert results[1].observation.state is ObservationState.ABSENT
    assert reads.count(original.root / "review/SKILL.md") == 1


def test_symlink_parent_traversal_keeps_os_path_semantics(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    nested = tmp_path / "physical/nested"
    nested.mkdir(parents=True)
    link = tmp_path / "link"
    link.symlink_to(nested, target_is_directory=True)
    target = Target.directory(link / "../skills")
    actual = Target.directory(tmp_path / "physical/skills")
    materialize(bundle, actual)
    results = inspect(bundle, [target, actual])

    assert results[0].root == actual.root
    assert results[0].observation.content_matches is True
    assert results[1].alias_of == 0


def test_symlink_loop_resolution_does_not_abort_other_targets(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    loop = tmp_path / "loop"
    loop.symlink_to(loop, target_is_directory=True)
    blocked = Target.directory(tmp_path / "missing/../loop/skills")
    healthy = Target.directory(tmp_path / "healthy")
    materialize(bundle, healthy)
    results = inspect(bundle, [blocked, healthy])

    assert results[0].target is blocked
    assert results[0].observation.state is ObservationState.UNKNOWN
    assert results[0].observation.error is not None
    assert results[0].observation.error.code in {ErrorCode.UNSAFE_PATH, ErrorCode.IO_ERROR}
    assert results[0].observation.error.path is not None
    assert results[1].target is healthy
    assert results[1].observation.is_current is True


@pytest.mark.parametrize(
    "placement", ["root", "preset", "skill", "file", "state", "receipt", "special"]
)
def test_managed_symlinks_and_special_files_are_refused(tmp_path: Path, placement: str) -> None:
    if placement == "special" and os.name == "nt":
        pytest.skip("POSIX FIFO")
    bundle = make_bundle(tmp_path / "source")
    target = Target.user("claude", home=tmp_path / "home")
    state = materialize(bundle, target)
    paths = {
        "root": target.root,
        "preset": target.root.parent,
        "skill": target.root / "review",
        "file": target.root / "review/SKILL.md",
        "state": state,
        "receipt": state / "receipts/team.json",
        "special": target.root / "review/run",
    }
    selected = paths[placement]
    destination = tmp_path / "moved"
    selected.rename(destination)
    if placement == "special":
        os.mkfifo(selected)
    else:
        selected.symlink_to(destination, target_is_directory=destination.is_dir())
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.UNKNOWN
    assert observed.error is not None
    assert observed.error.code is ErrorCode.UNSAFE_PATH
    assert observed.error.path == selected


def test_case_aliases_in_owned_trees(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    (target.root / "review").rename(target.root / "Review")
    observed = inspect(bundle, [target])[0].observation

    assert observed.conflicts[0].path == "Review"
    assert observed.conflicts[0].owner == "team"
    assert observed.content_matches is False
    assert {change.kind for change in observed.modifications} == {
        ModificationKind.MISSING,
        ModificationKind.ADDED,
    }


def test_portable_collisions_in_owned_trees(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    skill = target.root / "review"
    (skill / "same").mkdir()
    original_iterdir = Path.iterdir

    def colliding_entries(path: Path) -> object:
        children = list(original_iterdir(path))
        return iter([*children, skill / "SAME"] if path == skill else children)

    monkeypatch.setattr(Path, "iterdir", colliding_entries)
    collision = inspect(bundle, [target])[0].observation
    assert collision.error is not None
    assert collision.error.code is ErrorCode.UNSAFE_PATH


def test_nonportable_added_filename_is_refused(tmp_path: Path) -> None:
    if os.name == "nt":
        pytest.skip("Windows rejects this filename itself")
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    (target.root / "review/invalid?").touch()

    observed = inspect(bundle, [target])[0].observation
    assert observed.error is not None
    assert observed.error.code is ErrorCode.UNSAFE_PATH


def test_inputs_and_every_overlap_are_rejected_upfront(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    before = snapshot(tmp_path)
    with pytest.raises(TypeError, match="bundle must"):
        inspect("bundle", [target])  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="only Target"):
        inspect(bundle, [target, "bad"])  # type: ignore[list-item]
    with pytest.raises(ValueError, match="at least one"):
        inspect(bundle, [])
    for root in (tmp_path, tmp_path / "source", tmp_path / "source/skills"):
        with pytest.raises(ValueError, match="source, target and state"):
            inspect(bundle, [target, Target.directory(root)])
    for root in (
        target.root / "nested",
        target.root.parent / ".skills.flyrail",
        target.root.parent / ".skills.flyrail/nested",
        target.root / "nested/.inner.flyrail",
    ):
        with pytest.raises(ValueError, match="other destinations"):
            inspect(bundle, [target, Target.directory(root)])
    source_in_state = make_bundle(tmp_path / ".container.flyrail")
    with pytest.raises(ValueError, match="source, target and state"):
        inspect(source_in_state, [Target.directory(tmp_path / "container")])
    assert snapshot(tmp_path / "source") == {
        path.removeprefix("source/"): value
        for path, value in before.items()
        if path.startswith("source/")
    }


@pytest.mark.skipif(os.name == "nt", reason="POSIX filesystem permissions")
@pytest.mark.parametrize("placement", ["parent", "target", "file", "state", "receipt"])
def test_actual_permission_errors_are_per_target(tmp_path: Path, placement: str) -> None:
    bundle = make_bundle(tmp_path / "source")
    blocked = Target.directory(tmp_path / "denied/skills")
    healthy = Target.directory(tmp_path / "healthy")
    state = materialize(bundle, blocked)
    materialize(bundle, healthy)
    selected = {
        "parent": blocked.root.parent,
        "target": blocked.root,
        "file": blocked.root / "review/SKILL.md",
        "state": state,
        "receipt": state / "receipts/team.json",
    }[placement]
    mode = selected.stat().st_mode
    selected.chmod(0)
    try:
        if os.access(selected, os.R_OK):
            pytest.skip("current user bypasses filesystem permission checks")
        results = inspect(bundle, [blocked, healthy])
        assert results[0].observation.state is ObservationState.UNKNOWN
        assert results[0].observation.error is not None
        assert results[0].observation.error.code is ErrorCode.IO_ERROR
        assert results[0].observation.error.errno is not None
        assert results[1].observation.content_matches is True
    finally:
        selected.chmod(mode)


def test_currentness_requires_owned_unchanged_matching_revision(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    assert inspect(bundle, [target])[0].observation.is_current is False
    materialize(bundle, target, receipt=False)
    assert inspect(bundle, [target])[0].observation.is_current is False
    materialize(bundle, target)
    assert inspect(bundle, [target])[0].observation.is_current is True
    foreign = make_bundle(tmp_path / "foreign", identifier="foreign", names=("other",))
    materialize(foreign, target)
    (target.root / "other/SKILL.md").write_bytes(b"other edit")
    assert inspect(bundle, [target])[0].observation.is_current is True
    (target.root / "review/SKILL.md").write_bytes(b"edited")
    assert inspect(bundle, [target])[0].observation.is_current is False
    desired = make_bundle(tmp_path / "desired", data=b"edited")
    assert inspect(desired, [target])[0].observation.is_current is False


def test_detected_concurrent_edit_is_an_observation_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    selected = target.root / "review/SKILL.md"
    original_open = Path.open
    changed = False

    def change_before_open(path: Path, mode: str = "r", buffering: int = -1) -> object:
        nonlocal changed
        if path == selected and not changed:
            changed = True
            with original_open(path, "wb") as stream:
                stream.write(b"concurrent modification")
        return original_open(path, mode, buffering=buffering)

    monkeypatch.setattr(Path, "open", change_before_open)
    observed = inspect(bundle, [target])[0].observation

    assert selected.read_bytes() == b"concurrent modification"
    assert observed.state is ObservationState.UNKNOWN
    assert observed.error is not None
    assert observed.error.code is ErrorCode.CONCURRENT_CHANGE


def test_missing_skill_appearing_during_inspection_is_reported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from flyrail._observation import Observer

    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    target.root.mkdir()
    finish = Observer.finish
    calls = 0

    def create_before_finish(observer: Observer) -> None:
        nonlocal calls
        calls += 1
        if calls == 2:
            (target.root / "review").mkdir()
        finish(observer)

    monkeypatch.setattr(Observer, "finish", create_before_finish)
    observed = inspect(bundle, [target])[0].observation

    assert (target.root / "review").is_dir()
    assert observed.error is not None
    assert observed.error.code is ErrorCode.CONCURRENT_CHANGE


def test_existing_file_target_is_a_per_target_error(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    file = tmp_path / "file"
    file.touch()
    result = inspect(bundle, [Target.directory(file)])[0]

    assert result.observation.state is ObservationState.UNKNOWN
    assert result.observation.error is not None
    assert result.observation.error.code is ErrorCode.UNSAFE_PATH


@pytest.mark.parametrize("directory", ["preset", "state"])
def test_managed_directory_spelling_mismatches_are_refused(tmp_path: Path, directory: str) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.user("claude", home=tmp_path / "home")
    state = materialize(bundle, target)
    selected = target.root.parent if directory == "preset" else state
    selected.rename(selected.with_name(selected.name.upper()))
    observed = inspect(bundle, [target])[0].observation

    assert observed.state is ObservationState.UNKNOWN
    assert observed.error is not None
    assert observed.error.code is ErrorCode.UNSAFE_PATH
    assert "spelling" in observed.error.message


def test_existing_case_insensitive_explicit_target_alias_uses_same_sidecar(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    materialize(bundle, target)
    alias = Target.directory(tmp_path / "SKILLS")
    if not alias.root.exists():
        with pytest.raises(ValueError, match="other destinations"):
            inspect(bundle, [target, alias])
        return
    results = inspect(bundle, [alias, target])

    assert results[0].observation.is_current is True
    assert results[0].state_root == results[1].state_root
    assert results[1].alias_of == 0


def test_excessively_nested_receipt_does_not_abort_other_targets(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    malformed = Target.directory(tmp_path / "malformed")
    healthy = Target.directory(tmp_path / "healthy")
    state = materialize(bundle, malformed)
    materialize(bundle, healthy)
    (state / "receipts/team.json").write_bytes(b"[" * 2000 + b"0" + b"]" * 2000)
    results = inspect(bundle, [malformed, healthy])

    assert results[0].observation.error is not None
    assert results[0].observation.error.code is ErrorCode.INVALID_STATE
    assert results[1].observation.is_current is True
