from dataclasses import FrozenInstanceError

import pytest

from flyrail import BundleIdentity, SkillSpec


@pytest.mark.parametrize("identifier", ["a", "7", "team-tools", "x" * 64])
def test_bundle_identity_accepts_portable_identifiers(identifier: str) -> None:
    identity = BundleIdentity(identifier, "autumn release")

    assert identity.id == identifier
    assert identity.version == "autumn release"


@pytest.mark.parametrize(
    "identifier",
    ["", "x" * 65, "Team", "team_tools", "-team", "team-", "team--tools", "a/b", "a.b", "café"],
)
def test_bundle_identity_rejects_invalid_identifiers(identifier: str) -> None:
    with pytest.raises(ValueError, match="bundle id must be"):
        BundleIdentity(identifier, "1")


@pytest.mark.parametrize("identifier", ["con", "prn", "aux", "nul", "com1", "lpt9"])
def test_bundle_identity_rejects_reserved_device_names(identifier: str) -> None:
    with pytest.raises(ValueError, match="reserved Windows name"):
        BundleIdentity(identifier, "1")


@pytest.mark.parametrize(
    "version", ["1", "0.0.1", "next release", "  exact label  ", "秋", "cafe\u0301 \U0001f680"]
)
def test_bundle_versions_preserve_opaque_labels(version: str) -> None:
    identity = BundleIdentity("team", version)

    assert identity == BundleIdentity("team", version)
    assert identity.version == version


@pytest.mark.parametrize("version", ["", " ", "\n\t"])
def test_bundle_identity_rejects_blank_versions(version: str) -> None:
    with pytest.raises(ValueError, match="version must be nonblank"):
        BundleIdentity("team", version)


@pytest.mark.parametrize("version", ["\ud800", "release\udfff"])
def test_bundle_identity_rejects_non_scalar_versions(version: str) -> None:
    with pytest.raises(ValueError, match="version must contain only Unicode scalar values"):
        BundleIdentity("team", version)


def test_bundle_identity_rejects_incorrect_value_types() -> None:
    with pytest.raises(TypeError, match="bundle id must be a string"):
        BundleIdentity(5, "1")  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="version must be a string"):
        BundleIdentity("team", 1)  # type: ignore[arg-type]


def test_bundle_identity_is_immutable_and_hashable() -> None:
    identity = BundleIdentity("team", "1")

    assert {identity: "installed"}[BundleIdentity("team", "1")] == "installed"
    with pytest.raises(FrozenInstanceError):
        identity.version = "2"  # type: ignore[misc]


def test_skill_spec_snapshots_executable_intent() -> None:
    executables = ["scripts/run.py", "scripts/工具.sh"]
    skill = SkillSpec("review", "skills/review", executables)
    executables.append("later.py")

    assert skill.name == "review"
    assert skill.path == "skills/review"
    assert skill.executables == ("scripts/run.py", "scripts/工具.sh")
    assert skill == SkillSpec("review", "skills/review", ("scripts/run.py", "scripts/工具.sh"))
    assert {skill: "ready"}[skill] == "ready"
    with pytest.raises(FrozenInstanceError):
        skill.path = "elsewhere/review"  # type: ignore[misc]


def test_skill_spec_supports_a_root_skill_and_an_executable_generator() -> None:
    skill = SkillSpec("review", "review", (name for name in ["run.py"]))

    assert skill.path == "review"
    assert skill.executables == ("run.py",)
    assert SkillSpec("review", "review").executables == ()


def test_skill_spec_validates_name_and_containing_directory() -> None:
    with pytest.raises(ValueError, match="skill name must be"):
        SkillSpec("Review", "skills/Review")
    with pytest.raises(ValueError, match="must match its containing directory"):
        SkillSpec("review", "skills/another")


@pytest.mark.parametrize(
    "path",
    [
        "",
        "/review",
        "../review",
        "./review",
        "skills//review",
        "skills/review/",
        "skills/../review",
        "skills\\review",
        "C:/review",
        "C:review",
        "bad\x00/review",
        "bad\n/review",
        "bad?/review",
        "bad*/review",
        "bad</review",
        "bad>/review",
        'bad"/review',
        "bad|/review",
        "bad./review",
        "bad /review",
        "CON/review",
        "CON .txt/review",
        "LPT1 .log/review",
        "COM1 .exe/review",
        "aux.txt/review",
        "LPT¹/review",
        "conout$/review",
        "cafe\u0301/review",
        "\ud800/review",
    ],
)
def test_skill_spec_rejects_nonportable_skill_paths(path: str) -> None:
    with pytest.raises(ValueError, match="skill path"):
        SkillSpec("review", path)


@pytest.mark.parametrize(
    "path",
    ["../run.sh", "/run.sh", "scripts\\run.sh", "NUL.txt", "CON .txt", "LPT1 .log", "COM1 .exe"],
)
def test_skill_spec_validates_every_executable_path(path: str) -> None:
    with pytest.raises(ValueError, match="executable path"):
        SkillSpec("review", "skills/review", [path])


def test_skill_spec_accepts_spaces_before_extensions_in_non_device_paths() -> None:
    skill = SkillSpec("review", "skills/normal .txt/review", ["scripts/normal .txt"])

    assert skill.path == "skills/normal .txt/review"
    assert skill.executables == ("scripts/normal .txt",)


@pytest.mark.parametrize(
    "executables",
    [("run.py", "run.py"), ("scripts/run.py", "SCRIPTS/RUN.PY"), ("straße.py", "strasse.py")],
)
def test_skill_spec_rejects_colliding_executable_paths(executables: tuple[str, ...]) -> None:
    with pytest.raises(ValueError, match="executable paths must be unique"):
        SkillSpec("review", "skills/review", executables)


def test_skill_spec_rejects_incorrect_value_types() -> None:
    with pytest.raises(TypeError, match="skill path must be a string"):
        SkillSpec("review", 1)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="executables must be an iterable"):
        SkillSpec("review", "review", "run.py")
    with pytest.raises(TypeError, match="executables must be an iterable"):
        SkillSpec("review", "review", b"run.py")  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="executable path must be a string"):
        SkillSpec("review", "review", [1])  # type: ignore[list-item]
