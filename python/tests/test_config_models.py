import datetime as dt
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from flyrail import (
    Agent,
    ArrayValue,
    Audience,
    BundleEntry,
    Claim,
    Command,
    Dependency,
    DocumentFormat,
    EnvRef,
    Family,
    FileContent,
    FileMode,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HttpTransport,
    InstructionArtifact,
    Key,
    McpArtifact,
    Member,
    NativeArtifact,
    Notice,
    NoticeKind,
    ObjectValue,
    Ownership,
    Placement,
    Platform,
    Position,
    RenderedArtifact,
    RenderedBundle,
    Scalar,
    SectionBoundaries,
    SectionContent,
    Selector,
    SkillArtifact,
    StructuredContent,
    SupportAsset,
    Surface,
    Target,
    TargetScope,
    TreeContent,
    freeze_value,
    semantic_bytes,
    value_from_record,
)
from flyrail.rendered import dependency_order


def invalid(constructor: Callable[..., object], *args: object, **kwargs: object) -> object:
    return constructor(*args, **kwargs)


@pytest.mark.parametrize(
    "value,expected",
    [
        (None, b'["null"]'),
        (True, b'["boolean",true]'),
        (1, b'["integer","1"]'),
        (-0.0, b'["float","-0x0.0p+0"]'),
        (1.5, b'["float","0x1.8000000000000p+0"]'),
        ("hello", b'["string","hello"]'),
        (dt.date(2026, 1, 2), b'["date","2026-01-02"]'),
        (dt.time(1, 2, 3), b'["time","01:02:03.000000"]'),
        (dt.datetime(2026, 1, 2, 1, 2, 3), b'["datetime","2026-01-02T01:02:03.000000"]'),
        (
            dt.datetime(2026, 1, 2, 1, 2, 3, tzinfo=dt.timezone(dt.timedelta(hours=1))),
            b'["datetime","2026-01-02T00:02:03.000000+00:00"]',
        ),
    ],
)
def test_semantic_scalar_encoding_preserves_types(value: Any, expected: bytes) -> None:
    assert semantic_bytes(Scalar(value)) == expected


@pytest.mark.parametrize(
    "record",
    [
        ["null"],
        ["boolean", True],
        ["string", "hello"],
        ["integer", "-17"],
        ["float", "0x1.8000000000000p+0"],
        ["date", "2026-01-02"],
        ["time", "01:02:03.000000"],
        ["datetime", "2026-01-02T01:02:03.000000+00:00"],
        ["array", [["null"], ["integer", "1"]]],
        ["object", [["a", ["boolean", True]]]],
    ],
)
def test_semantic_records_round_trip_without_type_loss(record: list[object]) -> None:
    import json

    assert json.loads(semantic_bytes(value_from_record(record))) == record


@pytest.mark.parametrize(
    "record",
    [
        {},
        [],
        [1],
        ["null", None, None],
        ["object", [[]]],
        ["boolean", 1],
        ["integer", "01"],
        ["unknown", "x"],
        ["date", "20260102"],
        ["object", [["x", ["null"]], ["x", ["null"]]]],
    ],
)
def test_semantic_records_reject_ambiguous_encodings(record: object) -> None:
    with pytest.raises(ValueError):
        value_from_record(record)


def test_semantic_equality_does_not_coerce_python_scalars() -> None:
    assert len({Scalar(True), Scalar(1), Scalar(1.0), Scalar("1")}) == 4
    assert Scalar(0.0) != Scalar(-0.0)
    assert Scalar(True) != object()
    assert Scalar(dt.datetime(2026, 1, 1, tzinfo=dt.UTC)) == Scalar(
        dt.datetime(2026, 1, 1, 1, tzinfo=dt.timezone(dt.timedelta(hours=1)))
    )
    assert (
        semantic_bytes(freeze_value({"b": [1, True], "a": None}))
        == b'["object",[["a",["null"]],["b",["array",[["integer","1"],["boolean",true]]]]]]'
    )


def test_semantic_collections_snapshot_mutable_inputs() -> None:
    raw: dict[str, object] = {"numbers": [1, 2]}
    frozen = freeze_value(raw)
    raw["numbers"] = [3]
    assert frozen == ObjectValue({"numbers": ArrayValue([Scalar(1), Scalar(2)])})
    assert freeze_value(frozen) is frozen
    assert ObjectValue([("b", Scalar(2)), ("a", Scalar(1))]) == ObjectValue(
        {"a": Scalar(1), "b": Scalar(2)}
    )


def test_scalar_freezes_mutable_timezone_offsets() -> None:
    class MutableTimezone(dt.tzinfo):
        offset: dt.timedelta | None = dt.timedelta(hours=2)

        def utcoffset(self, date: dt.datetime | None) -> dt.timedelta | None:
            return self.offset

        def dst(self, date: dt.datetime | None) -> dt.timedelta | None:
            return None

        def tzname(self, date: dt.datetime | None) -> str | None:
            return "fixture"

    zone = MutableTimezone()
    original = dt.datetime(2026, 1, 2, 12, tzinfo=zone)
    snapshot = Scalar(original)
    before = semantic_bytes(snapshot)
    zone.offset = dt.timedelta(hours=7)
    assert semantic_bytes(snapshot) == before == b'["datetime","2026-01-02T10:00:00.000000+00:00"]'
    zone.offset = None
    local = Scalar(original)
    zone.offset = dt.timedelta(hours=3)
    assert local == Scalar(dt.datetime(2026, 1, 2, 12))
    assert semantic_bytes(local) == b'["datetime","2026-01-02T12:00:00.000000"]'


@pytest.mark.parametrize(
    "value", [float("nan"), float("inf"), 2**63, -(2**63) - 1, "\ud800", dt.time(tzinfo=dt.UTC)]
)
def test_rejects_nonportable_scalar_values(value: Any) -> None:
    with pytest.raises(ValueError):
        Scalar(value)


@pytest.mark.parametrize("value", [b"x", set(), object()])
def test_rejects_unsupported_value_types(value: Any) -> None:
    with pytest.raises(TypeError):
        Scalar(value)
    with pytest.raises(TypeError):
        freeze_value(value)


def test_object_value_rejects_duplicate_or_invalid_members() -> None:
    with pytest.raises(ValueError, match="duplicate"):
        ObjectValue([("name", Scalar("one")), ("name", Scalar("two"))])
    with pytest.raises(TypeError, match="string"):
        ObjectValue({1: Scalar("bad")})  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="values"):
        ArrayValue(["mutable"])  # type: ignore[list-item]


def test_tree_snapshot_infers_parents_and_preserves_empty_directories() -> None:
    entries = [BundleEntry("scripts/run", b"run", True), BundleEntry("empty")]
    tree = TreeContent(entries)
    entries.clear()
    assert tree.entries == (
        BundleEntry("empty"),
        BundleEntry("scripts"),
        BundleEntry("scripts/run", b"run", True),
    )


@pytest.mark.parametrize(
    "entries",
    [
        [BundleEntry("a"), BundleEntry("a")],
        [BundleEntry("a/x", b"x"), BundleEntry("A/y", b"y")],
        [BundleEntry("a", b"file"), BundleEntry("a/b", b"child")],
    ],
)
def test_tree_rejects_ambiguous_entries(entries: list[BundleEntry]) -> None:
    with pytest.raises(ValueError, match=r"collision|duplicate"):
        TreeContent(entries)


def test_commands_snapshot_literal_arguments_and_environment_references() -> None:
    argv = ["python", "space and 'quotes'", "$(not-executed)"]
    env: dict[str, str | EnvRef] = {"TOKEN": EnvRef("SOURCE_TOKEN"), "MODE": "quiet"}
    command = Command(argv, env, "support")
    argv.clear()
    env.clear()
    assert command.argv == ("python", "space and 'quotes'", "$(not-executed)")
    assert command.env == (("MODE", "quiet"), ("TOKEN", EnvRef("SOURCE_TOKEN")))
    assert command.cwd == "support"


def test_custom_section_boundaries_are_physical_selectors() -> None:
    boundaries = SectionBoundaries("<!-- >>> owned >>> -->", "<!-- <<< owned <<< -->")
    first = SectionContent("first", "text", boundaries)
    second = SectionContent("second", "text", boundaries)
    assert first.selector == second.selector
    assert Claim(Ownership.SECTION, first.selector) == Claim(Ownership.SECTION, second.selector)
    assert SectionContent("guide", "text").selector == SectionBoundaries(
        "<!-- flyrail:guide:start -->", "<!-- flyrail:guide:end -->"
    )
    assert Claim(Ownership.SECTION, "guide").selector == SectionContent("guide", "text").selector


def test_identifiable_array_members_and_placement() -> None:
    member = Member(Scalar("review"), ["name"])
    selector = Selector([Key("hooks"), member])
    value = freeze_value({"name": "review", "command": "check"})
    content = StructuredContent(
        DocumentFormat.JSONC,
        selector,
        value,
        Placement(Position.BEFORE, Member(Scalar("later"), ["name"])),
    )
    assert content.selector.parts == (Key("hooks"), member)
    assert content.value == value
    assert Member(value).key == ()


@pytest.mark.parametrize("position", [Position.FIRST, Position.LAST])
@pytest.mark.parametrize("anchor", ["invalid", [], {}, Member(Scalar("anchor"))])
def test_first_last_placement_requires_no_anchor(position: Position, anchor: Any) -> None:
    with pytest.raises(ValueError, match="anchor"):
        Placement(position, anchor)


@pytest.mark.parametrize("position", [Position.BEFORE, Position.AFTER])
@pytest.mark.parametrize("anchor", [None, "invalid", [], {}])
def test_before_after_placement_requires_member(position: Position, anchor: Any) -> None:
    with pytest.raises(ValueError, match="anchor"):
        Placement(position, anchor)


def test_placement_rejects_member_subclasses() -> None:
    class MutableMember(Member):
        pass

    with pytest.raises(ValueError, match="anchor"):
        Placement(Position.AFTER, MutableMember(Scalar("anchor")))


def test_native_audiences_keep_product_surface_and_platform_explicit() -> None:
    cursor = Audience(Agent.CURSOR, TargetScope.PROJECT, Surface.IDE, Platform.WINDOWS)
    copilot = Audience(Agent.COPILOT, TargetScope.PROJECT, Surface.VSCODE)
    assert (
        NativeArtifact(
            "guide",
            Family.INSTRUCTIONS,
            cursor,
            "rules/guide.md",
            FileContent(b"hi", FileMode.READABLE),
        ).audience
        == cursor
    )
    assert copilot.surface == Surface.VSCODE


def test_target_captures_supplied_home_and_relocation_without_secrets(tmp_path: Path) -> None:
    home = tmp_path / "home"
    env = {"XDG_CONFIG_HOME": str(tmp_path / "config"), "TOKEN": "private"}
    target = Target.user("opencode", home=home, env=env)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "changed")
    assert target.home == home
    assert target.environment == (("XDG_CONFIG_HOME", str(tmp_path / "config")),)
    assert target.root == tmp_path / "config/opencode/skills"


@pytest.mark.parametrize(
    "factory",
    [
        lambda: invalid(Key, ""),
        lambda: invalid(Member, Scalar("x"), "name"),
        lambda: invalid(Member, ArrayValue([]), ["name"]),
        lambda: invalid(Selector, []),
        lambda: invalid(Selector, [0]),
        lambda: invalid(Placement, "last"),
        lambda: invalid(Placement, Position.BEFORE),
        lambda: invalid(Placement, Position.FIRST, Member(Scalar("x"))),
        lambda: invalid(FileContent, bytearray(b"x")),
        lambda: invalid(FileContent, b"x", 0o644),
        lambda: invalid(TreeContent, ["x"]),
        lambda: invalid(SectionContent, "x", "\0"),
        lambda: invalid(SectionContent, "x", "<!-- flyrail:x:start -->"),
        lambda: invalid(SectionContent, "x", "hi", ("start", "end")),
        lambda: invalid(SectionContent, "x", "start\n", SectionBoundaries("start", "end")),
        lambda: invalid(SectionBoundaries, "same", "same"),
        lambda: invalid(SectionBoundaries, "\n", "end"),
        lambda: invalid(StructuredContent, "json", Selector([Key("x")]), Scalar(1)),
        lambda: invalid(
            StructuredContent, DocumentFormat.JSON, Selector([Key("x")]), Scalar(1), "last"
        ),
        lambda: invalid(
            StructuredContent, DocumentFormat.JSON, Selector([Key("x")]), Scalar(1), Placement()
        ),
        lambda: invalid(
            StructuredContent,
            DocumentFormat.JSON,
            Selector([Member(Scalar("x"))]),
            Scalar(1),
            Placement(Position.BEFORE, Member(Scalar("x"))),
        ),
        lambda: invalid(Audience, "codex", TargetScope.PROJECT),
        lambda: invalid(Audience, Agent.CODEX, TargetScope.DIRECTORY),
        lambda: invalid(Audience, Agent.CODEX, TargetScope.PROJECT, "cli"),
        lambda: invalid(Audience, Agent.CODEX, TargetScope.PROJECT, platform="linux"),
        lambda: invalid(Audience, Agent.CODEX, TargetScope.PROJECT, Surface.IDE),
        lambda: invalid(Audience, Agent.CURSOR, TargetScope.PROJECT, Surface.VSCODE),
        lambda: invalid(EnvRef, "BAD-NAME"),
        lambda: invalid(EnvRef, 1),
        lambda: invalid(Command, "python"),
        lambda: invalid(Command, []),
        lambda: invalid(Command, [""]),
        lambda: invalid(Command, ["\0"]),
        lambda: invalid(Command, [1]),
        lambda: invalid(Command, ["x"], [("A", "x"), ("A", "y")]),
        lambda: invalid(Command, ["x"], {"A": "\0"}),
        lambda: invalid(HttpTransport, "stdio:x"),
        lambda: invalid(HttpTransport, "https://user:pass@example.test/x"),
        lambda: invalid(HttpTransport, "https://example.test/a b"),
        lambda: invalid(HttpTransport, "https://example.test", "TOKEN"),
        lambda: invalid(SkillArtifact, "x", "x", "tree"),
        lambda: invalid(SkillArtifact, "x", "x", TreeContent([])),
        lambda: invalid(InstructionArtifact, "x", "\0"),
        lambda: invalid(McpArtifact, "x", "x", "command"),
        lambda: invalid(HookArtifact, "x", "stop", Command(["x"])),
        lambda: invalid(HookArtifact, "x", HookEvent.STOP, Command(["x"]), True),
        lambda: invalid(HookArtifact, "x", HookEvent.STOP, Command(["x"]), outcomes=[]),
        lambda: invalid(
            HookArtifact, "x", HookEvent.STOP, Command(["x"]), outcomes=[HookOutcome.CONTINUE] * 2
        ),
        lambda: invalid(HookArtifact, "x", HookEvent.STOP, Command(["x"]), tools="tool"),
        lambda: invalid(HookArtifact, "x", HookEvent.BEFORE_TOOL, Command(["x"]), tools=[""]),
        lambda: invalid(HookArtifact, "x", HookEvent.BEFORE_TOOL, Command(["x"]), tools=["x", "x"]),
        lambda: invalid(HookArtifact, "x", HookEvent.STOP, Command(["x"]), tools=["x"]),
        lambda: invalid(
            NativeArtifact,
            "x",
            "settings",
            Audience(Agent.CODEX, TargetScope.PROJECT),
            "x",
            FileContent(b""),
        ),
        lambda: invalid(
            NativeArtifact,
            "x",
            Family.INSTRUCTIONS,
            Audience(Agent.CODEX, TargetScope.PROJECT),
            "x",
            {},
        ),
        lambda: invalid(SupportAsset, "x", Family.HOOKS, "tree"),
        lambda: invalid(Dependency, "x", "x"),
    ],
)
def test_closed_models_reject_invalid_input(factory: Callable[[], Any]) -> None:
    with pytest.raises((ValueError, TypeError)):
        factory()


def test_rendered_snapshot_hashes_destinations_modes_and_dependency_order(tmp_path: Path) -> None:
    support = RenderedArtifact(
        "support", Family.HOOKS, tmp_path / "support", FileContent(b"run", FileMode.EXECUTABLE)
    )
    hook = RenderedArtifact("hook", Family.HOOKS, tmp_path / "hook.json", FileContent(b"reference"))
    notice = Notice("hook", NoticeKind.PREREQUISITE, "Requires adapter 1.0")
    first = RenderedBundle([hook, support], [Dependency("hook", "support")], [notice])
    second = RenderedBundle([support, hook], [Dependency("hook", "support")])
    assert first.content_digest == second.content_digest
    assert dependency_order(["hook", "support"], first.dependencies) == ("support", "hook")
    assert first.notices == (notice,)
    changed = RenderedArtifact("hook", Family.HOOKS, tmp_path / "other.json", hook.content)
    assert (
        RenderedBundle([changed, support], first.dependencies).content_digest
        != first.content_digest
    )


@pytest.mark.parametrize(
    "factory",
    [
        lambda: invalid(RenderedArtifact, "x", "instructions", Path("/x"), FileContent(b"x")),
        lambda: invalid(
            RenderedArtifact, "x", Family.INSTRUCTIONS, Path("relative"), FileContent(b"x")
        ),
        lambda: invalid(
            RenderedArtifact, "x", Family.INSTRUCTIONS, Path("/x/../other"), FileContent(b"x")
        ),
        lambda: invalid(Notice, "x", "activation", "message"),
        lambda: invalid(Notice, "x", NoticeKind.ACTIVATION, ""),
        lambda: invalid(RenderedBundle, ["x"]),
        lambda: invalid(RenderedBundle, [], notices=["x"]),
        lambda: invalid(
            RenderedBundle, [RenderedArtifact("x", Family.HOOKS, Path("/x"), FileContent(b"x"))] * 2
        ),
        lambda: invalid(dependency_order, ["x"], ["y"]),
    ],
)
def test_rendered_values_reject_invalid_input(factory: Callable[[], Any]) -> None:
    with pytest.raises((ValueError, TypeError)):
        factory()
