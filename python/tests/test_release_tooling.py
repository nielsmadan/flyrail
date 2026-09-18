import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any
from unittest.mock import Mock

import pytest

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent
GIT = shutil.which("git") or ""
assert GIT


def load_script(name: str) -> ModuleType:
    path = ROOT / "scripts" / name
    spec = importlib.util.spec_from_file_location(path.stem, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


release = load_script("release.py")
workflow = load_script("release_workflow.py")


def git_environment() -> dict[str, str]:
    environment = os.environ.copy()
    for name in tuple(environment):
        if name.startswith("GIT_"):
            del environment[name]
    return environment


def git(
    root: Path,
    *args: str,
    input_text: str | None = None,
    input_bytes: bytes | None = None,
) -> str:
    assert input_text is None or input_bytes is None
    result = subprocess.run(  # noqa: S603
        [GIT, *args],
        cwd=root,
        env=git_environment(),
        input=input_bytes if input_bytes is not None else input_text,
        text=input_bytes is None,
        encoding=None if input_bytes is not None else "utf-8",
        capture_output=True,
        check=True,
    )
    output = result.stdout.decode("utf-8") if isinstance(result.stdout, bytes) else result.stdout
    return output.strip()


def add_commit(
    root: Path,
    ref: str,
    message: str,
    path: str,
    content: str,
    parent: str | None = None,
) -> str:
    if parent is None:
        result = subprocess.run(  # noqa: S603
            [GIT, "rev-parse", "--verify", ref],
            cwd=root,
            env=git_environment(),
            text=True,
            encoding="utf-8",
            capture_output=True,
            check=False,
        )
        parent = result.stdout.strip() if result.returncode == 0 else None
    stream = (
        "blob\n"
        "mark :1\n"
        f"data {len(content.encode())}\n"
        f"{content}"
        f"commit {ref}\n"
        "mark :2\n"
        "author Fixture <fixture@example.invalid> 1700000000 +0000\n"
        "committer Fixture <fixture@example.invalid> 1700000000 +0000\n"
        f"data {len(message.encode())}\n"
        f"{message}\n"
    )
    if parent:
        stream += f"from {parent}\n"
    stream += f"M 100644 :1 {path}\ndone\n"
    git(root, "fast-import", "--quiet", input_bytes=stream.encode("utf-8"))
    return git(root, "rev-parse", ref)


def repository(tmp_path: Path, message: str = "feat: initial Python release") -> tuple[Path, Path]:
    root = tmp_path / "work"
    root.mkdir()
    git(root, "init", "--initial-branch=main")
    add_commit(root, "refs/heads/main", message, "python/source.txt", "source\n")
    git(root, "reset", "--hard", "HEAD")
    remote = tmp_path / "origin.git"
    git(tmp_path, "clone", "--bare", str(root), str(remote))
    git(root, "remote", "add", "origin", str(remote))
    git(root, "fetch", "origin")
    return root, remote


def test_git_fixtures_ignore_inherited_repository_context(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    caller = tmp_path / "caller"
    caller.mkdir()
    git(caller, "init", "--initial-branch=main")
    add_commit(caller, "refs/heads/main", "test: caller", "caller.txt", "caller\n")
    git(caller, "reset", "--hard", "HEAD")
    caller_index = caller / ".git/index"
    before = caller_index.read_bytes()
    monkeypatch.setenv("GIT_INDEX_FILE", str(caller_index))
    fixture = tmp_path / "fixture"
    fixture.mkdir()

    repository(fixture)

    assert caller_index.read_bytes() == before
    assert git(caller, "ls-files") == "caller.txt"


def annotated_tag(root: Path, name: str, target: str | None = None) -> str:
    target = target or git(root, "rev-parse", "HEAD")
    payload = (
        f"object {target}\n"
        "type commit\n"
        f"tag {name}\n"
        "tagger Fixture <fixture@example.invalid> 1700000000 +0000\n\n"
        f"Release {name}\n"
    )
    tag_object = git(root, "mktag", input_text=payload)
    git(root, "update-ref", f"refs/tags/{name}", tag_object)
    return tag_object


def release_config() -> Any:
    return release.load_config(ROOT / "scripts" / "release.json")


@pytest.mark.parametrize(
    ("tag", "expected"),
    [
        ("python-v0.1.0", (0, 1, 0)),
        ("python-v12.3.45", (12, 3, 45)),
        ("v0.1.0", None),
        ("python-v01.0.0", None),
    ],
)
def test_python_release_tags_are_component_scoped(
    tag: str, expected: tuple[int, int, int] | None
) -> None:
    assert workflow.version(tag) == expected
    assert release.tag_version(tag, "python-v") == expected


def test_previous_tag_ignores_other_language_releases() -> None:
    assert (
        workflow.previous_tag(
            "python-v1.2.0",
            ["0.9.0", "go/v1.9.0", "rust-v4.0.0", "python-v1.1.10", "python-v1.2.0"],
        )
        == "python-v1.1.10"
    )


@pytest.mark.parametrize(
    ("latest", "messages", "override", "expected"),
    [
        ("python-v0.1.0", ["fix: preserve receipts"], None, "0.1.1"),
        ("python-v0.1.0", ["feat: add receipts"], None, "0.2.0"),
        ("python-v0.1.0", ["feat!: replace receipts"], None, "0.2.0"),
        ("python-v1.2.3", ["fix: compatibility\n\nBREAKING CHANGE: remove API"], None, "2.0.0"),
        ("python-v1.2.3", ["chore: reorganize repository"], None, None),
        (None, ["chore: bootstrap"], None, "0.1.0"),
        ("python-v1.2.3", ["fix: preserve receipts"], "minor", "1.3.0"),
        ("python-v1.2.3", ["fix: preserve receipts"], "3.2.1", "3.2.1"),
    ],
)
def test_release_proposal_follows_semver(
    latest: str | None,
    messages: list[str],
    override: str | None,
    expected: str | None,
) -> None:
    state: dict[str, Any] = {
        "head": "a" * 40,
        "refs": {},
        "latest": latest,
        "messages": messages,
        "ahead": "0",
    }
    version, _counts = release.proposal(state, release_config(), override)
    assert version == expected


@pytest.mark.parametrize("override", ["1.2.3", "1.2.2"])
def test_release_proposal_rejects_non_increasing_versions(override: str) -> None:
    state: dict[str, Any] = {
        "head": "a" * 40,
        "refs": {},
        "latest": "python-v1.2.3",
        "messages": ["fix: preserve receipts"],
        "ahead": "0",
    }
    with pytest.raises(release.ReleaseError, match="must be newer"):
        release.proposal(state, release_config(), override)


@pytest.mark.parametrize(
    "origin",
    [
        "git@github.com:nielsmadan/flyrail.git",
        "ssh://git@github.com/nielsmadan/flyrail.git",
        "https://github.com/nielsmadan/flyrail.git",
    ],
)
def test_github_repository_accepts_exact_github_origins(origin: str) -> None:
    assert release.github_repository(origin) == "nielsmadan/flyrail"


@pytest.mark.parametrize(
    "origin",
    [
        "https://example.com/github.com/nielsmadan/flyrail.git",
        "git@github.example:nielsmadan/flyrail.git",
        "https://github.com/nielsmadan/flyrail/extra",
    ],
)
def test_github_repository_rejects_lookalike_origins(origin: str) -> None:
    assert release.github_repository(origin) is None


def test_release_config_prepares_and_checks_every_version_reference() -> None:
    config = release.load_config(ROOT / "scripts" / "release.json")
    stage = config["stages"][0]
    assert stage["files"] == [
        "python/pyproject.toml",
        "python/examples/filesystem/pyproject.toml",
        "python/examples/packaged/pyproject.toml",
        "python/uv.lock",
        "python/CHANGELOG.md",
    ]
    assert stage["checks"] == [["just", "check-package"]]
    assert stage["commands"][0][0] == "{python}"
    assert "python3" not in config["tools"]


@pytest.mark.parametrize("change", [{"components": 3}, {"branch": None}])
def test_release_config_rejects_unknown_or_invalid_values(
    tmp_path: Path, change: dict[str, object]
) -> None:
    config = json.loads((ROOT / "scripts" / "release.json").read_text())
    config.update(change)
    path = tmp_path / "release.json"
    path.write_text(json.dumps(config))
    with pytest.raises(release.ReleaseError):
        release.load_config(path)


def test_project_metadata_requires_pyflyrail_and_matching_version(tmp_path: Path) -> None:
    project = tmp_path / "pyproject.toml"
    project.write_text(
        """
[project]
name = "pyflyrail"
version = "0.1.0"
classifiers = ["Development Status :: 3 - Alpha"]
""".lstrip()
    )
    assert workflow.project_metadata("python-v0.1.0", project) == ("0.1.0", True)
    with pytest.raises(workflow.WorkflowError, match="does not match"):
        workflow.project_metadata("python-v0.1.1", project)


def test_prepare_release_updates_project_and_example_versions(tmp_path: Path) -> None:
    project = tmp_path / "python"
    project.mkdir()
    (project / "pyproject.toml").write_text('[project]\nname = "pyflyrail"\nversion = "0.1.0"\n')
    for name in ("filesystem", "packaged"):
        example = project / "examples" / name
        example.mkdir(parents=True)
        (example / "pyproject.toml").write_text('[project]\ndependencies = ["pyflyrail==0.1.0"]\n')
    subprocess.run(  # noqa: S603
        [sys.executable, str(ROOT / "scripts" / "prepare_release.py"), "9.8.7"],
        cwd=tmp_path,
        check=True,
    )
    assert (project / "pyproject.toml").read_text() == (
        '[project]\nname = "pyflyrail"\nversion = "9.8.7"\n'
    )
    for name in ("filesystem", "packaged"):
        assert (
            project / "examples" / name / "pyproject.toml"
        ).read_text() == '[project]\ndependencies = ["pyflyrail==9.8.7"]\n'


@pytest.mark.parametrize("invalid", ["missing", "duplicate"])
def test_prepare_release_validates_all_references_before_writing(
    tmp_path: Path, invalid: str
) -> None:
    project = tmp_path / "python"
    project.mkdir()
    original_project = '[project]\nname = "pyflyrail"\nversion = "0.1.0"\n'
    (project / "pyproject.toml").write_text(original_project)
    originals: dict[Path, str] = {}
    for name in ("filesystem", "packaged"):
        example = project / "examples" / name
        example.mkdir(parents=True)
        dependency = (
            'dependencies = ["other==0.1.0"]\n'
            if invalid == "missing" and name == "packaged"
            else 'dependencies = ["pyflyrail==0.1.0", "pyflyrail==0.1.0"]\n'
            if invalid == "duplicate" and name == "packaged"
            else 'dependencies = ["pyflyrail==0.1.0"]\n'
        )
        content = "[project]\n" + dependency
        path = example / "pyproject.toml"
        path.write_text(content)
        originals[path] = content
    result = subprocess.run(  # noqa: S603
        [sys.executable, str(ROOT / "scripts" / "prepare_release.py"), "9.8.7"],
        cwd=tmp_path,
        text=True,
        encoding="utf-8",
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert (project / "pyproject.toml").read_text() == original_project
    assert all(path.read_text() == content for path, content in originals.items())


def test_inspect_accepts_expected_repository_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    monkeypatch.setattr(release, "github_repository", lambda _origin: "nielsmadan/flyrail")
    state = release.inspect(root, release_config())
    assert state["head"] == git(root, "rev-parse", "HEAD")
    assert state["latest"] is None
    assert state["messages"] == ["feat: initial Python release"]


def test_inspect_rejects_wrong_branch(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    root, _remote = repository(tmp_path)
    git(root, "branch", "other")
    git(root, "switch", "other")
    monkeypatch.setattr(release, "github_repository", lambda _origin: "nielsmadan/flyrail")
    with pytest.raises(release.ReleaseError, match="Release from main"):
        release.inspect(root, release_config())


def test_inspect_rejects_wrong_origin(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    root, _remote = repository(tmp_path)
    monkeypatch.setattr(release, "github_repository", lambda _origin: "other/project")
    with pytest.raises(release.ReleaseError, match="does not match"):
        release.inspect(root, release_config())


def test_inspect_rejects_local_tag_not_on_origin(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    annotated_tag(root, "python-v0.1.0")
    monkeypatch.setattr(release, "github_repository", lambda _origin: "nielsmadan/flyrail")
    with pytest.raises(release.ReleaseError, match="tags differ"):
        release.inspect(root, release_config())


def test_inspect_rejects_checkout_behind_origin(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    original = git(root, "rev-parse", "HEAD")
    add_commit(root, "refs/heads/main", "fix: remote change", "python/remote.txt", "remote\n")
    git(root, "push", "origin", "main")
    git(root, "update-ref", "refs/heads/main", original)
    git(root, "fetch", "origin")
    monkeypatch.setattr(release, "github_repository", lambda _origin: "nielsmadan/flyrail")
    with pytest.raises(release.ReleaseError, match="include origin/main"):
        release.inspect(root, release_config())


def test_inspect_rejects_history_without_component_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path, message="chore: repository setup")
    base = git(root, "rev-parse", "HEAD")
    annotated_tag(root, "python-v0.1.0", base)
    git(root, "push", "origin", "refs/tags/python-v0.1.0")
    add_commit(root, "refs/heads/main", "chore: root docs", "README.md", "docs\n")
    git(root, "reset", "--hard", "HEAD")
    monkeypatch.setattr(release, "github_repository", lambda _origin: "nielsmadan/flyrail")
    with pytest.raises(release.ReleaseError, match="no component commits"):
        release.inspect(root, release_config())


def test_prepare_commits_only_allowed_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    allowed = root / "allowed.txt"
    add_commit(root, "refs/heads/main", "chore: add fixture", "allowed.txt", "before\n")
    git(root, "reset", "--hard", "HEAD")
    original_run = release.run

    def run_with_commit(root_path: Path, *args: str, capture: bool = True) -> str:
        if args[:2] != ("git", "commit"):
            result = original_run(root_path, *args, capture=capture)
            assert isinstance(result, str)
            return result
        add_commit(
            root_path,
            "refs/heads/main",
            "chore: release fixture",
            "allowed.txt",
            allowed.read_text(),
        )
        original_run(root_path, "git", "reset", "--mixed", "HEAD")
        return ""

    monkeypatch.setattr(release, "run", run_with_commit)
    config = release_config()
    config["stages"] = [
        {
            "files": ["allowed.txt"],
            "commands": [
                [
                    sys.executable,
                    "-c",
                    "from pathlib import Path; Path('allowed.txt').write_text('after\\n')",
                ]
            ],
            "checks": [],
            "message": "chore: release {version}",
        }
    ]
    release.prepare(root, config, "0.2.0")
    assert allowed.read_text() == "after\n"
    assert git(root, "status", "--porcelain") == ""


@pytest.mark.parametrize("phase", ["command", "check"])
def test_prepare_rejects_unexpected_changes(tmp_path: Path, phase: str) -> None:
    root, _remote = repository(tmp_path)
    config = release_config()
    writer = [
        sys.executable,
        "-c",
        "from pathlib import Path; Path('unexpected.txt').write_text('unexpected\\n')",
    ]
    config["stages"] = [
        {
            "files": ["allowed.txt"],
            "commands": [writer] if phase == "command" else [],
            "checks": [writer] if phase == "check" else [],
            "message": "chore: release {version}",
        }
    ]
    expected = "Preparation changed" if phase == "command" else "Prepared checks changed"
    with pytest.raises(release.ReleaseError, match=expected):
        release.prepare(root, config, "0.2.0")


def test_dry_run_requires_only_git(monkeypatch: pytest.MonkeyPatch) -> None:
    state: dict[str, Any] = {
        "head": "a" * 40,
        "refs": {},
        "latest": None,
        "messages": ["feat: initial Python release"],
        "ahead": "0",
    }
    monkeypatch.setattr(sys, "argv", ["release.py", "--dry-run"])
    monkeypatch.setattr(release, "inspect", Mock(return_value=state))
    which = Mock(return_value="/usr/bin/git")
    monkeypatch.setattr(release.shutil, "which", which)
    release.main()
    which.assert_called_once_with("git")


def test_validate_tag_accepts_annotated_tag_on_origin_main(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    annotated_tag(root, "python-v0.1.0")
    git(root, "push", "origin", "refs/tags/python-v0.1.0")
    monkeypatch.chdir(root)
    target = git(root, "rev-parse", "HEAD")
    assert workflow.validate_tag("python-v0.1.0", target) == target
    workflow.verify_remote_tag("python-v0.1.0", target)


def test_verify_remote_tag_rejects_divergent_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    annotated_tag(root, "python-v0.1.0")
    git(root, "push", "origin", "refs/tags/python-v0.1.0")
    monkeypatch.chdir(root)
    with pytest.raises(workflow.WorkflowError, match="verified commit"):
        workflow.verify_remote_tag("python-v0.1.0", "f" * 40)


def test_validate_tag_rejects_lightweight_tag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    git(root, "update-ref", "refs/tags/python-v0.1.0", git(root, "rev-parse", "HEAD"))
    monkeypatch.chdir(root)
    with pytest.raises(workflow.WorkflowError, match="annotated"):
        workflow.validate_tag("python-v0.1.0")


def test_validate_tag_rejects_event_sha_mismatch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    tagged = git(root, "rev-parse", "HEAD")
    annotated_tag(root, "python-v0.1.0", tagged)
    other = add_commit(root, "refs/heads/main", "fix: later", "python/later.txt", "later\n")
    monkeypatch.chdir(root)
    with pytest.raises(workflow.WorkflowError, match="does not identify"):
        workflow.validate_tag("python-v0.1.0", other)


def test_validate_tag_rejects_commit_outside_origin_main(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    main = git(root, "rev-parse", "HEAD")
    side = add_commit(
        root,
        "refs/heads/side",
        "feat: side",
        "python/side.txt",
        "side\n",
        parent=main,
    )
    annotated_tag(root, "python-v0.1.0", side)
    monkeypatch.chdir(root)
    with pytest.raises(workflow.WorkflowError, match="not contained"):
        workflow.validate_tag("python-v0.1.0")


def test_matching_partial_release_uploads_only_missing_assets(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wheel = tmp_path / "pyflyrail-0.1.0-py3-none-any.whl"
    source = tmp_path / "pyflyrail-0.1.0.tar.gz"
    wheel.write_bytes(b"wheel")
    source.write_bytes(b"source")
    notes = tmp_path / "notes.md"
    notes.write_text("notes\n")
    remote_tag = Mock()
    monkeypatch.setattr(workflow, "verify_remote_tag", remote_tag)
    monkeypatch.setattr(
        workflow,
        "_release",
        Mock(
            return_value={
                "tagName": "python-v0.1.0",
                "name": "Flyrail for Python 0.1.0",
                "body": "notes\n",
                "isDraft": False,
                "isPrerelease": True,
                "isImmutable": False,
                "assets": [
                    {
                        "name": wheel.name,
                        "state": "uploaded",
                        "size": wheel.stat().st_size,
                        "digest": f"sha256:{workflow._sha256(wheel)}",
                    }
                ],
            }
        ),
    )
    command = Mock()
    monkeypatch.setattr(workflow, "command", command)
    workflow.publish(
        "python-v0.1.0",
        "Flyrail for Python 0.1.0",
        notes,
        "a" * 40,
        [str(wheel), str(source)],
        prerelease=True,
    )
    remote_tag.assert_called_once_with("python-v0.1.0", "a" * 40)
    command.assert_called_once_with("gh", "release", "upload", "python-v0.1.0", str(source))


def release_assets(tmp_path: Path) -> tuple[Path, Path, Path]:
    wheel = tmp_path / "pyflyrail-0.1.0-py3-none-any.whl"
    source = tmp_path / "pyflyrail-0.1.0.tar.gz"
    notes = tmp_path / "notes.md"
    wheel.write_bytes(b"wheel")
    source.write_bytes(b"source")
    notes.write_text("notes\n")
    return wheel, source, notes


def matching_release(
    wheel: Path,
    source: Path,
    *,
    draft: bool = False,
    immutable: bool = False,
) -> dict[str, object]:
    return {
        "tagName": "python-v0.1.0",
        "name": "Flyrail for Python 0.1.0",
        "body": "notes\n",
        "isDraft": draft,
        "isPrerelease": True,
        "isImmutable": immutable,
        "assets": [
            {
                "name": asset.name,
                "state": "uploaded",
                "size": asset.stat().st_size,
                "digest": f"sha256:{workflow._sha256(asset)}",
            }
            for asset in (wheel, source)
        ],
    }


def test_new_release_creates_expected_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wheel, source, notes = release_assets(tmp_path)
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(workflow, "_release", Mock(return_value=None))
    command = Mock()
    monkeypatch.setattr(workflow, "command", command)
    workflow.publish(
        "python-v0.1.0",
        "Flyrail for Python 0.1.0",
        notes,
        "a" * 40,
        [str(wheel), str(source)],
        prerelease=True,
    )
    command.assert_called_once_with(
        "gh",
        "release",
        "create",
        "python-v0.1.0",
        str(wheel),
        str(source),
        "--verify-tag",
        "--title",
        "Flyrail for Python 0.1.0",
        "--notes-file",
        str(notes),
        "--latest=false",
        "--prerelease",
    )


def test_matching_draft_release_is_published(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wheel, source, notes = release_assets(tmp_path)
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(
        workflow, "_release", Mock(return_value=matching_release(wheel, source, draft=True))
    )
    command = Mock()
    monkeypatch.setattr(workflow, "command", command)
    workflow.publish(
        "python-v0.1.0",
        "Flyrail for Python 0.1.0",
        notes,
        "a" * 40,
        [str(wheel), str(source)],
        prerelease=True,
    )
    command.assert_called_once_with("gh", "release", "edit", "python-v0.1.0", "--draft=false")


def test_immutable_release_rejects_missing_assets(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wheel, source, notes = release_assets(tmp_path)
    release_data = matching_release(wheel, source, immutable=True)
    assets = release_data["assets"]
    assert isinstance(assets, list)
    release_data["assets"] = assets[:1]
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(workflow, "_release", Mock(return_value=release_data))
    with pytest.raises(workflow.WorkflowError, match="immutable"):
        workflow.publish(
            "python-v0.1.0",
            "Flyrail for Python 0.1.0",
            notes,
            "a" * 40,
            [str(wheel), str(source)],
            prerelease=True,
        )


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("name", "Wrong title", "name differs"),
        ("body", "wrong notes\n", "body differs"),
        ("isPrerelease", False, "prerelease state differs"),
    ],
)
def test_existing_release_rejects_metadata_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    field: str,
    value: object,
    message: str,
) -> None:
    wheel, source, notes = release_assets(tmp_path)
    release_data = matching_release(wheel, source)
    release_data[field] = value
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(workflow, "_release", Mock(return_value=release_data))
    with pytest.raises(workflow.WorkflowError, match=message):
        workflow.publish(
            "python-v0.1.0",
            "Flyrail for Python 0.1.0",
            notes,
            "a" * 40,
            [str(wheel), str(source)],
            prerelease=True,
        )


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("state", "new", "not fully uploaded"),
        ("size", 999, "size differs"),
        ("digest", "sha256:wrong", "digest differs"),
    ],
)
def test_existing_release_rejects_asset_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    field: str,
    value: object,
    message: str,
) -> None:
    wheel, source, notes = release_assets(tmp_path)
    release_data = matching_release(wheel, source)
    assets = release_data["assets"]
    assert isinstance(assets, list)
    first = assets[0]
    assert isinstance(first, dict)
    first[field] = value
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(workflow, "_release", Mock(return_value=release_data))
    with pytest.raises(workflow.WorkflowError, match=message):
        workflow.publish(
            "python-v0.1.0",
            "Flyrail for Python 0.1.0",
            notes,
            "a" * 40,
            [str(wheel), str(source)],
            prerelease=True,
        )


def job_section(content: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-z][a-z0-9-]*:\n|\Z)",
        content,
    )
    assert match is not None
    return match.group("body")


def test_release_workflow_uses_pinned_actions_and_split_permissions() -> None:
    path = REPOSITORY / ".github" / "workflows" / "release-python.yml"
    content = path.read_text()
    references = re.findall(r"uses:\s+[^@\s]+@([^\s#]+)", content)
    assert references
    assert all(re.fullmatch(r"[0-9a-f]{40}", reference) for reference in references)
    verify = job_section(content, "verify")
    platform_checks = job_section(content, "platform-checks")
    pypi = job_section(content, "publish-pypi")
    github = job_section(content, "publish-github")
    assert "id-token: write" not in verify
    assert "contents: write" not in verify
    assert "needs: verify" in platform_checks
    assert platform_checks.count("- os:") == 5
    assert "needs: [verify, platform-checks]" in pypi
    assert "id-token: write" in pypi
    assert "contents: write" not in pypi
    assert "uv publish" in pypi
    assert "needs: [verify, publish-pypi]" in github
    assert "contents: write" in github
    assert "id-token: write" not in github
    assert 'tags: ["python-v*"]' in content
    for variable in (
        "UV_CACHE_DIR",
        "UV_PYTHON_INSTALL_DIR",
        "UV_TOOL_DIR",
        "UV_TOOL_BIN_DIR",
        "XDG_CACHE_HOME",
    ):
        assert f"{variable}:" in content
