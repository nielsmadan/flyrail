import importlib.util
import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
from types import ModuleType

import pytest

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent


def load_script(name: str) -> ModuleType:
    return load_path(ROOT / "scripts" / name)


def load_path(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(path.stem, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


workflow = load_script("release_workflow.py")


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
    assert content.count("curtains ci validate-tag --config python/scripts/release.json") == 3
    assert_curtains_jobs_install_the_action(content)
    publish = workflow_step(github, "Publish Python GitHub release")
    assert "args=(--not-latest)" in publish
    assert "Release $RELEASE_TAG requires at least one asset" in publish
    assert (
        'curtains ci publish --config python/scripts/release.json "${args[@]}" \\\n'
        '            "$RELEASE_TAG" "$RELEASE_COMMIT" \\\n'
        '            --notes release-bundle/release-notes.md "${assets[@]}"'
    ) in publish
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


def assert_curtains_jobs_install_the_action(content: str) -> None:
    jobs = re.findall(r"(?ms)^  [a-z][a-z0-9-]*:\n.*?(?=^  [a-z][a-z0-9-]*:\n|\Z)", content)
    curtains_jobs = [job for job in jobs if "curtains ci " in job]
    assert curtains_jobs
    for job in curtains_jobs:
        action = job.find("uses: nielsmadan/curtaincall@")
        assert 0 <= action < job.index("curtains ci ")


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
    assert "curtains ci validate-tag --config rust/scripts/release.json" in workflow_step(
        verify, "Validate annotated tag and ancestry"
    )
    assert_curtains_jobs_install_the_action(content)
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
    assert "curtains ci validate-tag --config rust/scripts/release.json" in workflow_step(
        github, "Revalidate trusted publication code"
    )
    assert "args=(--not-latest)" in publish
    assert "Release $RELEASE_TAG requires at least one asset" in publish
    assert (
        'curtains ci publish --config rust/scripts/release.json "${args[@]}" \\\n'
        '            "$RELEASE_TAG" "$RELEASE_COMMIT" \\\n'
        '            --notes release-bundle/release-notes.md "${assets[@]}"'
    ) in publish


def test_swift_release_workflow_runs_swift_gates_and_publishes_notes() -> None:
    content = read_workflow("release-swift.yml")
    assert_pinned_and_cacheless(content)
    verify = job_section(content, "verify")
    github = job_section(content, "publish-github")
    assert 'tags: ["v*"]' in content
    assert "runs-on: macos-26" in verify
    assert "curtains ci validate-tag --config swift/scripts/release.json" in workflow_step(
        verify, "Validate annotated tag and ancestry"
    )
    assert_curtains_jobs_install_the_action(content)
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
    assert "curtains ci validate-tag --config swift/scripts/release.json" in workflow_step(
        github, "Revalidate trusted publication code"
    )
    publish = workflow_step(github, "Publish Swift GitHub release")
    assert "args=(--not-latest)" in publish
    assert publish.endswith(
        'curtains ci publish --config swift/scripts/release.json "${args[@]}" \\\n'
        '            "$RELEASE_TAG" "$RELEASE_COMMIT" \\\n'
        "            --notes release-bundle/release-notes.md\n"
    )


def test_swift_release_runs_integration_gates_before_pushing_the_tag() -> None:
    config = json.loads((REPOSITORY / "swift/scripts/release.json").read_text())
    assert ["just", "-f", "swift/Justfile", "check-integration"] in config["checks"]
    assert "python/.venv/bin/python" in config["tools"]
