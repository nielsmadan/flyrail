import hashlib
import importlib
import json
import os
import shutil
import stat
import subprocess
import sys
import zipfile
from collections.abc import Iterator
from dataclasses import FrozenInstanceError
from pathlib import Path
from typing import NotRequired, TypedDict, cast

import pytest
from conformance import FIXTURES

from flyrail import Bundle, BundleEntry, BundleIdentity, SkillSpec
from flyrail._sources import DirectorySource
from flyrail.bundle import _content_digest


class FixtureFile(TypedDict):
    path: str
    data_hex: str


class FixtureEntry(TypedDict):
    path: str
    data_hex: str | None
    executable: bool


class BundleCase(TypedDict):
    name: str
    manifest: dict[str, object]
    directories: list[str]
    files: list[FixtureFile]
    entries: list[FixtureEntry]
    content_digest: str
    framing_hex: NotRequired[str]


CASES = cast(
    list[BundleCase],
    json.loads((FIXTURES / "bundles.json").read_text())["cases"],
)
PACKAGE = "flyrail_fixture_pkg"


@pytest.fixture(autouse=True)
def clean_package_imports() -> Iterator[None]:
    yield
    sys.modules.pop(PACKAGE, None)
    importlib.invalidate_caches()


def write_bundle(root: Path, manifest: dict[str, object] | None = None) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    if manifest is None:
        manifest = {
            "schema_version": 1,
            "id": "team",
            "version": "1",
            "skills": [{"name": "review", "path": "skills/review"}],
        }
    (root / "flyrail.json").write_text(json.dumps(manifest), encoding="utf-8")
    skill = root / "skills/review"
    skill.mkdir(parents=True, exist_ok=True)
    (skill / "SKILL.md").write_bytes(b"---\r\ninvalid yaml: [\r\n---\r\n\xff\x00")
    return skill


def write_case(root: Path, case: BundleCase) -> None:
    root.mkdir(parents=True)
    (root / "flyrail.json").write_text(json.dumps(case["manifest"]), encoding="utf-8")
    for directory in case["directories"]:
        (root / directory).mkdir(parents=True, exist_ok=True)
    for file in case["files"]:
        path = root / file["path"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(bytes.fromhex(file["data_hex"]))


def make_package(parent: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    root = parent / PACKAGE
    root.mkdir(parents=True)
    (root / "__init__.py").write_text("", encoding="utf-8")
    monkeypatch.syspath_prepend(str(parent))
    return root


def zip_package(parent: Path, destination: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    with zipfile.ZipFile(destination, "w") as archive:
        for path in sorted(parent.rglob("*")):
            archive.write(path, path.relative_to(parent).as_posix())
    monkeypatch.syspath_prepend(str(destination))


@pytest.mark.parametrize("case", CASES, ids=lambda case: case["name"])
@pytest.mark.parametrize("source_kind", ["directory", "package", "zip"])
def test_shared_conformance_vectors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case: BundleCase, source_kind: str
) -> None:
    parent = tmp_path / "source"
    package = make_package(parent, monkeypatch)
    root = package / "flyrail"
    write_case(root, case)

    if source_kind == "directory":
        bundle = Bundle.from_directory(root)
        assert bundle.source_roots == (root.resolve(),)
    elif source_kind == "package":
        bundle = Bundle.from_package(PACKAGE)
        assert bundle.source_roots == (root.resolve(),)
    else:
        archive = tmp_path / "package.zip"
        zip_package(parent, archive, monkeypatch)
        bundle = Bundle.from_package(PACKAGE)
        assert bundle.source_roots == (archive.resolve(),)
        archive.unlink()
    shutil.rmtree(parent)

    assert bundle.identity == BundleIdentity(
        cast(str, case["manifest"]["id"]), cast(str, case["manifest"]["version"])
    )
    assert bundle.content_digest == case["content_digest"]
    assert bundle.entries == tuple(
        BundleEntry(
            entry["path"],
            None if entry["data_hex"] is None else bytes.fromhex(entry["data_hex"]),
            entry["executable"],
        )
        for entry in case["entries"]
    )
    if "framing_hex" in case:
        assert (
            hashlib.sha256(bytes.fromhex(case["framing_hex"])).hexdigest() == case["content_digest"]
        )


def test_snapshot_survives_mutation_and_is_immutable(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path)
    (skill / "empty").mkdir()
    bundle = Bundle.from_directory(str(tmp_path))
    original = (skill / "SKILL.md").read_bytes()
    (skill / "SKILL.md").write_bytes(b"changed")
    (skill / "empty").rmdir()
    (skill / "later.bin").write_bytes(b"later")
    (tmp_path / "flyrail.json").write_text("{}", encoding="utf-8")

    assert bundle.id == "team"
    assert bundle.version == "1"
    assert bundle.skills == (SkillSpec("review", "skills/review"),)
    assert bundle.entries == (
        BundleEntry("review"),
        BundleEntry("review/SKILL.md", original),
        BundleEntry("review/empty"),
    )
    assert _content_digest(bundle.entries) == bundle.content_digest
    assert {bundle: "snapshot"}[bundle] == "snapshot"
    with pytest.raises(FrozenInstanceError):
        bundle.content_digest = "other"  # type: ignore[misc]
    with pytest.raises(FrozenInstanceError):
        bundle.entries[1].data = b"other"  # type: ignore[misc]
    with pytest.raises(TypeError, match="from_directory"):
        Bundle()


def test_version_and_source_location_are_separate_from_content(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path)
    first = Bundle.from_directory(tmp_path)
    relocated = tmp_path / "other/review"
    relocated.parent.mkdir()
    skill.rename(relocated)
    manifest = json.loads((tmp_path / "flyrail.json").read_text())
    manifest.update(id="renamed", version="  older-looking  ")
    manifest["skills"][0]["path"] = "other/review"
    (tmp_path / "flyrail.json").write_text(json.dumps(manifest), encoding="utf-8")
    second = Bundle.from_directory(tmp_path)

    assert second.version == "  older-looking  "
    assert second.id == "renamed"
    assert second.content_digest == first.content_digest
    assert second.entries == first.entries
    (relocated / "SKILL.md").write_bytes(b"new content with same version")
    third = Bundle.from_directory(tmp_path)
    assert third.version == second.version
    assert third.content_digest != second.content_digest


@pytest.mark.parametrize("version", ["秋", "  cafe\u0301 \U0001f680  ", "\ud800", "release\udfff"])
def test_manifest_version_unicode_validation(tmp_path: Path, version: str) -> None:
    write_bundle(tmp_path)
    path = tmp_path / "flyrail.json"
    manifest = json.loads(path.read_text(encoding="utf-8"))
    manifest["version"] = version
    path.write_text(json.dumps(manifest), encoding="utf-8")

    if "\ud800" in version or "\udfff" in version:
        with pytest.raises(ValueError, match="version must contain only Unicode scalar values"):
            Bundle.from_directory(tmp_path)
    else:
        assert Bundle.from_directory(tmp_path).version == version


def test_digest_tracks_empty_directories_and_explicit_executable_intent(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path)
    initial = Bundle.from_directory(tmp_path)
    (skill / "empty").mkdir()
    with_directory = Bundle.from_directory(tmp_path)
    assert with_directory.content_digest != initial.content_digest
    manifest = json.loads((tmp_path / "flyrail.json").read_text())
    manifest["skills"][0]["executables"] = ["SKILL.md"]
    (tmp_path / "flyrail.json").write_text(json.dumps(manifest), encoding="utf-8")
    executable = Bundle.from_directory(tmp_path)
    assert executable.content_digest != with_directory.content_digest
    assert executable.entries[1].executable is True
    (skill / "SKILL.md").chmod(stat.S_IRUSR | stat.S_IWUSR)
    assert Bundle.from_directory(tmp_path).content_digest == executable.content_digest


@pytest.mark.skipif(os.name == "nt", reason="POSIX file executable bits")
def test_physical_executable_bits_do_not_supply_intent(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path)
    initial = Bundle.from_directory(tmp_path)
    (skill / "SKILL.md").chmod(stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR)
    after_chmod = Bundle.from_directory(tmp_path)

    assert after_chmod.entries[1].executable is False
    assert after_chmod.content_digest == initial.content_digest


def test_manifest_and_executable_order_do_not_change_digest(tmp_path: Path) -> None:
    case = CASES[2]
    write_case(tmp_path / "bundle", case)
    root = tmp_path / "bundle"
    manifest = json.loads((root / "flyrail.json").read_text())
    manifest["skills"][1]["executables"] = ["tools/run.sh", "SKILL.md"]
    (root / "flyrail.json").write_text(json.dumps(manifest))
    initial = Bundle.from_directory(root)
    manifest["skills"].reverse()
    for record in manifest["skills"]:
        record.get("executables", []).reverse()
    (root / "flyrail.json").write_text(json.dumps(manifest, sort_keys=True, indent=4))
    reordered = Bundle.from_directory(root)

    assert tuple(spec.name for spec in reordered.skills) == ("code-review", "notes")
    assert reordered.skills[0].executables == ("SKILL.md", "tools/run.sh")
    assert reordered.content_digest == initial.content_digest
    assert _content_digest(reversed(initial.entries)) == initial.content_digest
    notes = tuple(entry for entry in initial.entries if entry.path.split("/")[0] == "notes")
    only_notes = tmp_path / "only-notes"
    write_bundle(
        only_notes,
        {
            "schema_version": 1,
            "id": "notes",
            "version": "1",
            "skills": [{"name": "notes", "path": "notes"}],
        },
    )
    shutil.copytree(root / "notes", only_notes / "notes")
    assert _content_digest(notes) == Bundle.from_directory(only_notes).content_digest


@pytest.mark.parametrize(
    "field,value",
    [
        ("schema_version", True),
        ("schema_version", 1.0),
        ("schema_version", "1"),
        ("schema_version", 2),
        ("schema_version", None),
        ("id", 1),
        ("id", "Bad"),
        ("version", 1),
        ("version", " \n\t"),
        ("skills", {}),
        ("skills", []),
        ("skills", [None]),
        ("skills", [{"name": "review"}]),
        ("skills", [{"name": "review", "path": "review", "extra": None}]),
        ("skills", [{"name": "review", "path": "review", "executables": "run.py"}]),
        ("skills", [{"name": "review", "path": "review", "executables": [7]}]),
        ("skills", [{"name": "review", "path": "review", "executables": ["x", "X"]}]),
        ("skills", [{"name": "review", "path": "other"}]),
        ("unknown", 1),
    ],
)
def test_invalid_manifest_fields(tmp_path: Path, field: str, value: object) -> None:
    write_bundle(tmp_path)
    path = tmp_path / "flyrail.json"
    manifest = json.loads(path.read_text())
    manifest[field] = value
    path.write_text(json.dumps(manifest), encoding="utf-8")

    with pytest.raises(ValueError):
        Bundle.from_directory(tmp_path)


@pytest.mark.parametrize(
    "raw",
    [
        b"not json",
        b"[]",
        b"{}",
        b"\xff",
        b"\xef\xbb\xbf{}",
        b'{"schema_version":NaN}',
        b'{"schema_version":Infinity}',
        b'{"schema_version":-Infinity}',
    ],
)
def test_invalid_manifest_encoding_and_json(tmp_path: Path, raw: bytes) -> None:
    (tmp_path / "flyrail.json").write_bytes(raw)
    with pytest.raises(ValueError):
        Bundle.from_directory(tmp_path)


@pytest.mark.parametrize("field,value", [("id", "team"), ("name", "review")])
def test_duplicate_manifest_fields(tmp_path: Path, field: str, value: str) -> None:
    write_bundle(tmp_path)
    expected = Bundle.from_directory(tmp_path)
    path = tmp_path / "flyrail.json"
    original = path.read_text(encoding="utf-8")
    member = f'"{field}": "{value}"'
    assert original.count(member) == 1
    duplicated = original.replace(member, f"{member}, {member}")
    assert duplicated.count(member) == 2
    path.write_text(duplicated, encoding="utf-8")

    with pytest.raises(ValueError, match=f"^duplicate JSON field: {field}$"):
        Bundle.from_directory(tmp_path)

    path.write_text(original, encoding="utf-8")
    assert Bundle.from_directory(tmp_path).entries == expected.entries


@pytest.mark.parametrize(
    "skills",
    [
        [{"name": "review", "path": "one/review"}, {"name": "review", "path": "two/review"}],
        [{"name": "one", "path": "one"}, {"name": "two", "path": "one/two"}],
        [{"name": "two", "path": "ONE/two"}, {"name": "one", "path": "one"}],
    ],
)
def test_duplicate_or_overlapping_skills(tmp_path: Path, skills: list[dict[str, str]]) -> None:
    write_bundle(tmp_path, {"schema_version": 1, "id": "x", "version": "1", "skills": skills})
    with pytest.raises(ValueError, match=r"duplicate skill|must not overlap"):
        Bundle.from_directory(tmp_path)


@pytest.mark.parametrize("target", ["missing.py", "empty", "skill.md"])
def test_executables_must_name_exact_existing_files(tmp_path: Path, target: str) -> None:
    skill = write_bundle(
        tmp_path,
        {
            "schema_version": 1,
            "id": "x",
            "version": "1",
            "skills": [{"name": "review", "path": "skills/review", "executables": [target]}],
        },
    )
    (skill / "empty").mkdir()
    with pytest.raises(ValueError, match="existing file"):
        Bundle.from_directory(tmp_path)


@pytest.mark.parametrize("replacement", [None, "directory", "wrong-case"])
def test_skill_markdown_must_be_an_exact_regular_file(
    tmp_path: Path, replacement: str | None
) -> None:
    skill = write_bundle(tmp_path)
    (skill / "SKILL.md").unlink()
    if replacement == "directory":
        (skill / "SKILL.md").mkdir()
    elif replacement == "wrong-case":
        (skill / "skill.md").write_bytes(b"markdown")
    with pytest.raises(ValueError, match=r"SKILL\.md file"):
        Bundle.from_directory(tmp_path)


def test_missing_and_wrong_source_types(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        Bundle.from_directory(tmp_path / "absent")
    with pytest.raises(ValueError, match="missing"):
        Bundle.from_directory(tmp_path)
    manifest = tmp_path / "flyrail.json"
    manifest.mkdir()
    with pytest.raises(ValueError, match="expected a source file"):
        Bundle.from_directory(tmp_path)
    manifest.rmdir()
    manifest.write_bytes(b"{}")
    with pytest.raises(ValueError, match="root must be a directory"):
        Bundle.from_directory(manifest)
    skill = write_bundle(tmp_path)
    shutil.rmtree(skill)
    skill.write_bytes(b"not a directory")
    with pytest.raises(ValueError, match="skill source must be a directory"):
        Bundle.from_directory(tmp_path)


@pytest.mark.parametrize(
    "name", ["CON.txt", "trailing.", "bad\x7f", "bad\x85", "bad?", "cafe\u0301"]
)
def test_nonportable_actual_source_names(tmp_path: Path, name: str) -> None:
    skill = write_bundle(tmp_path)
    try:
        (skill / name).mkdir()
    except OSError:
        pytest.skip("host cannot create this nonportable filename; covered through ZIP fixtures")
    with pytest.raises(ValueError, match="source name"):
        Bundle.from_directory(tmp_path)


def test_actual_source_casing_must_match_manifest(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path)
    skill.parent.rename(tmp_path / "SKILLS")
    with pytest.raises(ValueError, match="case-mismatched"):
        Bundle.from_directory(tmp_path)


def test_empty_directory_portable_collisions(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path)
    (skill / "same").mkdir()
    try:
        (skill / "SAME").mkdir()
    except FileExistsError:
        pytest.skip("case-insensitive filesystem; covered through ZIP fixtures")
    with pytest.raises(ValueError, match="portable source name collision"):
        Bundle.from_directory(tmp_path)


@pytest.mark.parametrize("kind", ["root", "manifest", "ancestor", "skill", "file", "directory"])
def test_rejects_source_symlinks(tmp_path: Path, kind: str) -> None:
    root = tmp_path / "bundle"
    skill = write_bundle(root)
    external = tmp_path / "external"
    external.mkdir()
    (external / "SKILL.md").write_bytes(b"external")
    selected = {
        "root": root,
        "manifest": root / "flyrail.json",
        "ancestor": root / "skills",
        "skill": skill,
        "file": skill / "SKILL.md",
        "directory": skill / "linked",
    }[kind]
    is_directory = kind not in {"manifest", "file"}
    if selected.is_dir():
        shutil.rmtree(selected)
    elif selected.exists():
        selected.unlink()
    try:
        selected.symlink_to(
            external if is_directory else external / "SKILL.md", target_is_directory=is_directory
        )
    except OSError:
        pytest.skip("symlinks require host support or Windows Developer Mode")
    with pytest.raises(ValueError, match=r"symlinks|reparse"):
        Bundle.from_directory(root)


@pytest.mark.skipif(os.name == "nt", reason="POSIX FIFO")
def test_rejects_special_files_without_opening_them(tmp_path: Path) -> None:
    if sys.platform == "win32":
        pytest.skip("POSIX FIFO")
    skill = write_bundle(tmp_path)
    os.mkfifo(skill / "pipe")
    with pytest.raises(ValueError, match="special files"):
        Bundle.from_directory(tmp_path)


def test_package_module_and_custom_resource_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = make_package(tmp_path / "parent", monkeypatch)
    root = package / "data/custom"
    write_bundle(root)
    module = importlib.import_module(PACKAGE)

    assert Bundle.from_package(module, "data/custom").entries == Bundle.from_directory(root).entries


@pytest.mark.parametrize("resource", ["", ".", "../flyrail", "/flyrail", "data\\flyrail", "CON"])
def test_invalid_resource_paths(resource: str) -> None:
    with pytest.raises(ValueError, match="package resource"):
        Bundle.from_package(PACKAGE, resource)


def test_invalid_package_arguments() -> None:
    with pytest.raises(TypeError, match="package must"):
        Bundle.from_package(123)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="package resource"):
        Bundle.from_package(PACKAGE, 123)  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="not a module"):
        Bundle.from_package("json.encoder")
    with pytest.raises(ModuleNotFoundError):
        Bundle.from_package("flyrail_fixture_nonexistent_package")
    with pytest.raises(TypeError):
        Bundle.from_directory(123)  # type: ignore[arg-type]


def test_filesystem_namespace_package(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    package = make_package(tmp_path / "parent", monkeypatch)
    (package / "__init__.py").unlink()
    root = package / "flyrail"
    write_bundle(root)

    assert Bundle.from_package(PACKAGE).source_roots == (root.resolve(),)


def test_ambiguous_namespace_package(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    for name in ("first", "second"):
        package = make_package(tmp_path / name, monkeypatch)
        (package / "__init__.py").unlink()
        write_bundle(package / "flyrail")
    with pytest.raises(ValueError, match="multiple resource locations"):
        Bundle.from_package(PACKAGE)


def test_source_change_during_loading_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write_bundle(tmp_path)
    original = DirectorySource._read

    def read_and_change(source: DirectorySource, path: Path) -> bytes:
        data = original(source, path)
        if path.name == "SKILL.md":
            path.write_bytes(b"concurrent edit")
        return data

    monkeypatch.setattr(DirectorySource, "_read", read_and_change)
    with pytest.raises(ValueError, match="source changed while loading"):
        Bundle.from_directory(tmp_path)


def test_bundle_entry_rejects_invalid_mutable_values() -> None:
    with pytest.raises(TypeError, match="bytes or None"):
        BundleEntry("review/SKILL.md", bytearray(b"mutable"))  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="boolean"):
        BundleEntry("review/SKILL.md", b"", 1)  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="directory"):
        BundleEntry("review", executable=True)


def archive_members() -> list[tuple[str | zipfile.ZipInfo, bytes]]:
    return [
        (PACKAGE + "/__init__.py", b""),
        (PACKAGE + "/flyrail/flyrail.json", json.dumps(CASES[0]["manifest"]).encode()),
        (PACKAGE + "/flyrail/a/SKILL.md", b""),
    ]


def write_archive(
    path: Path,
    members: list[tuple[str | zipfile.ZipInfo, bytes]],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in members:
            archive.writestr(name, data)
    monkeypatch.syspath_prepend(str(path))


def test_zip_inferred_directories_and_closed_context(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "package.zip"
    write_archive(path, archive_members(), monkeypatch)
    bundle = Bundle.from_package(PACKAGE)
    path.unlink()

    assert bundle.entries == (BundleEntry("a"), BundleEntry("a/SKILL.md", b""))
    assert bundle.content_digest == CASES[0]["content_digest"]


@pytest.mark.parametrize(
    "name",
    [
        "CON.txt",
        "trailing.",
        "trailing ",
        "bad\x7f",
        "bad\x85",
        "bad?",
        "cafe\u0301",
        "../outside",
        "/double",
        "sub/../../outside",
        "C:/escape",
        "a\\b",
        "double//slash",
    ],
)
def test_zip_rejects_nonportable_and_traversal_names(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    members = [*archive_members(), (PACKAGE + "/flyrail/a/" + name, b"bad")]
    write_archive(tmp_path / "package.zip", members, monkeypatch)
    with pytest.raises(ValueError, match="archive path"):
        Bundle.from_package(PACKAGE)


@pytest.mark.parametrize(
    "names",
    [
        ["a/same/", "a/SAME/"],
        ["a/straße", "a/strasse"],
        ["a/parent/a", "a/PARENT/b"],
        ["a/file", "a/file/child"],
        ["a/file/child", "a/file"],
        ["a/extra//"],
    ],
)
def test_zip_rejects_collisions_and_ambiguous_directories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, names: list[str]
) -> None:
    members = archive_members() + [(PACKAGE + "/flyrail/" + name, b"") for name in names]
    write_archive(tmp_path / "package.zip", members, monkeypatch)
    with pytest.raises(ValueError, match=r"collision|ambiguous"):
        Bundle.from_package(PACKAGE)


@pytest.mark.parametrize("name", ["a/SKILL.md", "a/", ""])
def test_zip_rejects_duplicate_members(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    member = (PACKAGE + "/flyrail/" + name, b"")
    members = [*archive_members(), member, member]
    with pytest.warns(UserWarning, match="Duplicate name"):
        write_archive(tmp_path / "package.zip", members, monkeypatch)
    with pytest.raises(ValueError, match="duplicate"):
        Bundle.from_package(PACKAGE)


@pytest.mark.parametrize(
    "kind", [stat.S_IFLNK, stat.S_IFIFO, stat.S_IFSOCK, stat.S_IFCHR, stat.S_IFDIR]
)
def test_zip_rejects_special_member_types(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: int
) -> None:
    info = zipfile.ZipInfo(PACKAGE + "/flyrail/a/special")
    info.create_system = 3
    info.external_attr = (kind | stat.S_IRUSR | stat.S_IWUSR) << 16
    write_archive(tmp_path / "package.zip", [*archive_members(), (info, b"target")], monkeypatch)
    with pytest.raises(ValueError, match="symlinks and special"):
        Bundle.from_package(PACKAGE)


def test_zip_rejects_reparse_metadata(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    info = zipfile.ZipInfo(PACKAGE + "/flyrail/a/reparse")
    info.create_system = 0
    info.external_attr = stat.FILE_ATTRIBUTE_REPARSE_POINT
    write_archive(tmp_path / "package.zip", [*archive_members(), (info, b"target")], monkeypatch)
    with pytest.raises(ValueError, match="reparse"):
        Bundle.from_package(PACKAGE)


def test_zip_rejects_nul_truncated_member_names(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "package.zip"
    members = [*archive_members(), (PACKAGE + "/flyrail/a/nameXhidden", b"data")]
    write_archive(path, members, monkeypatch)
    data = path.read_bytes()
    assert data.count(b"nameXhidden") == 2
    path.write_bytes(data.replace(b"nameXhidden", b"name\0hidden"))

    with pytest.raises(ValueError, match="ambiguous archive entry"):
        Bundle.from_package(PACKAGE)


def test_zip_rejects_ambiguous_resource_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    members = [*archive_members(), (PACKAGE + "/FLYRAIL/", b"")]
    write_archive(tmp_path / "package.zip", members, monkeypatch)
    with pytest.raises(ValueError, match="portable archive name collision"):
        Bundle.from_package(PACKAGE)


@pytest.mark.parametrize(
    ("resource", "unrelated"),
    [
        ("flyrail", PACKAGE.upper() + "/unrelated/file"),
        ("resources/flyrail", PACKAGE + "/RESOURCES/unrelated/file"),
    ],
)
@pytest.mark.parametrize("unrelated_first", [False, True])
def test_zip_rejects_colliding_ancestors_inferred_from_unrelated_members(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    resource: str,
    unrelated: str,
    unrelated_first: bool,
) -> None:
    members: list[tuple[str | zipfile.ZipInfo, bytes]] = [
        (str(name).replace("/flyrail/", f"/{resource}/"), data) for name, data in archive_members()
    ]
    members.insert(0 if unrelated_first else len(members), (unrelated, b""))
    write_archive(tmp_path / "package.zip", members, monkeypatch)

    with pytest.raises(ValueError, match="portable archive name collision"):
        Bundle.from_package(PACKAGE, resource)


def test_zip_allows_unrelated_nonportable_members(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    members = [
        *archive_members(),
        (PACKAGE + "/unrelated/CON.txt", b""),
        (PACKAGE + "/UNRELATED/file", b""),
        ("outside/CON.txt", b""),
    ]
    write_archive(tmp_path / "package.zip", members, monkeypatch)

    bundle = Bundle.from_package(PACKAGE)

    assert bundle.entries == (BundleEntry("a"), BundleEntry("a/SKILL.md", b""))
    assert bundle.content_digest == CASES[0]["content_digest"]


@pytest.mark.parametrize("missing", ["resource", "manifest", "skill", "skill-markdown"])
def test_zip_missing_required_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, missing: str
) -> None:
    members = archive_members()
    if missing == "resource":
        members = members[:1]
    elif missing == "manifest":
        members.pop(1)
    elif missing == "skill":
        members.pop(2)
    else:
        members[2] = (PACKAGE + "/flyrail/a/", b"")
    write_archive(tmp_path / "package.zip", members, monkeypatch)
    with pytest.raises(ValueError, match=r"existing directory|flyrail\.json file|SKILL\.md file"):
        Bundle.from_package(PACKAGE)


def test_zip_rejects_missing_executables(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    members = archive_members()
    manifest = json.loads(members[1][1])
    manifest["skills"][0]["executables"] = ["absent.py"]
    members[1] = (members[1][0], json.dumps(manifest).encode())
    write_archive(tmp_path / "package.zip", members, monkeypatch)
    with pytest.raises(ValueError, match="existing file"):
        Bundle.from_package(PACKAGE)


@pytest.mark.skipif(os.name != "nt", reason="Windows junctions")
def test_rejects_windows_junctions(tmp_path: Path) -> None:
    skill = write_bundle(tmp_path / "bundle")
    target = tmp_path / "external"
    target.mkdir()
    command = shutil.which("cmd")
    assert command is not None
    result = subprocess.run(  # noqa: S603
        [command, "/c", "mklink", "/J", str(skill / "junction"), str(target)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    try:
        with pytest.raises(ValueError, match="reparse"):
            Bundle.from_directory(tmp_path / "bundle")
    finally:
        (skill / "junction").rmdir()
