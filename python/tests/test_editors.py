import datetime as dt
import json
import sys
import tomllib
from collections.abc import Callable
from dataclasses import FrozenInstanceError, replace
from typing import Any

import pytest

from flyrail import (
    Acquisition,
    ArrayPosition,
    Claim,
    CreatedContainer,
    Disposition,
    DocumentEdit,
    DocumentFormat,
    DocumentProvenance,
    DocumentRequest,
    EditStatus,
    Key,
    Member,
    Ownership,
    Placement,
    Position,
    Scalar,
    SectionBoundaries,
    SectionContent,
    SelectionSnapshot,
    Selector,
    StructuredContent,
    edit_document,
    freeze_value,
)

FORMATS = list(DocumentFormat)


def invalid(function: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
    return function(*args, **kwargs)


def structured(
    format: DocumentFormat, value: object, *parts: str | Member, placement: Placement | None = None
) -> StructuredContent:
    return StructuredContent(
        format,
        Selector(Key(part) if isinstance(part, str) else part for part in parts),
        freeze_value(value),
        placement,
    )


def set_content(
    data: bytes | None,
    content: SectionContent | StructuredContent,
    *,
    acquisition: Acquisition = Acquisition.CONFLICT,
) -> DocumentEdit:
    return edit_document(data, [DocumentRequest.set(content, acquisition=acquisition)])


def remove(edit: DocumentEdit, data: bytes | None = None) -> DocumentEdit:
    return edit_document(
        edit.after if data is None else data,
        [DocumentRequest.retire(item.claim) for item in edit.ownership],
        ownership=edit.ownership,
        provenance=edit.provenance,
    )


def update(
    edit: DocumentEdit, content: SectionContent | StructuredContent, *, data: bytes | None = None
) -> DocumentEdit:
    return edit_document(
        edit.after if data is None else data,
        [DocumentRequest.set(content)],
        ownership=edit.ownership,
        provenance=edit.provenance,
    )


@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"])
@pytest.mark.parametrize("prefix", ["", "foreign", "foreign\n", "foreign\n\n", "foreign\n\n\n"])
@pytest.mark.parametrize("body", ["", "hello", "hello\n", "hello\n\n", " a \n b \r\n"])
def test_sections_preserve_foreign_bytes(newline: str, prefix: str, body: str) -> None:
    raw = ("header" + newline + prefix.replace("\n", newline)).encode()
    content = SectionContent("guide", body)
    installed = set_content(raw, content)
    assert installed.applicable
    assert installed.after is not None and installed.after.startswith(raw)
    assert remove(installed).after == raw
    assert installed.ownership[0].disposition is Disposition.CREATED


@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"])
@pytest.mark.parametrize("retired", [("b",), ("a", "b"), ("b", "a")])
def test_section_retirement_preserves_adjacent_sections_and_suffix(
    newline: str, retired: tuple[str, ...]
) -> None:
    prefix = ("Header" + newline).encode()
    suffix = ("Üser suffix" + newline).encode()
    installed = edit_document(
        prefix,
        [DocumentRequest.set(SectionContent(name, name)) for name in ("a", "b")],
    )
    assert installed.after is not None
    observed = (
        installed.after.replace(
            ("<!-- flyrail:a:end -->" + newline * 2).encode(),
            ("<!-- flyrail:a:end -->" + newline).encode(),
        )
        + suffix
    )
    result = edit_document(
        observed,
        [
            DocumentRequest.retire(Claim(Ownership.SECTION, SectionContent(name, name).selector))
            for name in retired
        ],
        ownership=installed.ownership,
        provenance=installed.provenance,
    )
    assert result.applicable
    assert [item.status for item in result.observations] == [EditStatus.REMOVED] * len(retired)
    if len(retired) == 2:
        assert result.after == prefix + suffix
        assert result.ownership == ()
    else:
        first = ("<!-- flyrail:a:start -->\na\n<!-- flyrail:a:end -->\n").replace("\n", newline)
        assert result.after == prefix + newline.encode() + first.encode() + suffix
        current = update(result, SectionContent("a", "a"))
        assert current.observations[0].status is EditStatus.CURRENT
        assert current.after == result.after


@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"])
def test_section_retirement_preserves_an_unowned_standard_neighbor(newline: str) -> None:
    foreign = "<!-- flyrail:foreign:start -->\nUser text\n<!-- flyrail:foreign:end -->\n".replace(
        "\n", newline
    ).encode()
    installed = set_content(foreign, SectionContent("owned", "generated"))
    assert installed.after is not None
    observed = installed.after.replace(
        ("<!-- flyrail:foreign:end -->" + newline * 2).encode(),
        ("<!-- flyrail:foreign:end -->" + newline).encode(),
    )
    result = remove(installed, observed)
    assert result.applicable
    assert result.after == foreign


@pytest.mark.parametrize("prefix_newline", [b"\n", b"\r\n", b"\r"])
def test_section_bom_takeover_update_rename_and_restore(prefix_newline: bytes) -> None:
    pair = SectionBoundaries("<!-- splashdown:start -->", "<!-- splashdown:end -->")
    prefix = b"\xef\xbb\xbfUser" + prefix_newline * 2
    raw = prefix + b"<!-- splashdown:start -->\r\n  old \r\n\r\n<!-- splashdown:end -->\r\nTAIL"
    content = SectionContent("old-attribution", "new\n", pair)
    conflict = set_content(raw, content)
    assert conflict.after == raw
    assert conflict.observations[0].status is EditStatus.CONFLICT
    first = set_content(raw, content, acquisition=Acquisition.TAKEOVER)
    assert (
        first.after == prefix + b"<!-- splashdown:start -->\r\n"
        b"new\r\n<!-- splashdown:end -->\r\nTAIL"
    )
    second = update(first, SectionContent("renamed", "newer\n", pair))
    assert second.ownership[0].baseline == first.ownership[0].baseline
    assert second.observations[0].status is EditStatus.UPDATED
    assert remove(second).after == raw
    assert (
        update(second, SectionContent("renamed", "newer\n", pair)).observations[0].status
        is EditStatus.CURRENT
    )


@pytest.mark.parametrize("change", ["body", "space", "missing", "newline"])
def test_modified_section_retains_baseline(change: str) -> None:
    original = b"Foreign\n<!-- flyrail:x:start -->\r\nold\r\n<!-- flyrail:x:end -->\r\n"
    first = set_content(original, SectionContent("x", "new\n"), acquisition=Acquisition.TAKEOVER)
    assert first.after is not None
    if change == "newline":
        data = first.after.replace(b"\r\n", b"\n")
    elif change == "missing":
        data = b"foreign\n"
    else:
        data = first.after.replace(b"new", b"edited" if change == "body" else b"new ")
    for result in [update(first, SectionContent("x", "new\n"), data=data), remove(first, data)]:
        assert result.after == data
        assert result.ownership == first.ownership
        assert result.provenance == first.provenance
        assert result.observations[0].status is EditStatus.MODIFIED


@pytest.mark.parametrize("prefix_newline", ["\n", "\r\n", "\r"])
@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"])
def test_adoption_is_exact_and_removes_without_a_baseline(
    prefix_newline: str, newline: str
) -> None:
    pair = SectionBoundaries("START", "END")
    content = SectionContent("x", "generated\n", pair)
    prefix = ("before" + prefix_newline).encode()
    suffix = ("after" + prefix_newline).encode()
    raw = prefix + ("START\ngenerated\nEND\n").replace("\n", newline).encode() + suffix
    adopted = set_content(raw, content, acquisition=Acquisition.ADOPT)
    assert adopted.after == raw
    assert adopted.ownership[0].disposition is Disposition.ADOPTED
    assert adopted.ownership[0].baseline is None
    repeated = update(adopted, content)
    assert repeated.observations[0].status is EditStatus.CURRENT
    assert repeated.after == raw
    assert repeated.ownership == adopted.ownership
    assert remove(repeated).after == prefix + suffix
    alternate = b"\n" if newline != "\n" else b"\r\n"
    for variant in [
        raw.replace(b"generated", b"generated "),
        raw.replace(b"generated", b"generated" + newline.encode()),
        raw.replace(b"generated", b"GENERATED"),
        raw.replace(b"generated" + newline.encode(), b"generated" + alternate),
        raw.replace(b"END" + newline.encode(), b"END" + alternate),
    ]:
        result = set_content(variant, content, acquisition=Acquisition.ADOPT)
        assert result.observations[0].status is EditStatus.CONFLICT
        assert result.after == variant


def test_sections_use_their_own_newlines_and_new_sections_use_the_document() -> None:
    a = SectionContent("a", "new", SectionBoundaries("A", "END A"))
    b = SectionContent("b", "keep", SectionBoundaries("B", "END B"))
    c = SectionContent("c", "created")
    raw = b"Header\nA\r\nold\r\nEND A\r\nB\rkeep\rEND B\rFooter\n"
    result = edit_document(
        raw,
        [
            DocumentRequest.set(a, acquisition=Acquisition.TAKEOVER),
            DocumentRequest.set(b, acquisition=Acquisition.ADOPT),
            DocumentRequest.set(c),
        ],
    )
    assert [item.status for item in result.observations] == [
        EditStatus.TAKEN_OVER,
        EditStatus.ADOPTED,
        EditStatus.CREATED,
    ]
    assert result.after == (
        b"Header\nA\r\nnew\r\nEND A\r\nB\rkeep\rEND B\rFooter\n\n"
        b"<!-- flyrail:c:start -->\ncreated\n<!-- flyrail:c:end -->\n"
    )
    assert remove(result).after == b"Header\nA\r\nold\r\nEND A\r\nFooter\n"


@pytest.mark.parametrize(
    "raw",
    [
        b"START\n",
        b"END\n",
        b"END\nSTART\n",
        b"START\nx\nSTART\nEND\n",
        b"START\nEND\nSTART\nEND\n",
        b"prefix START\nEND\n",
        b"START\nEND suffix\n",
        b"START\nOTHER\nEND\nSTOP\n",
        b"START\nOTHER\nSTOP\nEND\n",
        b"<!-- flyrail:bad marker:start -->\n",
    ],
)
def test_malformed_or_intersecting_sections_are_rejected(raw: bytes) -> None:
    requests = [
        DocumentRequest.set(SectionContent("x", "new", SectionBoundaries("START", "END"))),
        DocumentRequest.set(SectionContent("y", "other", SectionBoundaries("OTHER", "STOP"))),
    ]
    with pytest.raises(ValueError):
        edit_document(raw, requests)


def test_unrequested_native_sections_are_also_checked_for_intersection() -> None:
    raw = b"<!-- flyrail:foreign:start -->\nSTART\n<!-- flyrail:foreign:end -->\nEND\n"
    with pytest.raises(ValueError, match="section"):
        set_content(
            raw,
            SectionContent("x", "new", SectionBoundaries("START", "END")),
            acquisition=Acquisition.TAKEOVER,
        )


def test_sections_multiowner_creation_and_separators() -> None:
    a = set_content(None, SectionContent("a", "A"))
    b = update(a, SectionContent("b", "B"))
    c = edit_document(
        b.after,
        [DocumentRequest.retire(a.ownership[0].claim)],
        ownership=b.ownership,
        provenance=b.provenance,
    )
    assert c.after == b"\n<!-- flyrail:b:start -->\nB\n<!-- flyrail:b:end -->\n"
    assert c.provenance.empty_document == b""
    assert remove(c).after is None
    foreign = update(a, SectionContent("b", "B"), data=b"User\n" + (a.after or b""))
    final = remove(foreign)
    assert final.after == b"User\n"
    assert final.provenance == DocumentProvenance()


def test_separator_is_retained_when_no_longer_adjacent() -> None:
    first = set_content(b"foreign", SectionContent("x", "body"))
    assert first.after is not None
    data = first.after.replace(b"\n\n<!--", b"\n\nUSER\n<!--")
    result = remove(first, data)
    assert result.after == b"foreign\n\nUSER\n"


def test_section_resource_conflicts_are_atomic() -> None:
    a = set_content(None, SectionContent("a", "A"))
    assert a.after is not None
    data = a.after.replace(b"\nA\n", b"\nUSER\n")
    result = edit_document(
        data,
        [
            DocumentRequest.retire(a.ownership[0].claim),
            DocumentRequest.set(SectionContent("b", "B")),
        ],
        ownership=a.ownership,
        provenance=a.provenance,
    )
    assert result.after == data
    assert result.ownership == a.ownership
    assert not result.applicable


@pytest.mark.parametrize("format", FORMATS)
@pytest.mark.parametrize("raw", [None, b""])
def test_structured_create_nested_and_remove(format: DocumentFormat, raw: bytes | None) -> None:
    if raw == b"" and format is not DocumentFormat.TOML:
        with pytest.raises(ValueError):
            set_content(raw, structured(format, 1, "mcp", "server", "key"))
        return
    result = set_content(raw, structured(format, 1, "mcp", "server", "key"))
    assert result.applicable
    assert result.ownership[0].installed.value == Scalar(1)
    assert len(result.provenance.containers) == 2
    assert remove(result).after == raw


@pytest.mark.parametrize(
    "format,raw",
    [
        (DocumentFormat.JSON, b'{"foreign":1}'),
        (DocumentFormat.JSONC, b'\xef\xbb\xbf{\r\n // top\r\n "foreign" : 1,\r\n}\r\n'),
        (DocumentFormat.TOML, b"\xef\xbb\xbf# top\r\nforeign = 0x12 # keep\r\n"),
    ],
)
def test_foreign_structured_syntax_roundtrips(format: DocumentFormat, raw: bytes) -> None:
    first = set_content(
        raw, structured(format, {"command": "runner", "args": ["one", "two"]}, "mcp", "server")
    )
    assert first.applicable
    changed = update(
        first, structured(format, {"command": "runner", "args": ["three"]}, "mcp", "server")
    )
    assert changed.observations[0].status is EditStatus.UPDATED
    assert remove(changed).after == raw


@pytest.mark.parametrize(
    "format,raw",
    [
        (DocumentFormat.JSON, b'{"foreign":true,"mcp":{"server":{"command":"old"}}}'),
        (
            DocumentFormat.JSONC,
            b'{ /* top */ "mcp": { "server": {"command" : "old" /* owned */} }, "foreign":true }',
        ),
        (DocumentFormat.TOML, b"foreign = true\n[mcp.server] # header\ncommand = 'old' # owned\n"),
    ],
)
def test_takeover_keeps_first_syntax_baseline_across_updates(
    format: DocumentFormat, raw: bytes
) -> None:
    content = structured(format, {"command": "new"}, "mcp", "server")
    assert set_content(raw, content).observations[0].status is EditStatus.CONFLICT
    first = set_content(raw, content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable
    second = update(first, structured(format, {"command": "newer"}, "mcp", "server"))
    third = update(second, content)
    assert third.ownership[0].baseline == first.ownership[0].baseline
    assert remove(third).after == raw
    assert remove(third).ownership == ()


@pytest.mark.parametrize("format", FORMATS)
def test_structured_exact_adoption_has_removal_disposition(format: DocumentFormat) -> None:
    generated = set_content(None, structured(format, True, "mcp", "enabled"))
    adopted = set_content(
        generated.after, structured(format, True, "mcp", "enabled"), acquisition=Acquisition.ADOPT
    )
    assert adopted.after == generated.after
    assert adopted.ownership[0].baseline is None
    assert adopted.ownership[0].disposition is Disposition.ADOPTED
    removed = remove(adopted)
    assert removed.after is not None
    assert removed.ownership == ()
    mismatch = set_content(
        generated.after, structured(format, 1, "mcp", "enabled"), acquisition=Acquisition.ADOPT
    )
    assert mismatch.observations[0].status is EditStatus.CONFLICT


@pytest.mark.parametrize("format", FORMATS)
def test_modified_values_and_absent_owned_selections_retain_claims(format: DocumentFormat) -> None:
    first = set_content(None, structured(format, "installed", "x"))
    assert first.after is not None
    modified = first.after.replace(b"installed", b"user")
    result = remove(first, modified)
    assert result.after == modified
    assert result.ownership == first.ownership
    assert result.observations[0].status is EditStatus.MODIFIED
    absent = b"" if format is DocumentFormat.TOML else b"{}"
    missing = remove(first, absent)
    assert missing.ownership == first.ownership
    assert missing.observations[0].status is EditStatus.MODIFIED


@pytest.mark.parametrize(
    "format,initial,edited",
    [
        (DocumentFormat.JSONC, b'{"x":{"v":1}}', b'{"x":{"v":1 /* changed */}}'),
        (DocumentFormat.TOML, b"[x]\nv=1\n", b"[x]\nv=1 # changed\n"),
    ],
)
def test_comment_changes_inside_owned_units_are_modifications(
    format: DocumentFormat, initial: bytes, edited: bytes
) -> None:
    first = set_content(initial, structured(format, {"v": 1}, "x"), acquisition=Acquisition.ADOPT)
    result = remove(first, edited)
    assert result.observations[0].status is EditStatus.MODIFIED
    assert result.after == edited
    assert result.ownership == first.ownership


def test_json_whitespace_is_semantically_compared() -> None:
    first = set_content(
        b'{"x":{"v":1}}',
        structured(DocumentFormat.JSON, {"v": 1}, "x"),
        acquisition=Acquisition.ADOPT,
    )
    result = remove(first, b'{"x":{ "v" : 1 }}')
    assert result.after == b"{}"


@pytest.mark.parametrize(
    "format,raw,expected",
    [
        (DocumentFormat.JSONC, b'{"x": 1 /* foreign */}', b"{  /* foreign */}"),
        (DocumentFormat.TOML, b"x = 1 # foreign\n", b" # foreign\n"),
    ],
)
def test_key_trivia_is_foreign_even_after_adoption(
    format: DocumentFormat, raw: bytes, expected: bytes
) -> None:
    first = set_content(raw, structured(format, 1, "x"), acquisition=Acquisition.ADOPT)
    assert remove(first).after == expected


@pytest.mark.parametrize("format", FORMATS)
def test_created_containers_outlive_first_owner_but_foreign_empty_ones_remain(
    format: DocumentFormat,
) -> None:
    a = set_content(None, structured(format, "A", "mcp", "a"))
    b = update(a, structured(format, "B", "mcp", "b"))
    c = edit_document(
        b.after,
        [DocumentRequest.retire(a.ownership[0].claim)],
        ownership=b.ownership,
        provenance=b.provenance,
    )
    assert c.applicable and c.after is not None
    assert c.provenance == b.provenance
    assert remove(c).after is None
    empty = b"[mcp]\n" if format is DocumentFormat.TOML else b'{"mcp":{}}'
    foreign = set_content(empty, structured(format, "A", "mcp", "a"))
    assert foreign.provenance.containers == ()
    assert remove(foreign).after == empty


@pytest.mark.parametrize("format", [DocumentFormat.JSONC, DocumentFormat.TOML])
def test_foreign_comments_prevent_pruning_and_end_creation_provenance(
    format: DocumentFormat,
) -> None:
    first = set_content(None, structured(format, 1, "mcp", "x"))
    assert first.after is not None
    edited = (
        first.after.replace(b'"mcp":{', b'"mcp":{/* foreign */')
        if format is DocumentFormat.JSONC
        else first.after.replace(b"[mcp]\n", b"[mcp] # foreign\n")
    )
    result = remove(first, edited)
    assert result.applicable and result.after is not None
    assert b"foreign" in result.after
    assert result.provenance == DocumentProvenance()
    reinstall = set_content(result.after, structured(format, 2, "mcp", "x"))
    assert remove(reinstall).after == result.after


@pytest.mark.parametrize("format", FORMATS)
@pytest.mark.parametrize(
    "placement",
    [
        Placement(Position.FIRST),
        Placement(Position.LAST),
        Placement(Position.BEFORE, Member(Scalar("b"), ("name",))),
        Placement(Position.AFTER, Member(Scalar("a"), ("name",))),
    ],
)
def test_array_placements_preserve_foreign_values_and_order(
    format: DocumentFormat, placement: Placement
) -> None:
    initial = (
        b'hooks=[{name="a",x=1},{name="b",x=2}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"name":"a","x":1},{"name":"b","x":2}]}'
    )
    member = Member(Scalar("new"), ("name",))
    first = set_content(
        initial, structured(format, {"name": "new", "x": 3}, "hooks", member, placement=placement)
    )
    assert first.applicable
    assert first.ownership[0].installed.value == freeze_value({"name": "new", "x": 3})
    assert remove(first).after == initial


@pytest.mark.parametrize(
    "format,raw",
    [
        (DocumentFormat.JSONC, b'{"hooks":[1, /*two*/ 2,\n 3,]}'),
        (DocumentFormat.TOML, b"hooks=[1, #two\n 2,\n 3,] #outside\n"),
    ],
)
def test_array_roundtrip_retains_comments_and_whitespace(
    format: DocumentFormat, raw: bytes
) -> None:
    first = set_content(
        raw,
        structured(
            format,
            4,
            "hooks",
            Member(Scalar(4)),
            placement=Placement(Position.BEFORE, Member(Scalar(2))),
        ),
    )
    assert first.applicable
    assert remove(first).after == raw
    adopted = set_content(
        raw, structured(format, 2, "hooks", Member(Scalar(2))), acquisition=Acquisition.ADOPT
    )
    result = remove(adopted)
    assert result.after is not None and b"two" in result.after and b"\n 3," in result.after


@pytest.mark.parametrize("format", FORMATS)
def test_array_takeover_restores_native_identity_payload_and_original_position(
    format: DocumentFormat,
) -> None:
    initial = (
        b'hooks=[{name="a"},{name="x",v=1},{name="b"}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"name":"a"},{"name":"x","v":1},{"name":"b"}]}'
    )
    member = Member(Scalar("x"), ("name",))
    first = set_content(
        initial,
        structured(
            format, {"name": "x", "v": 2}, "hooks", member, placement=Placement(Position.LAST)
        ),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable
    second = update(first, structured(format, {"name": "x", "v": 3}, "hooks", member))
    assert second.ownership[0].baseline == first.ownership[0].baseline
    assert remove(second).after == initial
    assert second.after is not None
    swapped = (
        second.after.replace(b'"a"', b'"temp"').replace(b'"b"', b'"a"').replace(b'"temp"', b'"b"')
    )
    blocked = remove(second, swapped)
    assert not blocked.applicable
    assert blocked.after == swapped and blocked.ownership == second.ownership


@pytest.mark.parametrize("format", FORMATS)
def test_array_restore_with_surviving_anchor_and_missing_position_conflict(
    format: DocumentFormat,
) -> None:
    initial = (
        b'hooks=[{name="a"},{name="x",v=1},{name="b"}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"name":"a"},{"name":"x","v":1},{"name":"b"}]}'
    )
    first = set_content(
        initial,
        structured(format, {"name": "x", "v": 2}, "hooks", Member(Scalar("x"), ("name",))),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.after is not None
    needle = b'{name="a"},' if format is DocumentFormat.TOML else b'{"name":"a"},'
    result = remove(first, first.after.replace(needle, b""))
    assert result.applicable
    assert result.after == initial.replace(needle, b"")
    data = first.after.replace(b'"a"', b'"c"').replace(b'"b"', b'"d"')
    blocked = remove(first, data)
    assert not blocked.applicable and blocked.ownership == first.ownership


@pytest.mark.parametrize("format", FORMATS)
def test_single_array_member_restoration_and_exact_identity_retarget(
    format: DocumentFormat,
) -> None:
    initial = (
        b'hooks=[{name="x",v=1}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"name":"x","v":1}]}'
    )
    first = set_content(
        initial,
        structured(format, {"name": "x", "v": 2}, "hooks", Member(Scalar("x"), ("name",))),
        acquisition=Acquisition.TAKEOVER,
    )
    assert remove(first).after == initial
    exact = set_content(None, structured(format, "old", "hooks", Member(Scalar("old"))))
    retarget = edit_document(
        exact.after,
        [
            DocumentRequest.retire(exact.ownership[0].claim),
            DocumentRequest.set(structured(format, "new", "hooks", Member(Scalar("new")))),
        ],
        ownership=exact.ownership,
        provenance=exact.provenance,
    )
    assert retarget.applicable
    assert retarget.ownership[0].disposition is Disposition.CREATED
    assert retarget.ownership[0].baseline is None
    assert remove(retarget).after is None


@pytest.mark.parametrize("format", FORMATS)
def test_nested_member_keys_and_nested_selections(format: DocumentFormat) -> None:
    initial = (
        b'hooks=[{identity={name="x"},config={value=1},foreign=2}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"identity":{"name":"x"},"config":{"value":1},"foreign":2}]}'
    )
    content = structured(
        format, 3, "hooks", Member(Scalar("x"), ("identity", "name")), "config", "value"
    )
    first = set_content(initial, content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable
    assert remove(first).after == initial


@pytest.mark.parametrize("format", FORMATS)
def test_array_rejects_ambiguous_identities_and_anchors(format: DocumentFormat) -> None:
    raw = (
        b'hooks=[{name="a"},{name="a"}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"name":"a"},{"name":"a"}]}'
    )
    with pytest.raises(ValueError, match="ambiguous"):
        set_content(
            raw, structured(format, {"name": "new"}, "hooks", Member(Scalar("new"), ("name",)))
        )
    for payload in [b"hooks=[1,1]\n" if format is DocumentFormat.TOML else b'{"hooks":[1,1]}']:
        with pytest.raises(ValueError, match="ambiguous"):
            set_content(payload, structured(format, 2, "hooks", Member(Scalar(2))))
    bad_anchor = set_content(
        None,
        structured(
            format,
            2,
            "hooks",
            Member(Scalar(2)),
            placement=Placement(Position.AFTER, Member(Scalar(1))),
        ),
    )
    assert not bad_anchor.applicable and bad_anchor.after is None
    mismatch = set_content(
        None, structured(format, {"name": "wrong"}, "hooks", Member(Scalar("right"), ("name",)))
    )
    assert not mismatch.applicable and mismatch.after is None


@pytest.mark.parametrize("format", FORMATS)
def test_typed_array_identities_distinguish_bool_integer_and_float(format: DocumentFormat) -> None:
    raw = (
        b"hooks=[true,1,1.0,-0.0,0.0]\n"
        if format is DocumentFormat.TOML
        else b'{"hooks":[true,1,1.0,-0.0,0.0]}'
    )
    first = set_content(
        raw, structured(format, 1, "hooks", Member(Scalar(1))), acquisition=Acquisition.ADOPT
    )
    result = remove(first)
    expected = (
        b"hooks=[true,1.0,-0.0,0.0]\n"
        if format is DocumentFormat.TOML
        else b'{"hooks":[true,1.0,-0.0,0.0]}'
    )
    assert result.after == expected


@pytest.mark.parametrize(
    "value", [True, False, 12, -12, 1.25, -0.0, "ü string", "line\nbreak", [], {}, [1, "two"]]
)
@pytest.mark.parametrize("format", FORMATS)
def test_scalar_and_container_types_roundtrip(format: DocumentFormat, value: object) -> None:
    first = set_content(None, structured(format, value, "x"))
    assert first.applicable
    assert first.ownership[0].installed.value == freeze_value(value)
    assert remove(first).after is None


@pytest.mark.parametrize(
    "value",
    [
        dt.date(2026, 1, 2),
        dt.time(1, 2, 3),
        dt.datetime(2026, 1, 2, 1, 2, 3),
        dt.datetime(2026, 1, 2, 1, 2, 3, tzinfo=dt.timezone(dt.timedelta(hours=1))),
    ],
)
def test_toml_temporal_types(value: object) -> None:
    first = set_content(None, structured(DocumentFormat.TOML, value, "x"))
    assert first.applicable and first.ownership[0].installed.value == freeze_value(value)
    assert remove(first).after is None
    for format in [DocumentFormat.JSON, DocumentFormat.JSONC]:
        invalid = set_content(None, structured(format, value, "x"))
        assert not invalid.applicable and invalid.after is None


def test_null_is_supported_only_in_json() -> None:
    for format in [DocumentFormat.JSON, DocumentFormat.JSONC]:
        first = set_content(None, structured(format, None, "x"))
        assert first.applicable and first.ownership[0].installed.value == Scalar(None)
        assert remove(first).after is None
    result = set_content(None, structured(DocumentFormat.TOML, None, "x"))
    assert not result.applicable and result.after is None


@pytest.mark.parametrize(
    "format,raw",
    [
        (DocumentFormat.JSON, b'{"a":1,"a":2}'),
        (DocumentFormat.JSON, b'{"a":NaN}'),
        (DocumentFormat.JSON, b'{"a":1e400}'),
        (DocumentFormat.JSON, b'{"a":9223372036854775808}'),
        (DocumentFormat.JSON, b'{"a":1,}'),
        (DocumentFormat.JSON, b"{/* c */}"),
        (DocumentFormat.JSONC, b'{"a":1,"a":1}'),
        (DocumentFormat.JSONC, b"{/*oops"),
        (DocumentFormat.JSONC, b'{"a":1} trailing'),
        (DocumentFormat.JSONC, b"{1:2}"),
        (DocumentFormat.JSONC, b'{"a" 1}'),
        (DocumentFormat.JSONC, b"["),
        (DocumentFormat.JSONC, b"[]]"),
        (DocumentFormat.JSONC, b'{"a":"\\ud800"}'),
        (DocumentFormat.TOML, b"a=1\na=2\n"),
        (DocumentFormat.TOML, b"[a]\nx=1\n[a]\ny=2\n"),
    ],
)
def test_malformed_structured_input_is_rejected(format: DocumentFormat, raw: bytes) -> None:
    with pytest.raises(ValueError):
        set_content(raw, structured(format, 1, "mcp", "x"))


@pytest.mark.parametrize(
    "parts,raw",
    [
        (["x", "y"], b'{"x":1}'),
        (["x", Member(Scalar("a"))], b'{"x":{}}'),
        ([Member(Scalar("a"))], b"{}"),
    ],
)
def test_invalid_selector_paths_are_rejected(parts: list[str | Member], raw: bytes) -> None:
    with pytest.raises(ValueError):
        set_content(raw, structured(DocumentFormat.JSON, 1, *parts))


def test_json_root_array_and_empty_comment_documents() -> None:
    first = set_content(None, structured(DocumentFormat.JSON, 1, Member(Scalar(1))))
    assert first.after == b"[1]"
    assert remove(first).after is None
    with pytest.raises(ValueError):
        set_content(b"// no root", structured(DocumentFormat.JSONC, 1, "x"))
    first = set_content(b"# preserved\n", structured(DocumentFormat.TOML, 1, "x"))
    assert remove(first).after == b"# preserved\n"


def test_toml_no_final_newline_and_foreign_table_order() -> None:
    for raw in [
        b"foreign=1",
        b"[foreign]\nx=1\n",
        b"first=1\n[foreign]\nx=1\n",
        b"a.b=1\nforeign=2\n",
    ]:
        first = set_content(raw, structured(DocumentFormat.TOML, 2, "x"))
        assert first.applicable
        assert remove(first).after == raw


def test_toml_foreign_nonfinite_numbers_remain_untouched() -> None:
    raw = b"foreign=inf\nother=nan\nx=1\n"
    first = set_content(
        raw, structured(DocumentFormat.TOML, 2, "x"), acquisition=Acquisition.TAKEOVER
    )
    assert first.applicable
    assert remove(first).after == raw


def test_toml_array_of_tables_and_out_of_order_foreign_tables() -> None:
    raw = b'[[hooks]]\nname="a"\nx=1\n[[hooks]]\nname="b"\nx=2\n'
    first = set_content(
        raw,
        structured(
            DocumentFormat.TOML, {"name": "x", "x": 3}, "hooks", Member(Scalar("x"), ("name",))
        ),
    )
    assert first.applicable
    assert remove(first).after == raw
    out_of_order = b"[a.first]\nx=1\n[foreign]\ny=2\n[a.second]\nz=3\n"
    second = set_content(
        out_of_order,
        structured(DocumentFormat.TOML, 4, "a", "second", "z"),
        acquisition=Acquisition.TAKEOVER,
    )
    assert second.applicable
    assert remove(second).after == out_of_order
    with pytest.raises(ValueError, match="noncontiguous"):
        set_content(
            out_of_order, structured(DocumentFormat.TOML, {}, "a"), acquisition=Acquisition.TAKEOVER
        )


def test_conflicting_claims_are_rejected_before_changes() -> None:
    parent = structured(DocumentFormat.JSON, {}, "mcp")
    child = structured(DocumentFormat.JSON, 1, "mcp", "x")
    for requests in [
        [DocumentRequest.set(parent), DocumentRequest.set(child)],
        [DocumentRequest.set(child), DocumentRequest.set(child)],
        [DocumentRequest.set(child), DocumentRequest.set(SectionContent("x", "body"))],
        [DocumentRequest.set(child), DocumentRequest.set(structured(DocumentFormat.JSONC, 2, "y"))],
    ]:
        with pytest.raises(ValueError):
            edit_document(None, requests)
    section_a = SectionContent("a", "A", SectionBoundaries("START", "END"))
    section_b = SectionContent("b", "B", SectionBoundaries("START", "STOP"))
    with pytest.raises(ValueError, match="overlap"):
        edit_document(None, [DocumentRequest.set(section_a), DocumentRequest.set(section_b)])


def test_inputs_outputs_and_baselines_are_immutable() -> None:
    content = structured(DocumentFormat.JSON, {"x": [1]}, "mcp")
    requests = [DocumentRequest.set(content)]
    first = edit_document(None, requests)
    requests.clear()
    owned = list(first.ownership)
    clone = invalid(replace, first, ownership=owned, observations=list(first.observations))
    owned.clear()
    assert clone == first
    with pytest.raises(FrozenInstanceError):
        first.after = b"changed"  # type: ignore[misc]
    with pytest.raises(FrozenInstanceError):
        first.ownership[0].installed.syntax = b"changed"  # type: ignore[misc]
    containers = [CreatedContainer(Selector([Key("mcp")]), b"{}")]
    provenance = DocumentProvenance(containers=containers)  # type: ignore[arg-type]
    containers.clear()
    assert len(provenance.containers) == 1


@pytest.mark.parametrize(
    "factory",
    [
        lambda: invalid(edit_document, bytearray(b"{}"), []),
        lambda: invalid(edit_document, None, [object()]),
        lambda: invalid(edit_document, None, [], ownership=[object()]),
        lambda: invalid(edit_document, None, [], provenance=object()),
        lambda: invalid(DocumentRequest.set, object()),
        lambda: invalid(DocumentRequest, Claim(Ownership.FILE), None),
        lambda: invalid(
            DocumentRequest, Claim(Ownership.SECTION, "a"), SectionContent("b", "text")
        ),
        lambda: invalid(DocumentRequest, Claim(Ownership.SECTION, "a"), None, Acquisition.ADOPT),
        lambda: invalid(DocumentRequest, Claim(Ownership.SECTION, "a"), None, "adopt"),
        lambda: invalid(ArrayPosition, previous="bad"),
        lambda: invalid(ArrayPosition, Member(Scalar(1)), Member(Scalar(1))),
        lambda: invalid(SelectionSnapshot, [], b"", b""),
        lambda: invalid(SelectionSnapshot, b"", bytearray(), b""),
        lambda: invalid(SelectionSnapshot, b"", b"", b"", position="x"),
        lambda: invalid(DocumentProvenance, empty_document=bytearray()),
        lambda: invalid(DocumentProvenance, containers=[object()]),
        lambda: invalid(DocumentProvenance, trailing_separators=["bad"]),
        lambda: invalid(CreatedContainer, "x", b"{}"),
        lambda: invalid(DocumentEdit, None, bytearray(), (), DocumentProvenance(), ()),
        lambda: invalid(DocumentEdit, None, None, (), object(), ()),
    ],
)
def test_invalid_editor_values_fail_closed(factory: Any) -> None:
    with pytest.raises((TypeError, ValueError)):
        factory()


def test_invalid_ownership_and_duplicate_provenance() -> None:
    first = set_content(None, SectionContent("x", "body"))
    owned = first.ownership[0]
    changesets: list[dict[str, Any]] = [
        {"baseline": owned.installed},
        {"disposition": Disposition.TAKEN_OVER},
        {"separator": bytearray()},
        {"format": DocumentFormat.JSON},
        {"installed": object()},
        {"claim": object()},
    ]
    for changes in changesets:
        with pytest.raises((TypeError, ValueError)):
            replace(owned, **changes)
    with pytest.raises(ValueError, match="duplicate"):
        edit_document(first.after, [], ownership=[owned, owned])
    c = CreatedContainer(Selector([Key("x")]), b"{}")
    with pytest.raises(ValueError, match="duplicate"):
        DocumentProvenance(containers=(c, c))
    with pytest.raises(ValueError):
        edit_document(
            None, [DocumentRequest.retire(Claim(Ownership.STRUCTURED, Selector([Key("x")])))]
        )
    result = edit_document(b"foreign", [DocumentRequest.retire(Claim(Ownership.SECTION, "x"))])
    assert result.observations[0].status is EditStatus.CONFLICT


def test_noop_and_invalid_encoding() -> None:
    assert edit_document(b"unchanged", []).after == b"unchanged"
    for raw in [b"\xff", b"bad\0"]:
        with pytest.raises(ValueError):
            edit_document(raw, [])
    with pytest.raises(ValueError, match="nesting"):
        set_content(
            ("[" * (sys.getrecursionlimit() + 10)).encode(),
            structured(DocumentFormat.JSON, 1, Member(Scalar(1))),
        )


def test_documents_require_complete_json_and_toml_roots() -> None:
    with pytest.raises(ValueError):
        set_content(b"1", structured(DocumentFormat.JSON, 1, "x"))
    first = set_content(None, structured(DocumentFormat.JSON, 1, "x"))
    changed = edit_document(
        first.after,
        [
            DocumentRequest.set(structured(DocumentFormat.JSON, 2, "x")),
            DocumentRequest.set(structured(DocumentFormat.JSON, 3, "bad", Member(Scalar(4)))),
        ],
        ownership=first.ownership,
        provenance=first.provenance,
    )
    assert changed.after == first.after and changed.ownership == first.ownership


def test_missing_intermediate_member_reports_conflict() -> None:
    result = set_content(
        b'{"x":[]}', structured(DocumentFormat.JSON, 1, "x", Member(Scalar("a")), "y")
    )
    assert not result.applicable
    assert result.after == b'{"x":[]}'


@pytest.mark.parametrize(
    "raw,value",
    [
        (b"x=1\nforeign=2\n", {"nested": 1}),
        (b"[x]\nnested=1\n[foreign]\ny=2\n", {"nested": 2}),
        (b"x = { nested = 1 } # foreign\nother=3\n", {"nested": 2}),
    ],
)
def test_toml_reversible_object_updates_preserve_key_layout(raw: bytes, value: object) -> None:
    first = set_content(
        raw, structured(DocumentFormat.TOML, value, "x"), acquisition=Acquisition.TAKEOVER
    )
    assert first.applicable
    newer = update(first, structured(DocumentFormat.TOML, {"newer": True}, "x"))
    assert newer.applicable
    assert newer.ownership[0].baseline == first.ownership[0].baseline
    assert remove(newer).after == raw


def test_toml_array_of_tables_takeover_updates_and_restores() -> None:
    raw = b'[[hooks]]\nname="x"\nv=1\n[[hooks]]\nname="b"\nv=2\n'
    selected = Member(Scalar("x"), ("name",))
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, {"name": "x", "v": 3}, "hooks", selected),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable
    second = update(
        first, structured(DocumentFormat.TOML, {"name": "x", "v": 4}, "hooks", selected)
    )
    assert second.applicable
    assert remove(second).after == raw


@pytest.mark.parametrize(
    "raw,value",
    [
        (b"[x]\nnested=1\n[foreign]\ny=2\n", 2),
        (b"[x]\nnested=1\n", []),
        (b'[[x]]\nname="a"\n', {"name": "b"}),
    ],
)
def test_toml_representation_changes_refuse_without_mutation(raw: bytes, value: object) -> None:
    result = set_content(
        raw, structured(DocumentFormat.TOML, value, "x"), acquisition=Acquisition.TAKEOVER
    )
    assert result.observations[0].status is EditStatus.CONFLICT
    assert result.after == raw
    assert result.ownership == ()


def test_jsonc_key_comments_and_spacing_are_foreign() -> None:
    raw = b'{"x" /* before colon */ : /* before value */ 1, "foreign":2}'
    first = set_content(
        raw, structured(DocumentFormat.JSONC, 1, "x"), acquisition=Acquisition.ADOPT
    )
    assert remove(first).after == b'{ /* before colon */  /* before value */  "foreign":2}'


def test_created_container_with_user_whitespace_is_retained() -> None:
    first = set_content(None, structured(DocumentFormat.JSONC, 1, "mcp", "x"))
    assert first.after is not None
    foreign = first.after.replace(b'{"x"', b'{  "x"')
    result = remove(first, foreign)
    assert result.after == b'{"mcp":{  }}'
    assert result.provenance == DocumentProvenance()


def test_no_requests_preserve_absence_and_missing_owned_documents() -> None:
    assert edit_document(None, []).after is None
    first = set_content(None, structured(DocumentFormat.JSON, 1, "x"))
    result = edit_document(None, [], ownership=first.ownership, provenance=first.provenance)
    assert result.after is None
    assert result.ownership == first.ownership


def test_invalid_ownership_and_observation_snapshots_are_rejected() -> None:
    from flyrail import EditObservation

    section = set_content(None, SectionContent("x", "body")).ownership[0]
    structured_owned = set_content(None, structured(DocumentFormat.JSON, 1, "x")).ownership[0]
    variants: list[Callable[[], object]] = [
        lambda: invalid(replace, section, disposition=Disposition.TAKEN_OVER, baseline=object()),
        lambda: replace(section, installed=structured_owned.installed),
        lambda: replace(structured_owned, separator=b"\n"),
        lambda: invalid(EditObservation, section.claim, "wrong", None),
        lambda: invalid(EditObservation, section.claim, EditStatus.CURRENT, []),
        lambda: invalid(EditObservation, section.claim, EditStatus.CURRENT, None, message=[]),
    ]
    for variant in variants:
        with pytest.raises((TypeError, ValueError)):
            variant()


@pytest.mark.parametrize("format", [DocumentFormat.JSONC, DocumentFormat.TOML])
@pytest.mark.parametrize("selected", ["a", "x", "b"])
@pytest.mark.parametrize("position", [Position.FIRST, Position.LAST])
def test_array_moves_restore_original_trivia(
    format: DocumentFormat, selected: str, position: Position
) -> None:
    raw = (
        b'hooks=[ {name="a"}, # x\n {name="x"}, # b\n {name="b"} ]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[ {"name":"a"}, /* x */ {"name":"x"}, /* b */ {"name":"b"} ]}'
    )
    member = Member(Scalar(selected), ("name",))
    first = set_content(
        raw,
        structured(
            format, {"name": selected, "v": 2}, "hooks", member, placement=Placement(position)
        ),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable
    result = remove(first)
    assert result.applicable
    assert result.after == raw


@pytest.mark.parametrize("format", [DocumentFormat.JSONC, DocumentFormat.TOML])
def test_changed_restoration_gap_preserves_foreign_revision(format: DocumentFormat) -> None:
    raw = (
        b'hooks=[{name="a"}, # original\n {name="x"},{name="b"}]\n'
        if format is DocumentFormat.TOML
        else b'{"hooks":[{"name":"a"}, /* original */ {"name":"x"},{"name":"b"}]}'
    )
    first = set_content(
        raw,
        structured(
            format,
            {"name": "x", "v": 2},
            "hooks",
            Member(Scalar("x"), ("name",)),
            placement=Placement(Position.LAST),
        ),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after is not None
    changed = first.after.replace(b"original", b"changed")
    result = remove(first, changed)
    assert result.observations[0].status is EditStatus.CONFLICT
    assert result.after == changed
    assert result.ownership == first.ownership


def test_batched_array_takeovers_capture_positions_before_any_edit() -> None:
    raw = b'{"hooks":[{"name":"a"},{"name":"x"},{"name":"y"},{"name":"b"}]}'
    x = structured(
        DocumentFormat.JSON,
        {"name": "x", "v": 1},
        "hooks",
        Member(Scalar("x"), ("name",)),
        placement=Placement(Position.LAST),
    )
    y = structured(
        DocumentFormat.JSON, {"name": "y", "v": 2}, "hooks", Member(Scalar("y"), ("name",))
    )
    result = edit_document(
        raw,
        [
            DocumentRequest.set(x, acquisition=Acquisition.TAKEOVER),
            DocumentRequest.set(y, acquisition=Acquisition.TAKEOVER),
        ],
    )
    assert result.applicable
    baseline = result.ownership[1].baseline
    assert baseline is not None and baseline.position is not None
    assert baseline.position.previous == Member(Scalar("x"), ("name",))
    assert baseline.position.following == Member(Scalar("b"), ("name",))
    assert remove(result).after == raw


@pytest.mark.parametrize("parts", [("mcp", "foo"), ("mcp", "foo", "env"), ("mcp", "other")])
def test_toml_dotted_objects_preserve_foreign_semantics(parts: tuple[str, ...]) -> None:
    raw = b'mcp.foo.command="old"\nforeign=1\n# end\n'
    value = {"command": "new", "env": {"X": "y"}}
    first = set_content(
        raw, structured(DocumentFormat.TOML, value, *parts), acquisition=Acquisition.TAKEOVER
    )
    assert first.applicable and first.after is not None
    expected: dict[str, Any] = {"mcp": {"foo": {"command": "old"}}, "foreign": 1}
    target = expected
    for part in parts[:-1]:
        target = target[part]
    target[parts[-1]] = value
    assert tomllib.loads(first.after.decode()) == expected
    assert first.after.endswith(b"foreign=1\n# end\n")
    second = update(first, structured(DocumentFormat.TOML, {"updated": True}, *parts))
    assert second.applicable
    assert second.ownership[0].baseline == first.ownership[0].baseline
    restored = remove(second)
    assert restored.applicable
    assert restored.after == raw


@pytest.mark.parametrize(
    "raw",
    [
        None,
        b"# foreign\n[mcp]\n",
        (
            b'[mcp . "test"] # header\ncommand="old"\n'
            b'[mcp . "test" . env] # nested\nX="before"\n# gap\n[foreign]\nv=1\n'
        ),
    ],
)
def test_toml_nested_mcp_snapshots_match_published_bytes(raw: bytes | None) -> None:
    content = structured(DocumentFormat.TOML, {"command": "cmd", "env": {"X": "y"}}, "mcp", "test")
    first = set_content(raw, content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode())["mcp"]["test"] == {
        "command": "cmd",
        "env": {"X": "y"},
    }
    unchanged = update(first, content)
    assert unchanged.applicable and unchanged.observations[0].status is EditStatus.CURRENT
    second = update(
        first, structured(DocumentFormat.TOML, {"command": "new", "env": {"X": "z"}}, "mcp", "test")
    )
    assert second.applicable
    assert second.ownership[0].baseline == first.ownership[0].baseline
    assert remove(second).after == raw


@pytest.mark.parametrize("whole", [False, True])
@pytest.mark.parametrize("create", [False, True])
def test_toml_nested_aot_members_publish_at_the_destination(whole: bool, create: bool) -> None:
    raw = (
        b"foreign=1\n"
        if create
        else (b'foreign=1\n[[hooks]] # header\nname="a"\n[hooks . env] # nested\nX="before"\n')
    )
    value = {"name": "a", "env": {"X": "y"}, "checks": [{"name": "check", "v": 1}]}
    parts: tuple[str | Member, ...] = (
        ("hooks",) if whole else ("hooks", Member(Scalar("a"), ("name",)))
    )
    content = structured(DocumentFormat.TOML, [value] if whole else value, *parts)
    first = set_content(raw, content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode()) == {"foreign": 1, "hooks": [value]}
    assert update(first, content).applicable
    newer = {"name": "a", "env": {"Y": "z"}}
    second = update(first, structured(DocumentFormat.TOML, [newer] if whole else newer, *parts))
    assert second.applicable and second.after is not None
    assert tomllib.loads(second.after.decode()) == {"foreign": 1, "hooks": [newer]}
    assert remove(second).after == raw


def test_toml_nested_aot_insertion_preserves_foreign_members() -> None:
    raw = b'[[hooks]] # foreign\nname="b"\n[hooks.env]\nB="old"\n'
    first = set_content(
        raw,
        structured(
            DocumentFormat.TOML,
            {"name": "a", "env": {"X": "y"}},
            "hooks",
            Member(Scalar("a"), ("name",)),
            placement=Placement(Position.FIRST),
        ),
    )
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode()) == {
        "hooks": [{"name": "a", "env": {"X": "y"}}, {"name": "b", "env": {"B": "old"}}]
    }
    assert first.after.endswith(raw)
    assert remove(first).after == raw


@pytest.mark.parametrize("acquisition", [Acquisition.TAKEOVER, Acquisition.ADOPT])
@pytest.mark.parametrize("where", ["only", "first", "last"])
def test_toml_aot_header_and_gap_comments_remain_foreign(
    acquisition: Acquisition, where: str
) -> None:
    member = b'[[hooks]] # header\nname="a"\nv=1\n# gap\n'
    foreign = b'[[hooks]] # other\nname="b"\n'
    raw = (foreign if where == "last" else b"") + member + (foreign if where == "first" else b"")
    selected = Member(Scalar("a"), ("name",))
    value = {"name": "a", "v": 1} if acquisition is Acquisition.ADOPT else {"name": "a", "v": 2}
    first = set_content(
        raw, structured(DocumentFormat.TOML, value, "hooks", selected), acquisition=acquisition
    )
    assert first.applicable and first.after is not None
    assert b"[[hooks]] # header\n" in first.after
    changed = first.after.replace(b"# header", b"# user header").replace(b"# gap", b"# user gap")
    second = update(
        first,
        structured(DocumentFormat.TOML, {"name": "a", "env": {"X": "y"}}, "hooks", selected),
        data=changed,
    )
    assert second.applicable and second.after is not None
    assert b"[[hooks]] # user header\n" in second.after
    assert b"# user gap\n" in second.after
    restored = remove(second)
    assert restored.applicable
    expected = raw.replace(b"# header", b"# user header").replace(b"# gap", b"# user gap")
    if acquisition is Acquisition.ADOPT:
        expected = expected.replace(
            b'[[hooks]] # user header\nname="a"\nv=1\n', b" # user header\n"
        )
    assert restored.after == expected


@pytest.mark.parametrize(
    "format,aot",
    [
        (DocumentFormat.JSON, False),
        (DocumentFormat.JSONC, False),
        (DocumentFormat.TOML, False),
        (DocumentFormat.TOML, True),
    ],
)
@pytest.mark.parametrize("reverse", [False, True])
@pytest.mark.parametrize("move_y", [False, True])
def test_batch_retirement_plans_all_original_positions(
    format: DocumentFormat, aot: bool, reverse: bool, move_y: bool
) -> None:
    raw = (
        b'[[hooks]]\nname="a"\n[[hooks]]\nname="x"\n[[hooks]]\nname="y"\n[[hooks]]\nname="b"\n'
        if aot
        else b'hooks=[ {name="a"}, # x\n {name="x"}, # y\n {name="y"}, # b\n {name="b"} ]\n'
        if format is DocumentFormat.TOML
        else (
            b'{"hooks":[ {"name":"a"}, /* x */ {"name":"x"}, '
            b'/* y */ {"name":"y"}, /* b */ {"name":"b"} ]}'
        )
        if format is DocumentFormat.JSONC
        else b'{"hooks":[{"name":"a"},{"name":"x"},{"name":"y"},{"name":"b"}]}'
    )
    contents = [
        structured(
            format,
            {"name": name, "v": 1},
            "hooks",
            Member(Scalar(name), ("name",)),
            placement=Placement(Position.LAST)
            if name == "x"
            else Placement(Position.FIRST)
            if move_y
            else None,
        )
        for name in ("x", "y")
    ]
    first = edit_document(
        raw, [DocumentRequest.set(item, acquisition=Acquisition.TAKEOVER) for item in contents]
    )
    assert first.applicable and first.after is not None
    ownership = first.ownership[::-1] if reverse else first.ownership
    requests = [DocumentRequest.retire(item.claim) for item in ownership]
    restored = edit_document(
        first.after, requests, ownership=ownership, provenance=first.provenance
    )
    assert restored.applicable
    assert restored.after == raw
    reordered = (
        first.after.replace(b'"a"', b'"temp"').replace(b'"b"', b'"a"').replace(b'"temp"', b'"b"')
    )
    blocked = edit_document(reordered, requests, ownership=ownership, provenance=first.provenance)
    assert not blocked.applicable
    assert blocked.after == reordered and blocked.ownership == ownership


def test_snapshot_restoration_layout_is_immutable() -> None:
    with pytest.raises(TypeError):
        invalid(SelectionSnapshot, Scalar(1), b"", b"", layout=bytearray())


@pytest.mark.parametrize("gap", [b"", b"# between\n\n"])
def test_contiguous_dotted_table_roundtrip(gap: bytes) -> None:
    raw = b'mcp.foo.command="old"\n' + gap + b'mcp.foo.env.X="before"\nforeign=1\n'
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, {"command": "new", "env": {"Y": "z"}}, "mcp", "foo"),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode()) == {
        "mcp": {"foo": {"command": "new", "env": {"Y": "z"}}},
        "foreign": 1,
    }
    restored = remove(first)
    assert restored.applicable and restored.after == raw


def test_foreign_key_splits_dotted_aggregate_but_leaves_descendant_editable() -> None:
    raw = b'mcp.foo.command="old"\nforeign=1\nmcp.foo.env.X="before"\n'
    with pytest.raises(ValueError, match="noncontiguous"):
        set_content(
            raw,
            structured(DocumentFormat.TOML, {"command": "new"}, "mcp", "foo"),
            acquisition=Acquisition.TAKEOVER,
        )
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, "new", "mcp", "foo", "command"),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after == raw.replace(b'"old"', b'"new"')
    assert remove(first).after == raw


@pytest.mark.parametrize("reverse", [False, True])
def test_moved_aot_batch_preserves_live_foreign_trivia(reverse: bool) -> None:
    raw = (
        b'[[hooks]] # a\nname="a"\n[[hooks]] # x\nname="x"\n# gap x\n'
        b'[[hooks]] # y\nname="y"\n# gap y\n[[hooks]] # b\nname="b"\n'
    )
    first = edit_document(
        raw,
        [
            DocumentRequest.set(
                structured(
                    DocumentFormat.TOML,
                    {"name": name, "env": {"X": "y"}},
                    "hooks",
                    Member(Scalar(name), ("name",)),
                    placement=Placement(Position.LAST if name == "x" else Position.FIRST),
                ),
                acquisition=Acquisition.TAKEOVER,
            )
            for name in ("x", "y")
        ],
    )
    assert first.applicable and first.after is not None
    observed = first.after.replace(b"# x", b"# live x").replace(b"# gap y", b"# live gap y")
    ownership = first.ownership[::-1] if reverse else first.ownership
    restored = edit_document(
        observed, [DocumentRequest.retire(item.claim) for item in ownership], ownership=ownership
    )
    assert restored.applicable
    assert restored.after == raw.replace(b"# x", b"# live x").replace(b"# gap y", b"# live gap y")


def test_toml_layout_persists_only_owned_descendant_header_spelling() -> None:
    raw = b'[mcp.test] # foreign\ncommand="old"\n[mcp . test . env] # owned\nX="before"\n# gap\n'
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, {"command": "cmd", "env": {"X": "y"}}, "mcp", "test"),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after is not None
    baseline = first.ownership[0].baseline
    assert baseline is not None
    assert json.loads(baseline.layout) == {"version": 1, "headers": [[["env"], "mcp . test . env"]]}
    restored = remove(
        first, first.after.replace(b"# foreign", b"# live").replace(b"# gap", b"# new gap")
    )
    assert restored.applicable and restored.after == raw.replace(b"# foreign", b"# live").replace(
        b"# gap", b"# new gap"
    )


def test_whole_aot_protects_nested_header_comments() -> None:
    content = structured(DocumentFormat.TOML, [{"name": "a", "env": {"X": "y"}}], "hooks")
    first = set_content(
        b'[[hooks]] # owned\nname="a"\n[hooks.env]\nX="y"\n', content, acquisition=Acquisition.ADOPT
    )
    assert first.applicable and first.after is not None
    changed = first.after.replace(b"# owned", b"# edited")
    result = remove(first, changed)
    assert result.observations[0].status is EditStatus.MODIFIED and result.after == changed


@pytest.mark.parametrize("outer", [False, True])
@pytest.mark.parametrize("newline", [b"\n", b"\r\n"])
def test_child_before_parent_takeover_updates_and_restores_exact_layout(
    outer: bool, newline: bytes
) -> None:
    raw = (
        b'# before\n[mcp . "foo" . env] # child\nX="old"\n# between\n'
        b'[mcp . "foo"] # parent\ncommand="old"\n# gap\n[foreign]\nx=1\n'
    ).replace(b"\n", newline)
    parts = ("mcp",) if outer else ("mcp", "foo")
    value: dict[str, object] = {"command": "new", "env": {"X": "new"}}
    content = structured(DocumentFormat.TOML, {"foo": value} if outer else value, *parts)
    first = set_content(raw, content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode()) == {"mcp": {"foo": value}, "foreign": {"x": 1}}
    assert update(first, content).observations[0].status is EditStatus.CURRENT
    newer: dict[str, object] = {"command": "updated", "env": {"Y": "changed"}}
    second = update(
        first, structured(DocumentFormat.TOML, {"foo": newer} if outer else newer, *parts)
    )
    assert second.applicable and second.after is not None
    assert second.ownership[0].baseline == first.ownership[0].baseline
    assert tomllib.loads(second.after.decode()) == {"mcp": {"foo": newer}, "foreign": {"x": 1}}
    restored = remove(second)
    assert restored.applicable and restored.after == raw


def test_child_before_parent_preserves_live_parent_header_and_gap() -> None:
    raw = b'[mcp.foo.env] # owned\nX="old"\n[mcp . foo] # parent\ncommand="old"\n# gap\n'
    content = structured(DocumentFormat.TOML, {"command": "new", "env": {"X": "new"}}, "mcp", "foo")
    first = set_content(raw, content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable and first.after is not None
    observed = first.after.replace(b"# parent", b"# live parent").replace(b"# gap", b"# live gap")
    restored = remove(first, observed)
    assert restored.applicable and restored.after == raw.replace(
        b"# parent", b"# live parent"
    ).replace(b"# gap", b"# live gap")


@pytest.mark.parametrize("nested_array", [False, True])
@pytest.mark.parametrize("parent_comment", [b"", b" # parent"])
@pytest.mark.parametrize("newline", [b"\n", b"\r\n"])
def test_descendant_only_takeover_preserves_live_parent_through_update_and_retirement(
    nested_array: bool, parent_comment: bytes, newline: bytes
) -> None:
    raw = (
        b'[mcp.foo.env] # owned\nX="old"\n[mcp . "foo"]'
        + parent_comment
        + b'\ncommand="old"\n# gap\n[foreign]\nx=1\n'
    ).replace(b"\n", newline)
    first_value: dict[str, object] = (
        {"checks": [{"name": "new"}]} if nested_array else {"env": {"X": "new"}}
    )
    first_content = structured(DocumentFormat.TOML, first_value, "mcp", "foo")
    first = set_content(raw, first_content, acquisition=Acquisition.TAKEOVER)
    assert first.applicable and first.after is not None
    own_header = b'[mcp . "foo"]' + parent_comment + newline
    assert own_header in first.after
    assert tomllib.loads(first.after.decode()) == {"mcp": {"foo": first_value}, "foreign": {"x": 1}}
    assert update(first, first_content).observations[0].status is EditStatus.CURRENT
    assert remove(first).after == raw

    live_header = b"[mcp . foo] # live parent" + newline
    observed = first.after.replace(own_header, live_header).replace(b"# gap", b"# live gap")
    second_value: dict[str, object] = (
        {"env": {"X": "updated"}} if nested_array else {"checks": [{"name": "updated"}]}
    )
    second = update(
        first, structured(DocumentFormat.TOML, second_value, "mcp", "foo"), data=observed
    )
    assert second.applicable and second.after is not None
    assert second.observations[0].status is EditStatus.UPDATED
    assert second.ownership[0].baseline == first.ownership[0].baseline
    assert live_header in second.after
    assert second.after.endswith((b"# live gap\n[foreign]\nx=1\n").replace(b"\n", newline))
    assert tomllib.loads(second.after.decode()) == {
        "mcp": {"foo": second_value},
        "foreign": {"x": 1},
    }
    restored = remove(second)
    assert restored.applicable and restored.after == raw.replace(own_header, live_header).replace(
        b"# gap", b"# live gap"
    )

    child_header = b"[mcp . foo.env]" if nested_array else b"[[mcp . foo.checks]]"
    edited = second.after.replace(child_header, child_header + b" # user edit")
    assert child_header + b" # user edit" in edited
    blocked = remove(second, edited)
    assert blocked.observations[0].status is EditStatus.MODIFIED
    assert blocked.after == edited and blocked.ownership == second.ownership


def test_child_before_parent_adoption_protects_child_and_detaches_foreign_trivia() -> None:
    raw = b'[mcp.foo.env] # owned\nX="old"\n[mcp.foo] # parent\ncommand="old"\n# gap\n'
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, {"command": "old", "env": {"X": "old"}}, "mcp", "foo"),
        acquisition=Acquisition.ADOPT,
    )
    assert first.applicable and first.after == raw
    edited = raw.replace(b"# owned", b"# modified")
    blocked = remove(first, edited)
    assert blocked.observations[0].status is EditStatus.MODIFIED and blocked.after == edited
    retired = remove(first)
    assert retired.applicable and retired.after == b"[mcp]\n # parent\n# gap\n"


def test_child_before_parent_with_later_enclosing_parent_remains_contiguous() -> None:
    raw = b'[mcp.foo.env]\nX="old"\n[mcp.foo]\ncommand="old"\n[mcp]\nforeign=1\n'
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, {"command": "new", "env": {"X": "new"}}, "mcp", "foo"),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode()) == {
        "mcp": {"foo": {"command": "new", "env": {"X": "new"}}, "foreign": 1}
    }
    assert remove(first).after == raw


def test_foreign_table_splits_child_before_parent_but_allows_scalar_takeover() -> None:
    raw = b'[mcp.foo.env]\nX="old"\n[foreign]\nx=1\n[mcp.foo]\ncommand="old"\n'
    with pytest.raises(ValueError, match="noncontiguous"):
        set_content(
            raw,
            structured(DocumentFormat.TOML, {"command": "new"}, "mcp", "foo"),
            acquisition=Acquisition.TAKEOVER,
        )
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, "new", "mcp", "foo", "command"),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after == raw.replace(b'command="old"', b'command="new"')
    assert remove(first).after == raw


@pytest.mark.parametrize(
    "layout",
    [
        b"not json",
        b"\xff",
        b"[]",
        b'{"version":1}',
        b'{"version":true,"headers":[]}',
        b'{"version":1.0,"headers":[]}',
        b'{"version":2,"headers":[]}',
        b'{"version":1,"headers":[],"alien":true}',
        b'{"version":1,"version":1,"headers":[]}',
        b'{"version":1,"headers":{}}',
        b'{"version":1,"headers":[{"x":1,"y":2}]}',
        b'{"version":1,"headers":[[["env"]]]}',
        b'{"version":1,"headers":[[[],"mcp.test"]]}',
        b'{"version":1,"headers":[[["env",-1],"mcp.test.env"]]}',
        b'{"version":1,"headers":[[["env",true],"mcp.test.env"]]}',
        b'{"version":1,"headers":[[["env"],"mcp.test.env"],[["env"],"mcp.test.env"]]}',
        b'{"version":1,"headers":[[["env"],null]]}',
    ],
)
def test_snapshot_rejects_malformed_layout_at_construction(layout: bytes) -> None:
    with pytest.raises(ValueError, match="TOML restoration"):
        SelectionSnapshot(freeze_value({"env": {}}), b"", b"", layout=layout)


@pytest.mark.parametrize(
    "name",
    [
        "",
        "mcp.test.env] # injected\n# another injected line\n#",
        "mcp.test.env] # injected",
        "mcp.test.env\n",
        "[mcp.test.env]",
        "mcp.test..env",
        'mcp.test."env\\q"',
        'mcp.test."\ud800"',
    ],
)
def test_snapshot_rejects_header_syntax_injection(name: str) -> None:
    layout = json.dumps({"version": 1, "headers": [[["env"], name]]}).encode()
    with pytest.raises(ValueError, match="TOML restoration header"):
        SelectionSnapshot(freeze_value({"env": {}}), b"", b"", layout=layout)


@pytest.mark.parametrize("path", [["missing"], ["env", 0], ["checks", 1], ["scalar"]])
def test_layout_paths_must_refer_to_existing_snapshot_objects(path: list[str | int]) -> None:
    layout = json.dumps({"version": 1, "headers": [[path, "mcp.test.env"]]}).encode()
    with pytest.raises(ValueError, match="TOML restoration"):
        SelectionSnapshot(
            freeze_value({"env": {}, "checks": [{}], "scalar": 1}), b"", b"", layout=layout
        )


def test_layout_header_names_are_bound_to_the_claim() -> None:
    first = set_content(
        b'[mcp.test]\ncommand="old"\n[mcp.test.env]\nX="old"\n',
        structured(DocumentFormat.TOML, {"command": "new", "env": {"X": "new"}}, "mcp", "test"),
        acquisition=Acquisition.TAKEOVER,
    )
    owned = first.ownership[0]
    assert owned.baseline is not None
    baseline = replace(
        owned.baseline,
        layout=b'{"version":1,"headers":[[["env"],"foreign.env"]]}',
    )
    with pytest.raises(ValueError, match="destination"):
        replace(owned, baseline=baseline)
    with pytest.raises(ValueError, match="requires a TOML"):
        replace(owned, format=DocumentFormat.JSONC)


@pytest.mark.parametrize("whole", [False, True])
def test_layout_accepts_quoted_header_data_and_owned_array_offsets(whole: bool) -> None:
    raw = b'[[hooks]]\nname="a"\n[hooks."env#[]"]\nX="old"\n[[hooks.checks]]\nname="check"\n'
    value = {"name": "a", "env#[]": {"X": "new"}, "checks": [{"name": "new-check"}]}
    parts: tuple[str | Member, ...] = (
        ("hooks",) if whole else ("hooks", Member(Scalar("a"), ("name",)))
    )
    first = set_content(
        raw,
        structured(DocumentFormat.TOML, [value] if whole else value, *parts),
        acquisition=Acquisition.TAKEOVER,
    )
    assert first.applicable and first.after is not None
    assert tomllib.loads(first.after.decode()) == {"hooks": [value]}
    restored = remove(first)
    assert restored.applicable and restored.after == raw
