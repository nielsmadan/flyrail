import json
from collections.abc import Mapping
from pathlib import Path, PurePosixPath
from typing import Any, cast

import pytest
from conformance import FIXTURES

from flyrail import (
    Claim,
    Dependency,
    DependencyMode,
    DocumentFormat,
    DocumentSchema,
    Family,
    FileContent,
    FileMode,
    Key,
    Member,
    Ownership,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    Scalar,
    SectionBoundaries,
    SectionContent,
    Selector,
    StructuredContent,
    freeze_value,
)
from flyrail.values import value_from_record

VECTORS = json.loads((FIXTURES / "configurations.json").read_text(encoding="utf-8"))
CLAIMS = {case["name"]: case for case in VECTORS["claims"]}


def claim_from_record(record: Mapping[str, Any]) -> Claim:
    kind = Ownership(record["kind"])
    selector = record["selector"]
    if kind is Ownership.SECTION:
        selector = SectionBoundaries(*selector)
    elif kind is Ownership.STRUCTURED:
        selector = Selector(
            Key(part[1]) if part[0] == "key" else Member(value_from_record(part[2]), part[1])
            for part in selector
        )
    return Claim(kind, selector)


@pytest.fixture
def portable_paths(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("flyrail.authority.canonical_destination", PurePosixPath)


@pytest.mark.usefixtures("portable_paths")
@pytest.mark.parametrize("case", VECTORS["claims"], ids=lambda case: case["name"])
def test_shared_claim_identities(case: dict[str, Any]) -> None:
    authority = ResourceAuthority(case["destination"])
    assert claim_from_record(case).identity(authority) == case["identity"]


@pytest.mark.parametrize("case", VECTORS["claim_overlaps"])
def test_shared_claim_overlap_vectors(case: dict[str, Any]) -> None:
    first = claim_from_record(CLAIMS[case["first"]])
    second = claim_from_record(CLAIMS[case["second"]])
    assert first.overlaps(second) is case["overlaps"]
    assert second.overlaps(first) is case["overlaps"]


@pytest.mark.usefixtures("portable_paths")
@pytest.mark.parametrize("case", VECTORS["authorities"], ids=lambda case: case["name"])
def test_shared_authority_paths(case: dict[str, Any]) -> None:
    authority = ResourceAuthority(case["destination"])
    assert authority.destination.as_posix() == case["destination"]
    assert authority.state_root.as_posix() == case["state_root"]
    assert authority.lock_path.as_posix() == case["lock_path"]


def rendered_artifact_from_record(raw: dict[str, Any]) -> RenderedArtifact:
    value = raw["content"]
    content: FileContent | SectionContent | StructuredContent
    if value["kind"] == "section":
        content = SectionContent(value["marker"], value["text"])
    elif value["kind"] == "file":
        content = FileContent(
            bytes.fromhex(value["data_hex"]), FileMode(value.get("mode", "private"))
        )
    else:
        assert value["kind"] == "structured"
        content = StructuredContent(
            DocumentFormat(value["format"]),
            Selector(
                Key(part)
                if isinstance(part, str)
                else Member(freeze_value(part["identity"]), part.get("key", []))
                for part in value["selector"]
            ),
            freeze_value(value["value"]),
        )
    return RenderedArtifact(
        raw["id"],
        Family(raw["family"]),
        cast(Path, PurePosixPath(raw["destination"])),
        content,
        subtree=raw.get("subtree"),
        require_existing=raw.get("require_existing", False),
        schema_requirements=tuple(
            DocumentSchema(DocumentFormat(item["format"]), item["key"], Scalar(item["value"]))
            for item in raw.get("schema_requirements", [])
        ),
    )


@pytest.mark.parametrize("case", VECTORS["renderings"], ids=lambda case: case["name"])
def test_shared_rendering_digests(monkeypatch: pytest.MonkeyPatch, case: dict[str, Any]) -> None:
    monkeypatch.setattr("flyrail.rendered.Path", PurePosixPath)
    rendered = RenderedBundle(
        (rendered_artifact_from_record(raw) for raw in case["artifacts"]),
        (
            Dependency(
                raw["dependent"], raw["required"], DependencyMode(raw.get("mode", "revision"))
            )
            for raw in case["dependencies"]
        ),
    )
    assert rendered.content_digest == case["content_digest"]
