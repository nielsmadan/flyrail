import os
import shutil
import stat
import subprocess
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from types import SimpleNamespace

import pytest
from skill_helpers import make_bundle

from flyrail import (
    Bundle,
    BundleEntry,
    BundleIdentity,
    ErrorCode,
    OperationStatus,
    SkillArtifact,
    Target,
    TreeContent,
    inspect,
    install,
    uninstall,
    update,
)
from flyrail import _observation as observation
from flyrail import _resource_io as io
from flyrail import _security as security
from flyrail import _sources as sources
from flyrail._observation import ObservationFailure, Observer
from flyrail._sources import DirectorySource

pytestmark = pytest.mark.integration


def after_security_sample(
    monkeypatch: pytest.MonkeyPatch, parent: Path, change: Callable[[], None]
) -> None:
    original = security.security

    def sample(path: Path, *, ancestor: bool = False) -> bytes:
        result = original(path, ancestor=ancestor)
        if ancestor and path == parent:
            change()
        return result

    monkeypatch.setattr(observation, "security", sample)
    monkeypatch.setattr(io, "security", sample)


@pytest.mark.parametrize("case_alias", [False, True])
@pytest.mark.parametrize("seam", ["ancestor", "filtered-lookup"])
def test_skill_lifecycle_accepts_unrelated_sibling_creation_and_removal_during_inspection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case_alias: bool, seam: str
) -> None:
    parent = tmp_path / "Config"
    parent.mkdir()
    alias = tmp_path / "config" if case_alias else parent
    if not alias.is_dir():
        pytest.skip("host is case sensitive")
    original = Target.directory(parent / "skills")
    equivalent = Target.directory(alias / "skills")
    sibling = parent / "unrelated"
    changes = 0

    def change() -> None:
        nonlocal changes
        if changes % 2:
            sibling.unlink()
        else:
            sibling.write_bytes(b"other writer")
        changes += 1

    if seam == "ancestor":
        after_security_sample(monkeypatch, parent, change)
    else:
        original_directory = Observer.directory

        def directory(
            observer: Observer, path: Path, *, matching: str | None = None
        ) -> tuple[Path, ...]:
            children = original_directory(observer, path, matching=matching)
            if path == parent and matching is not None:
                change()
            return children

        monkeypatch.setattr(Observer, "directory", directory)

    def skill(version: str) -> Bundle:
        return Bundle.from_artifacts(
            BundleIdentity("noise", version),
            [
                SkillArtifact(
                    "skill", "example", TreeContent([BundleEntry("SKILL.md", version.encode())])
                )
            ],
        )

    assert install(skill("1"), [original])[0].status is OperationStatus.APPLIED
    previous = changes
    assert previous >= 2
    assert inspect(skill("1"), [equivalent])[0].observation.is_current
    assert changes > previous
    previous = changes
    assert install(skill("1"), [equivalent, original])[0].status is OperationStatus.UNCHANGED
    assert changes > previous
    previous = changes
    assert update(skill("2"), [equivalent])[0].status is OperationStatus.APPLIED
    assert (parent / "skills" / "example" / "SKILL.md").read_bytes() == b"2"
    assert changes > previous
    previous = changes
    assert uninstall("noise", [equivalent])[0].status is OperationStatus.APPLIED
    assert changes > previous
    assert not (parent / "skills" / "example").exists()


@pytest.mark.parametrize("change", ["identity", "mode", "type", "xattr", "acl"])
def test_ancestor_sampling_rejects_identity_and_native_security_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    if change == "mode" and os.name == "nt":
        pytest.skip("POSIX directory mode")
    if change == "xattr" and sys.platform not in {"darwin", "linux"}:
        pytest.skip("native macOS or Linux extended attributes")
    if change == "acl" and sys.platform != "darwin":
        pytest.skip("native macOS ACL")
    parent = tmp_path / "parent"
    parent.mkdir(mode=0o755)
    changed = False

    def mutate() -> None:
        nonlocal changed
        if changed:
            return
        changed = True
        if change in {"identity", "type"}:
            parent.rename(tmp_path / "original")
            if change == "identity":
                parent.mkdir(mode=0o755)
            else:
                parent.write_bytes(b"replacement")
        elif change == "mode":
            parent.chmod(0o700)
        elif change == "acl":
            executable = shutil.which("chmod")
            assert executable is not None
            subprocess.run(  # noqa: S603
                [executable, "+a", "everyone allow read", str(parent)],
                check=True,
                capture_output=True,
            )
        elif sys.platform == "darwin":
            executable = shutil.which("xattr")
            assert executable is not None
            subprocess.run(  # noqa: S603
                [executable, "-w", "flyrail.ancestor", "changed", str(parent)],
                check=True,
                capture_output=True,
            )
        elif sys.platform == "linux":
            os.setxattr(parent, "user.flyrail", b"changed")

    after_security_sample(monkeypatch, parent, mutate)
    with pytest.raises(ObservationFailure) as caught:
        io.ancestors(parent / "resource")
    assert changed
    assert caught.value.error.path == parent
    assert caught.value.error.code is (
        ErrorCode.UNSAFE_PATH if change == "type" else ErrorCode.CONCURRENT_CHANGE
    )


@pytest.mark.parametrize("change", ["uid", "gid", "reparse", "descriptor"])
def test_ancestor_sampling_binds_owner_group_reparse_and_security_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    parent = tmp_path / "parent"
    parent.mkdir()
    sampled = False
    original = Path.lstat
    original_security = security.security

    def metadata(path: Path) -> os.stat_result:
        result = original(path)
        if path == parent and sampled:
            if change == "reparse":
                return SimpleNamespace(  # type: ignore[return-value]
                    st_mode=result.st_mode, st_file_attributes=stat.FILE_ATTRIBUTE_REPARSE_POINT
                )
            if change in {"uid", "gid"}:
                return SimpleNamespace(  # type: ignore[return-value]
                    st_mode=result.st_mode,
                    st_file_attributes=getattr(result, "st_file_attributes", 0),
                    st_dev=result.st_dev,
                    st_ino=result.st_ino,
                    st_uid=result.st_uid + (change == "uid"),
                    st_gid=result.st_gid + (change == "gid"),
                )
        return result

    def descriptor(path: Path, *, ancestor: bool = False) -> bytes:
        nonlocal sampled
        value = original_security(path, ancestor=ancestor)
        if ancestor and path == parent:
            value = value + b"changed" if sampled and change == "descriptor" else value
            sampled = True
        return value

    monkeypatch.setattr(Path, "lstat", metadata)
    monkeypatch.setattr(observation, "security", descriptor)
    with pytest.raises(ObservationFailure) as caught:
        io.ancestors(parent / "resource")
    assert sampled
    assert caught.value.error.code is (
        ErrorCode.UNSAFE_PATH if change == "reparse" else ErrorCode.CONCURRENT_CHANGE
    )


@pytest.mark.parametrize("existing", [False, True])
def test_ancestor_sampling_rejects_appearance_and_disappearance(
    tmp_path: Path, existing: bool
) -> None:
    parent = tmp_path / "parent"
    if existing:
        parent.mkdir()
    observer = Observer()
    observer.ancestor(parent)
    if existing:
        parent.rmdir()
    else:
        parent.mkdir()
    with pytest.raises(ObservationFailure) as caught:
        observer.finish()
    assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE


@pytest.mark.parametrize("change", ["added", "removed", "alias", "identity"])
def test_filtered_directory_lookup_rechecks_selected_membership_and_identity(
    tmp_path: Path, change: str
) -> None:
    selected = tmp_path / "state"
    if change != "added":
        selected.mkdir()
    observer = Observer()
    observer.managed_directory(selected)
    if change == "added":
        selected.mkdir()
    elif change == "removed":
        selected.rmdir()
    elif change == "alias":
        selected.rename(tmp_path / "temporary")
        (tmp_path / "temporary").rename(tmp_path / "STATE")
    else:
        selected.rename(tmp_path / "original")
        selected.mkdir()
    with pytest.raises(ObservationFailure) as caught:
        observer.finish()
    assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE


@pytest.mark.parametrize("change", ["added", "removed", "alias"])
def test_filtered_name_snapshot_rejects_membership_changes(tmp_path: Path, change: str) -> None:
    selected = tmp_path / "state"
    if change != "added":
        selected.mkdir()
    observer = Observer()
    assert observer.directory(tmp_path, matching="state") == (
        () if change == "added" else (selected,)
    )
    if change == "added":
        selected.mkdir()
    elif change == "removed":
        selected.rmdir()
    else:
        selected.rename(tmp_path / "temporary")
        (tmp_path / "temporary").rename(tmp_path / "STATE")
    with pytest.raises(ObservationFailure) as caught:
        observer.finish()
    assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE
    assert caught.value.error.message == "selected directory entries changed"


@pytest.mark.parametrize("source", [False, True])
@pytest.mark.parametrize("change", ["added", "removed"])
def test_complete_directory_observation_still_rejects_membership_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, source: bool, change: str
) -> None:
    tree = tmp_path / "source"
    make_bundle(tree)
    selected = tree / "review"
    sibling = selected / "unrelated"
    if change == "removed":
        sibling.write_bytes(b"foreign")
    original = Path.iterdir
    changed = False

    def children(path: Path) -> Iterator[Path]:
        nonlocal changed
        values = tuple(original(path))
        yield from values
        if path == selected and not changed:
            changed = True
            if change == "added":
                sibling.write_bytes(b"foreign")
            else:
                sibling.unlink()

    monkeypatch.setattr(Path, "iterdir", children)
    if source:
        with pytest.raises(ValueError, match="source changed while loading"):
            Bundle.from_directory(tree)
    else:
        with pytest.raises(ObservationFailure) as caught:
            io.observe(selected)
        assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE
    assert changed


def test_source_final_snapshot_still_rejects_directory_membership_changes(tmp_path: Path) -> None:
    source = DirectorySource(tmp_path)
    (tmp_path / "added").write_bytes(b"foreign")
    with pytest.raises(ValueError, match="source changed while loading"):
        source.finish()


@pytest.mark.parametrize("descriptor_changed", [False, True])
def test_windows_file_observation_compares_ctime_only_within_stat_channels(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, descriptor_changed: bool
) -> None:
    path = tmp_path / "file"
    path.write_bytes(b"content")
    original = os.fstat
    samples = 0

    def descriptor(file_descriptor: int) -> SimpleNamespace:
        nonlocal samples
        metadata = original(file_descriptor)
        samples += 1
        return SimpleNamespace(
            st_dev=metadata.st_dev,
            st_ino=metadata.st_ino,
            st_mode=metadata.st_mode,
            st_size=metadata.st_size,
            st_mtime_ns=metadata.st_mtime_ns,
            st_ctime_ns=metadata.st_ctime_ns + 1 + (samples > 1 and descriptor_changed),
        )

    monkeypatch.setattr(sources, "_WINDOWS", True)
    monkeypatch.setattr(os, "fstat", descriptor)
    if descriptor_changed:
        with pytest.raises(ObservationFailure) as caught:
            Observer().file(path)
        assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE
        assert caught.value.error.message == "file changed while reading"
    else:
        assert Observer().file(path) == b"content"


@pytest.mark.parametrize("source", [False, True])
def test_listed_file_disappearance_is_a_concurrent_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, source: bool
) -> None:
    tree = tmp_path / "source"
    make_bundle(tree)
    selected = tree / "review"
    disappearing = selected / "SKILL.md"
    original = Path.iterdir

    def children(path: Path) -> Iterator[Path]:
        values = tuple(original(path))
        if path == selected:
            disappearing.unlink()
        yield from values

    monkeypatch.setattr(Path, "iterdir", children)
    if source:
        with pytest.raises(ValueError, match="source changed while loading"):
            Bundle.from_directory(tree)
    else:
        with pytest.raises(ObservationFailure) as caught:
            io.observe(selected)
        assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE
