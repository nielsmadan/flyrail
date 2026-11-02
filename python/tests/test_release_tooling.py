import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
from types import ModuleType
from unittest.mock import Mock

import pytest

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent
GIT = shutil.which("git") or ""
assert GIT


def load_script(name: str) -> ModuleType:
    return load_path(ROOT / "scripts" / name)


def load_path(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(path.stem, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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


@pytest.mark.parametrize(
    ("tag", "expected"),
    [
        ("python-v0.1.0", ("python", (0, 1, 0))),
        ("python-v12.3.45", ("python", (12, 3, 45))),
        ("rust-v0.1.0", ("rust", (0, 1, 0))),
        ("v2.0.1", ("swift", (2, 0, 1))),
        ("python-v01.0.0", None),
        ("go/v1.0.0", None),
        ("0.1.0", None),
        ("vv0.1.0", None),
    ],
)
def test_release_tags_identify_their_component(
    tag: str, expected: tuple[str, tuple[int, int, int]] | None
) -> None:
    assert workflow.parse_tag(tag) == expected


TAGS = [
    "0.9.0",
    "go/v1.9.0",
    "rust-v1.1.10",
    "rust-v4.0.0",
    "python-v1.1.10",
    "python-v1.2.0",
    "v1.1.9",
    "v1.3.0",
]


@pytest.mark.parametrize(
    ("current", "expected"),
    [
        ("python-v1.2.0", "python-v1.1.10"),
        ("rust-v1.2.0", "rust-v1.1.10"),
        ("v1.2.0", "v1.1.9"),
        ("rust-v1.0.0", None),
    ],
)
def test_previous_tag_ignores_other_language_releases(current: str, expected: str | None) -> None:
    assert workflow.previous_tag(current, TAGS) == expected


def test_previous_tag_rejects_unknown_tag() -> None:
    with pytest.raises(workflow.WorkflowError, match="Invalid release tag"):
        workflow.previous_tag("go/v1.0.0", TAGS)


def test_release_config_prepares_and_checks_every_version_reference() -> None:
    config = json.loads((ROOT / "scripts" / "release.json").read_text())
    stage = config["stages"][0]
    assert stage["files"] == [
        "python/pyproject.toml",
        "python/examples/filesystem/pyproject.toml",
        "python/examples/packaged/pyproject.toml",
        "python/uv.lock",
        "python/CHANGELOG.md",
    ]
    assert stage["checks"] == [["just", "-f", "python/Justfile", "check-package"]]
    assert stage["commands"][0][0] == "{python}"
    assert "python3" not in config["tools"]


LANGUAGE_RELEASES = [
    (
        "rust",
        "rust-v",
        "release-rust.yml",
        ["rust", "rust-toolchain.toml"],
        [
            "rust/Cargo.toml",
            "rust/examples/filesystem/Cargo.toml",
            "rust/examples/packaged/Cargo.toml",
            "rust/Cargo.lock",
            "rust/CHANGELOG.md",
        ],
    ),
    (
        "swift",
        "v",
        "release-swift.yml",
        ["swift", "Package.swift", "Package.resolved"],
        ["swift/CHANGELOG.md"],
    ),
]


def include_paths(arguments: list[str]) -> list[str]:
    return [
        arguments[index + 1] for index, value in enumerate(arguments) if value == "--include-path"
    ]


@pytest.mark.parametrize(
    ("component", "prefix", "workflow_name", "paths", "files"), LANGUAGE_RELEASES
)
def test_language_release_configs_scope_tags_history_and_changed_files(
    component: str, prefix: str, workflow_name: str, paths: list[str], files: list[str]
) -> None:
    config = json.loads((REPOSITORY / component / "scripts" / "release.json").read_text())
    assert config["tag_prefix"] == prefix
    assert workflow.parse_tag(prefix + config["initial_version"]) == (component, (0, 1, 0))
    assert config["paths"] == paths
    assert config["workflow"] == workflow_name
    [stage] = config["stages"]
    assert stage["files"] == files


@pytest.mark.parametrize(
    ("component", "prefix", "workflow_name", "paths", "files"), LANGUAGE_RELEASES
)
def test_every_changelog_command_uses_the_component_history(
    component: str, prefix: str, workflow_name: str, paths: list[str], files: list[str]
) -> None:
    expected = [f"{path}/**" if (REPOSITORY / path).is_dir() else path for path in paths]
    [stage] = json.loads((REPOSITORY / component / "scripts" / "release.json").read_text())[
        "stages"
    ]
    cliff = stage["commands"][-1]
    assert cliff[cliff.index("--config") + 1] == f"{component}/cliff.toml"
    assert cliff[cliff.index("--tag") + 1] == prefix + "{version}"
    assert cliff[cliff.index("-o") + 1] == f"{component}/CHANGELOG.md"
    assert include_paths(cliff) == expected
    recipe = re.search(
        rf"^changelog-{component}:\n    (?P<command>.+)$",
        (REPOSITORY / "Justfile").read_text(),
        re.MULTILINE,
    )
    assert recipe is not None
    assert include_paths(recipe.group("command").replace('"', "").split()) == expected
    notes = job_section((REPOSITORY / ".github/workflows" / workflow_name).read_text(), "verify")
    assert re.findall(r'--include-path "([^"]+)"', notes) == expected
    cliff_config = (REPOSITORY / component / "cliff.toml").read_text()
    assert f'tag_pattern = "^{prefix}(' in cliff_config
    assert f"Flyrail for {component.capitalize()}." in cliff_config


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
    assert run_script(ROOT / "scripts" / "prepare_release.py", tmp_path, "9.8.7").returncode == 0
    assert (project / "pyproject.toml").read_text() == (
        '[project]\nname = "pyflyrail"\nversion = "9.8.7"\n'
    )
    for name in ("filesystem", "packaged"):
        assert (
            project / "examples" / name / "pyproject.toml"
        ).read_text() == '[project]\ndependencies = ["pyflyrail==9.8.7"]\n'


RUST_MANIFEST = """[workspace]
members = ["."]

[workspace.package]
version = "0.2.0"
edition = "2024"

[package]
name = "flyrail"
version.workspace = true
"""


def test_project_metadata_reads_rust_workspace_version(tmp_path: Path) -> None:
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text(RUST_MANIFEST)
    assert workflow.project_metadata("rust-v0.2.0", manifest) == ("0.2.0", True)
    with pytest.raises(workflow.WorkflowError, match="does not match"):
        workflow.project_metadata("rust-v0.2.1", manifest)
    manifest.write_text(RUST_MANIFEST.replace('name = "flyrail"', 'name = "other"'))
    with pytest.raises(workflow.WorkflowError, match="Expected flyrail crate"):
        workflow.project_metadata("rust-v0.2.0", manifest)


@pytest.mark.parametrize(
    ("manifest", "version", "expected"),
    [
        ('[package]\nname = "flyrail"\nversion = "1.0.0"\n', "1.0.0", ("1.0.0", False)),
        ('[package]\nname = "flyrail"\nversion = "0.3.0"\n', "0.3.0", ("0.3.0", True)),
    ],
)
def test_project_metadata_reads_literal_rust_versions(
    tmp_path: Path, manifest: str, version: str, expected: tuple[str, bool]
) -> None:
    path = tmp_path / "Cargo.toml"
    path.write_text(manifest)
    assert workflow.project_metadata(f"rust-v{version}", path) == expected


@pytest.mark.parametrize(
    ("tag", "manifest"),
    [
        ("rust-v0.2.0", '[package]\nname = "flyrail"\nversion.workspace = true\n'),
        ("rust-v0.2.0", '[workspace.package]\nversion = "0.2.0"\n'),
        ("rust-v0.2.0", 'let package = Package(name: "Flyrail",)\n'),
        ("python-v0.2.0", RUST_MANIFEST),
    ],
)
def test_project_metadata_reports_unusable_manifests(
    tmp_path: Path, tag: str, manifest: str
) -> None:
    path = tmp_path / "manifest"
    path.write_text(manifest)
    with pytest.raises(workflow.WorkflowError, match="is not a valid"):
        workflow.project_metadata(tag, path)


def test_project_metadata_takes_swift_version_from_tag(tmp_path: Path) -> None:
    manifest = tmp_path / "Package.swift"
    manifest.write_text('let package = Package(\n    name: "Flyrail",\n)\n')
    assert workflow.project_metadata("v0.3.0", manifest) == ("0.3.0", True)
    assert workflow.project_metadata("v1.0.0", manifest) == ("1.0.0", False)


@pytest.mark.parametrize(
    "manifest",
    [
        'let package = Package(name: "Other")\n',
        'let package = Package(name: "FlyrailKit",)\n',
        'let package = Package(name: "Other", targets: [.target(name: "Flyrail",)])\n',
    ],
)
def test_project_metadata_rejects_other_swift_packages(tmp_path: Path, manifest: str) -> None:
    path = tmp_path / "Package.swift"
    path.write_text(manifest)
    with pytest.raises(workflow.WorkflowError, match="Flyrail Swift package"):
        workflow.project_metadata("v0.3.0", path)


def test_repository_manifests_match_their_own_release_tags() -> None:
    manifest = tomllib.loads((REPOSITORY / "rust/Cargo.toml").read_text())
    rust = manifest["workspace"]["package"]["version"]
    assert workflow.project_metadata(f"rust-v{rust}", REPOSITORY / "rust/Cargo.toml")[0] == rust
    assert workflow.project_metadata("v0.1.0", REPOSITORY / "Package.swift")[0] == "0.1.0"


EXAMPLE_MANIFEST = (
    '[package]\nname = "example"\nversion = "2.1.0"\n\n'
    '[dependencies]\nflyrail = { version = "0.2.0", path = "../.." }\n'
)


def rust_release_tree(root: Path, packaged: str = EXAMPLE_MANIFEST) -> dict[Path, str]:
    files = {
        root / "rust/Cargo.toml": RUST_MANIFEST,
        root / "rust/examples/filesystem/Cargo.toml": EXAMPLE_MANIFEST,
        root / "rust/examples/packaged/Cargo.toml": packaged,
    }
    for path, content in files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    return files


def run_script(script: Path, root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(  # noqa: S603
        [sys.executable, str(script), *arguments],
        cwd=root,
        text=True,
        encoding="utf-8",
        capture_output=True,
        check=False,
    )


RUST_PREPARE = REPOSITORY / "rust/scripts/prepare_release.py"


def test_rust_prepare_release_updates_workspace_and_example_requirements(tmp_path: Path) -> None:
    files = rust_release_tree(tmp_path)
    assert run_script(RUST_PREPARE, tmp_path, "0.3.0").returncode == 0
    assert (tmp_path / "rust/Cargo.toml").read_text() == RUST_MANIFEST.replace(
        'version = "0.2.0"', 'version = "0.3.0"'
    )
    assert [path.read_text() for path in files if "examples" in path.parts] == [
        EXAMPLE_MANIFEST.replace('flyrail = { version = "0.2.0"', 'flyrail = { version = "0.3.0"')
    ] * 2


def test_rust_prepare_release_accepts_the_repository_manifests(tmp_path: Path) -> None:
    for name in ("Cargo.toml", "examples/filesystem/Cargo.toml", "examples/packaged/Cargo.toml"):
        (tmp_path / "rust" / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(REPOSITORY / "rust" / name, tmp_path / "rust" / name)
    assert run_script(RUST_PREPARE, tmp_path, "9.8.7").returncode == 0
    manifest = tomllib.loads((tmp_path / "rust/Cargo.toml").read_text())
    assert manifest["workspace"]["package"]["version"] == "9.8.7"
    assert all(
        tomllib.loads((tmp_path / "rust/examples" / name / "Cargo.toml").read_text())[
            "dependencies"
        ]["flyrail"]["version"]
        == "9.8.7"
        for name in ("filesystem", "packaged")
    )


@pytest.mark.parametrize(
    ("root_manifest", "packaged"),
    [
        (RUST_MANIFEST, '[package]\nname = "example"\n'),
        (RUST_MANIFEST, EXAMPLE_MANIFEST + 'flyrail = { version = "0.2.0", path = "../.." }\n'),
        (
            RUST_MANIFEST.replace('version = "0.2.0"\n', "").replace(
                "version.workspace = true", 'version = "0.2.0"'
            ),
            EXAMPLE_MANIFEST,
        ),
    ],
    ids=["missing", "duplicate", "outside-workspace-package"],
)
def test_rust_prepare_release_validates_all_references_before_writing(
    tmp_path: Path, root_manifest: str, packaged: str
) -> None:
    files = rust_release_tree(tmp_path, packaged)
    (tmp_path / "rust/Cargo.toml").write_text(root_manifest)
    files[tmp_path / "rust/Cargo.toml"] = root_manifest
    result = run_script(RUST_PREPARE, tmp_path, "0.3.0")
    assert "Expected one version reference" in result.stderr
    assert [path.read_text() for path in files] == list(files.values())


@pytest.mark.parametrize("arguments", [(), ("0.3",), ("v0.3.0",), ("0.3.0", "extra")])
def test_rust_prepare_release_requires_one_canonical_version(
    tmp_path: Path, arguments: tuple[str, ...]
) -> None:
    files = rust_release_tree(tmp_path)
    result = run_script(RUST_PREPARE, tmp_path, *arguments)
    assert "usage: prepare_release.py MAJOR.MINOR.PATCH" in result.stderr
    assert [path.read_text() for path in files] == list(files.values())


def test_rust_package_qa_reads_the_workspace_crate_version(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package_qa = load_path(REPOSITORY / "rust/scripts/package_qa.py")
    (tmp_path / "Cargo.toml").write_text(RUST_MANIFEST.replace("0.2.0", "9.8.7"))
    monkeypatch.setattr(package_qa, "RUST", tmp_path)
    assert package_qa.crate_version() == "9.8.7"


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
    result = run_script(ROOT / "scripts" / "prepare_release.py", tmp_path, "9.8.7")
    assert result.returncode != 0
    assert (project / "pyproject.toml").read_text() == original_project
    assert all(path.read_text() == content for path, content in originals.items())


def test_validate_tag_accepts_annotated_tag_on_origin_main(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    annotated_tag(root, "python-v0.1.0")
    git(root, "push", "origin", "refs/tags/python-v0.1.0")
    monkeypatch.chdir(root)
    target = git(root, "rev-parse", "HEAD")
    assert workflow.validate_tag("python-v0.1.0", "python", target) == target
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
        workflow.validate_tag("python-v0.1.0", "python")


def test_validate_tag_rejects_event_sha_mismatch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _remote = repository(tmp_path)
    tagged = git(root, "rev-parse", "HEAD")
    annotated_tag(root, "python-v0.1.0", tagged)
    other = add_commit(root, "refs/heads/main", "fix: later", "python/later.txt", "later\n")
    monkeypatch.chdir(root)
    with pytest.raises(workflow.WorkflowError, match="does not identify"):
        workflow.validate_tag("python-v0.1.0", "python", other)


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
        workflow.validate_tag("python-v0.1.0", "python")


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
        "_github_release",
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


def test_new_release_without_assets_creates_notes_only_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    notes = tmp_path / "notes.md"
    notes.write_text("notes\n")
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(workflow, "_github_release", Mock(return_value=None))
    command = Mock()
    monkeypatch.setattr(workflow, "command", command)
    workflow.publish("v0.1.0", "Flyrail for Swift 0.1.0", notes, "a" * 40, [], prerelease=True)
    command.assert_called_once_with(
        "gh",
        "release",
        "create",
        "v0.1.0",
        "--verify-tag",
        "--title",
        "Flyrail for Swift 0.1.0",
        "--notes-file",
        str(notes),
        "--latest=false",
        "--prerelease",
    )


@pytest.mark.parametrize(("tag", "component"), [("v0.1.0", "rust"), ("rust-v0.1.0", "python")])
def test_validate_tag_rejects_another_components_tag(tag: str, component: str) -> None:
    with pytest.raises(workflow.WorkflowError, match=f"not {component}"):
        workflow.validate_tag(tag, component)


def test_publish_requires_assets_for_registry_components(tmp_path: Path) -> None:
    notes = tmp_path / "notes.md"
    notes.write_text("notes\n")
    with pytest.raises(workflow.WorkflowError, match="requires at least one asset"):
        workflow.publish("rust-v0.1.0", "Flyrail for Rust 0.1.0", notes, "a" * 40, [])


def test_publish_cli_accepts_a_release_without_assets(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    publish = Mock()
    monkeypatch.setattr(workflow, "publish", publish)
    monkeypatch.setattr(
        sys,
        "argv",
        ["release_workflow.py", "publish", "v0.1.0", "Title", "notes.md", "a" * 40, "--prerelease"],
    )
    workflow.main()
    publish.assert_called_once_with(
        "v0.1.0", "Title", Path("notes.md"), "a" * 40, [], prerelease=True
    )


def swift_release(notes: str, assets: list[dict[str, object]], *, draft: bool) -> dict[str, object]:
    return {
        "tagName": "v0.1.0",
        "name": "Flyrail for Swift 0.1.0",
        "body": notes,
        "isDraft": draft,
        "isPrerelease": True,
        "isImmutable": False,
        "assets": assets,
    }


def test_swift_retry_publishes_existing_draft_without_assets(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    notes = tmp_path / "notes.md"
    notes.write_text("notes\n")
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(
        workflow, "_github_release", Mock(return_value=swift_release("notes\n", [], draft=True))
    )
    command = Mock()
    monkeypatch.setattr(workflow, "command", command)
    workflow.publish("v0.1.0", "Flyrail for Swift 0.1.0", notes, "a" * 40, [], prerelease=True)
    command.assert_called_once_with("gh", "release", "edit", "v0.1.0", "--draft=false")


def test_swift_retry_rejects_release_with_added_assets(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    notes = tmp_path / "notes.md"
    notes.write_text("notes\n")
    added = {"name": "extra.zip", "state": "uploaded", "size": 1, "digest": "sha256:0"}
    monkeypatch.setattr(workflow, "verify_remote_tag", Mock())
    monkeypatch.setattr(
        workflow,
        "_github_release",
        Mock(return_value=swift_release("notes\n", [added], draft=False)),
    )
    with pytest.raises(workflow.WorkflowError, match=r"unexpected assets: extra\.zip"):
        workflow.publish("v0.1.0", "Flyrail for Swift 0.1.0", notes, "a" * 40, [], prerelease=True)


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
    monkeypatch.setattr(workflow, "_github_release", Mock(return_value=None))
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
        workflow, "_github_release", Mock(return_value=matching_release(wheel, source, draft=True))
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
    monkeypatch.setattr(workflow, "_github_release", Mock(return_value=release_data))
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
    monkeypatch.setattr(workflow, "_github_release", Mock(return_value=release_data))
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
    monkeypatch.setattr(workflow, "_github_release", Mock(return_value=release_data))
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
    content = read_workflow("release-python.yml")
    assert_pinned_and_cacheless(content)
    verify = job_section(content, "verify")
    platform_checks = job_section(content, "platform-checks")
    pypi = job_section(content, "publish-pypi")
    github = job_section(content, "publish-github")
    assert "id-token: write" not in verify
    assert "contents: write" not in verify
    assert "needs: verify" in platform_checks
    assert re.findall(r'- os: (\S+)\n\s+python: "(\S+)"', platform_checks) == [
        ("ubuntu-24.04", "3.14"),
        ("macos-15", "3.11"),
        ("macos-15", "3.14"),
    ]
    assert "needs: [verify, platform-checks]" in pypi
    assert "id-token: write" in pypi
    assert "contents: write" not in pypi
    assert "uv publish" in pypi
    assert "needs: [verify, publish-pypi]" in github
    assert "contents: write" in github
    assert "id-token: write" not in github
    assert 'tags: ["python-v*"]' in content
    assert content.count("validate-tag --component python") == 3
    for variable in (
        "UV_CACHE_DIR",
        "UV_PYTHON_INSTALL_DIR",
        "UV_TOOL_DIR",
        "UV_TOOL_BIN_DIR",
        "XDG_CACHE_HOME",
    ):
        assert f"{variable}:" in content


def read_workflow(name: str) -> str:
    return (REPOSITORY / ".github" / "workflows" / name).read_text()


def assert_pinned_and_cacheless(content: str) -> None:
    references = re.findall(r"uses:\s+([^@\s]+)@([^\s#]+)", content)
    assert references
    assert all(re.fullmatch(r"[0-9a-f]{40}", reference) for _action, reference in references)
    cache_settings = {
        "astral-sh/setup-uv": "enable-cache: false",
        "actions/setup-node": "package-manager-cache: false",
        "oven-sh/setup-bun": "no-cache: true",
    }
    for action, setting in cache_settings.items():
        steps = re.findall(rf"uses: {re.escape(action)}@.*\n(?:\s+(?!- ).*\n)*", content)
        assert all(setting in step for step in steps), action


def workflow_step(job: str, name: str) -> str:
    match = re.search(rf"(?ms)^      - name: {re.escape(name)}\n(?P<body>.*?)(?=^      - |\Z)", job)
    assert match is not None, name
    return match.group("body")


def test_rust_release_workflow_publishes_verified_crate_with_split_permissions() -> None:
    content = read_workflow("release-rust.yml")
    assert_pinned_and_cacheless(content)
    verify = job_section(content, "verify")
    platform_checks = job_section(content, "platform-checks")
    crates = job_section(content, "publish-crates")
    github = job_section(content, "publish-github")
    assert 'tags: ["rust-v*"]' in content
    assert "validate-tag --component rust" in workflow_step(
        verify, "Validate annotated tag and ancestry"
    )
    metadata = workflow_step(verify, "Validate crate metadata")
    assert '"$RELEASE_TAG" rust/Cargo.toml > "$RUNNER_TEMP/metadata"' in metadata
    assert "s/^version=/VERSION=/p" in metadata
    gates = workflow_step(verify, "Run full release gates")
    assert "cargo test --workspace --all-targets --all-features" in gates
    assert "python rust/scripts/package_qa.py" in workflow_step(
        verify, "Verify extracted crate archives"
    )
    assert "cargo publish -p flyrail --locked --dry-run" in workflow_step(
        verify, "Package and dry-run publish"
    )
    assert "id-token: write" not in verify
    assert "contents: write" not in verify
    assert "needs: verify" in platform_checks
    assert "runs-on: macos-15" in platform_checks
    assert "needs: [verify, platform-checks]" in crates
    assert "environment: crates-io" in crates
    assert "id-token: write" in crates
    assert "contents: write" not in crates
    unpublished = "if: steps.existing.outputs.published == 'false'"
    repackage = workflow_step(crates, "Repackage and compare with the verified crate")
    assert unpublished in repackage
    assert (
        'cmp "target/package/flyrail-${VERSION}.crate" \\\n'
        '            "../release-bundle/dist/flyrail-${VERSION}.crate"'
    ) in repackage
    auth = workflow_step(crates, "Authenticate with crates.io trusted publishing")
    assert f"{unpublished} && env.HAS_REGISTRY_TOKEN != 'true'" in auth
    assert "rust-lang/crates-io-auth-action@" in auth
    upload = workflow_step(crates, "Publish to crates.io")
    assert unpublished in upload
    assert "steps.auth.outputs.token || secrets.CARGO_REGISTRY_TOKEN" in upload
    assert "cargo publish -p flyrail --locked --no-verify" in upload
    confirm = workflow_step(crates, "Confirm crates.io serves the verified crate")
    assert "if:" not in confirm
    assert "['version']['checksum']" in confirm
    assert "needs: [verify, publish-crates]" in github
    assert "contents: write" in github
    assert "id-token: write" not in github
    publish = workflow_step(github, "Publish Rust GitHub release")
    assert "validate-tag --component rust" in workflow_step(
        github, "Revalidate trusted publication code"
    )
    assert '"Flyrail for Rust $VERSION"' in publish
    assert '"$RELEASE_COMMIT" \\\n            release-bundle/dist/* "${args[@]}"' in publish


def test_swift_release_workflow_runs_swift_gates_and_publishes_notes() -> None:
    content = read_workflow("release-swift.yml")
    assert_pinned_and_cacheless(content)
    verify = job_section(content, "verify")
    github = job_section(content, "publish-github")
    assert 'tags: ["v*"]' in content
    assert "runs-on: macos-26" in verify
    assert "validate-tag --component swift" in workflow_step(
        verify, "Validate annotated tag and ancestry"
    )
    assert '"$RELEASE_TAG" Package.swift >> "$GITHUB_OUTPUT"' in workflow_step(
        verify, "Validate package metadata"
    )
    assert "uses: ./.github/actions/setup-swift-tools" in verify
    gates = "\n".join(
        workflow_step(verify, name)
        for name in ("Run full release gates", "Run integration gates", "Verify extracted package")
    )
    for gate in (
        "check_case_folding.py",
        "swiftformat --lint",
        "swiftlint lint --strict",
        "swift build",
        "swift test",
        "check_differential.py",
        "check_native_interop.py",
        "check_transaction_interop.py",
        "packagedHookRuntimeExecutesSharedHarnesses",
        "swift/scripts/package_qa.py",
    ):
        assert gate in gates
    assert "contents: write" not in verify
    assert "id-token: write" not in content
    assert "needs: verify" in github
    assert "contents: write" in github
    assert "validate-tag --component swift" in workflow_step(
        github, "Revalidate trusted publication code"
    )
    publish = workflow_step(github, "Publish Swift GitHub release")
    assert '"Flyrail for Swift $VERSION"' in publish
    assert '"$RELEASE_COMMIT" \\\n            "${args[@]}"' in publish


def test_swift_release_runs_integration_gates_before_pushing_the_tag() -> None:
    config = json.loads((REPOSITORY / "swift/scripts/release.json").read_text())
    assert ["just", "-f", "swift/Justfile", "check-integration"] in config["checks"]
    assert "python/.venv/bin/python" in config["tools"]
