import hashlib
import importlib
import importlib.resources
import json
import os
import shutil
import stat
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import TypedDict, cast

import flyrail
from flyrail import Bundle, OperationStatus, Target, install, uninstall


class Error(TypedDict):
    code: str


class Installed(TypedDict):
    version: str
    content_digest: str


class ObservationReport(TypedDict):
    state: str
    installed: Installed | None
    version_matches: bool | None
    content_matches: bool | None


class Result(TypedDict):
    status: str
    is_current: bool
    error: Error | None
    observation: ObservationReport
    target: dict[str, object]


class Report(TypedDict):
    app_version: str
    bundle_id: str
    bundle_version: str | None
    results: list[Result]


def command(arguments: list[str], expected: int = 0) -> str:
    environment = os.environ.copy()
    environment.pop("PYTHONPATH", None)
    result = subprocess.run(  # noqa: S603
        arguments, capture_output=True, text=True, env=environment, check=False
    )
    assert result.returncode == expected, (
        f"{arguments}: expected exit {expected}, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    if expected != 2:
        assert result.stderr == "", result.stderr
    else:
        assert result.stderr.strip(), "request errors must explain the failure"
    return result.stdout.strip()


def change_source(root: Path, label: str, extra: bytes = b"") -> None:
    manifest_path = root / "flyrail.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["version"] = label
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    if extra:
        skill = root / manifest["skills"][0]["path"] / "SKILL.md"
        skill.write_bytes(skill.read_bytes() + extra)


@dataclass(frozen=True)
class Host:
    executable: Path
    source: Path
    bundle_id: str
    app_version: str
    skill: str
    filesystem: bool

    def arguments(self, action: str, targets: list[Path], *extra: str) -> list[str]:
        result = [str(self.executable), "ai", action]
        if self.filesystem:
            result += (
                ["--bundle-id", self.bundle_id]
                if action == "uninstall"
                else ["--source", str(self.source)]
            )
        for target in targets:
            result += ["--target", str(target)]
        return [*result, *extra]

    def call(self, action: str, targets: list[Path], *extra: str, expected: int = 0) -> Report:
        report = cast(
            Report, json.loads(command(self.arguments(action, targets, *extra), expected))
        )
        assert report["app_version"] == self.app_version
        assert report["bundle_id"] == self.bundle_id
        assert len(report["results"]) == len(targets)
        assert all(set(item["target"]) == {"root", "agent", "scope"} for item in report["results"])
        return report


def statuses(report: Report) -> list[str]:
    return [result["status"] for result in report["results"]]


def lifecycle(host: Host) -> None:
    root = Path.cwd() / host.bundle_id
    root.mkdir()
    first, blocked, later = [root / name for name in ("first", "blocked", "later")]
    targets = [first, blocked, later]
    blocked.mkdir()
    collision = blocked / host.skill
    collision.mkdir()
    (collision / "SKILL.md").write_bytes(b"untracked content")
    report = host.call("status", [first], expected=1)
    assert report["results"][0]["observation"]["state"] == "absent"
    assert not first.exists()
    assert not flyrail.ResourceAuthority(first).state_root.exists()
    command([str(host.executable), "ai", "install"], expected=2)
    report = host.call("install", targets, expected=1)
    assert statuses(report) == ["applied", "failed", "applied"]
    assert report["results"][1]["error"]
    assert report["results"][1]["error"]["code"] == "conflict"
    assert (collision / "SKILL.md").read_bytes() == b"untracked content"
    active = [first, later]
    for target in active:
        assert (target / host.skill / "SKILL.md").is_file()
    assert statuses(host.call("install", active)) == ["unchanged", "unchanged"]
    assert all(result["is_current"] for result in host.call("status", active)["results"])
    original = Bundle.from_directory(host.source)
    change_source(host.source, "earlier-looking-label")
    report = host.call("status", active, expected=1)
    assert report["bundle_version"] == "earlier-looking-label"
    assert report["results"][0]["observation"]["version_matches"] is False
    assert report["results"][0]["observation"]["content_matches"] is True
    assert statuses(host.call("update", active)) == ["applied", "applied"]
    change_source(host.source, "earlier-looking-label", b"\nA new skill revision.\n")
    report = host.call("status", active, expected=1)
    assert report["results"][0]["observation"]["version_matches"] is True
    assert report["results"][0]["observation"]["content_matches"] is False
    refused = host.call("install", active, expected=1)
    assert all(
        result["error"] and result["error"]["code"] == "update_required"
        for result in refused["results"]
    )
    edited = first / host.skill / "SKILL.md"
    edited.write_bytes(b"local edit")
    (first / host.skill / "personal.txt").write_bytes(b"local addition")
    report = host.call("update", active, expected=1)
    assert statuses(report) == ["failed", "applied"]
    assert report["results"][0]["error"] and report["results"][0]["error"]["code"] == "modified"
    assert edited.read_bytes() == b"local edit"
    assert (first / host.skill / "personal.txt").read_bytes() == b"local addition"
    assert statuses(host.call("update", active, "--replace-modified")) == ["applied", "unchanged"]
    current = Bundle.from_directory(host.source)
    expected_tree = {entry.path: entry.data for entry in current.entries if entry.data is not None}
    installed_tree = {
        path.relative_to(first).as_posix(): path.read_bytes()
        for path in (first / host.skill).rglob("*")
        if path.is_file()
    }
    assert installed_tree == expected_tree
    assert current.version != original.version
    assert current.content_digest != original.content_digest
    assert statuses(host.call("update", active)) == ["unchanged", "unchanged"]
    assert statuses(host.call("update", targets, "--replace-modified", expected=1)) == [
        "unchanged",
        "failed",
        "unchanged",
    ]
    assert (collision / "SKILL.md").read_bytes() == b"untracked content"
    foreign_source = root / "foreign-source"
    foreign_source.mkdir()
    (foreign_source / "foreign-skill").mkdir()
    (foreign_source / "foreign-skill/SKILL.md").write_bytes(b"foreign owned bytes")
    manifest = {
        "schema_version": 1,
        "id": "foreign-owner",
        "version": "x",
        "skills": [{"name": "foreign-skill", "path": "foreign-skill"}],
    }
    (foreign_source / "flyrail.json").write_text(json.dumps(manifest), encoding="utf-8")
    assert (
        install(Bundle.from_directory(foreign_source), [Target.directory(first)])[0].status
        is OperationStatus.APPLIED
    )
    (first / "untracked.txt").write_bytes(b"untracked neighbor")
    edited.write_bytes(b"keep until explicitly removed")
    shutil.rmtree(host.source)
    assert not host.source.exists()
    command(host.arguments("status", active), expected=2)
    report = host.call("uninstall", active, expected=1)
    assert statuses(report) == ["failed", "applied"]
    assert edited.read_bytes() == b"keep until explicitly removed"
    assert statuses(host.call("uninstall", active, "--replace-modified")) == [
        "applied",
        "unchanged",
    ]
    assert statuses(host.call("uninstall", active)) == ["unchanged", "unchanged"]
    assert {path.name for path in first.iterdir()} == {"foreign-skill", "untracked.txt"}
    assert not later.exists()
    assert flyrail.ResourceAuthority(later).state_root.is_dir()
    assert (first / "foreign-skill/SKILL.md").read_bytes() == b"foreign owned bytes"
    assert (first / "untracked.txt").read_bytes() == b"untracked neighbor"
    assert (collision / "SKILL.md").read_bytes() == b"untracked content"
    print(
        f"{host.executable.name}: lifecycle, repeats, partial failure, "
        "edits and source-free removal passed."
    )


def zip_resources(wheel: Path) -> None:
    module_name = "flyrail_example_checksum"
    sys.modules.pop(module_name, None)
    archive = Path.cwd() / wheel.name
    shutil.copyfile(wheel, archive)
    sys.path.insert(0, str(archive))
    importlib.invalidate_caches()
    bundle = Bundle.from_package(module_name)
    assert bundle.source_roots == (archive.resolve(),)
    archive.unlink()
    expected = bytes.fromhex("0001020a0d7f80feff464c595241494c00")
    entries = {entry.path: entry for entry in bundle.entries}
    assert entries["checksum-review/assets/sample.bin"].data == expected
    assert entries["checksum-review/scratch/README.txt"].data
    assert entries["checksum-review/scratch"].is_directory
    assert entries["checksum-review/scripts/verify_checksum.py"].executable
    target = Target.directory(Path.cwd() / "zip-skills")
    assert install(bundle, [target])[0].status is OperationStatus.APPLIED
    script = target.root / "checksum-review/scripts/verify_checksum.py"
    assert script.read_bytes() == entries["checksum-review/scripts/verify_checksum.py"].data
    if os.name != "nt":
        assert stat.S_IMODE(script.stat().st_mode) == 0o755
    assert (target.root / "checksum-review/assets/sample.bin").read_bytes() == expected
    assert uninstall(bundle.id, [target])[0].status is OperationStatus.APPLIED
    print(
        "Built wheel ZIP resources, binary bytes, executable intent "
        "and closed-source snapshot passed."
    )


def main() -> int:
    assert Path(flyrail.__file__).is_relative_to(Path(sys.prefix))
    binary = Path(sys.prefix) / ("Scripts" if os.name == "nt" else "bin")
    suffix = ".exe" if os.name == "nt" else ""
    notes = binary / f"flyrail-notes{suffix}"
    checksum = binary / f"flyrail-checksum{suffix}"
    assert command([str(notes), "--version"]).endswith(" 0.4.0")
    assert command([str(checksum), "--version"]).endswith(" 2.1.0")
    sample = Path.cwd() / "sample.txt"
    sample.write_bytes(b"one two\nthree\n")
    assert command([str(notes), "words", str(sample)]) == "3"
    assert (
        command([str(checksum), "digest", str(sample)])
        == hashlib.sha256(sample.read_bytes()).hexdigest()
    )
    for executable, action in ((notes, "words"), (checksum, "digest")):
        command([str(executable), action, str(sample.with_name("missing"))], expected=2)
    resources = importlib.resources.files("flyrail_example_checksum").joinpath("flyrail")
    assert isinstance(resources, Path)
    assert resources.is_relative_to(Path(sys.prefix))
    lifecycle(
        Host(notes, Path.cwd() / "notes-bundle", "example-notes", "0.4.0", "notes-summary", True)
    )
    lifecycle(Host(checksum, resources, "example-checksum", "2.1.0", "checksum-review", False))
    assert command([str(notes), "words", str(sample)]) == "3"
    assert (
        command([str(checksum), "digest", str(sample)])
        == hashlib.sha256(sample.read_bytes()).hexdigest()
    )
    zip_resources(Path(sys.argv[1]))
    print(
        "Installed console-script QA passed outside source directories with no source PYTHONPATH."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
