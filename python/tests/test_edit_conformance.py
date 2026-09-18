import json
from typing import Any

import pytest
from conformance import FIXTURES

from flyrail import (
    Acquisition,
    Claim,
    Disposition,
    DocumentFormat,
    DocumentProvenance,
    DocumentRequest,
    Key,
    Member,
    OwnedSelection,
    Ownership,
    Placement,
    Position,
    SectionBoundaries,
    SectionContent,
    SelectionSnapshot,
    Selector,
    StructuredContent,
    edit_document,
    freeze_value,
    value_from_record,
)

FIXTURE = json.loads((FIXTURES / "edits.json").read_text(encoding="utf-8"))


def member(record: dict[str, Any]) -> Member:
    return Member(
        value_from_record(record["semantic_identity"])
        if "semantic_identity" in record
        else freeze_value(record["identity"]),
        record.get("key", []),
    )


def request(format: str, record: dict[str, Any]) -> DocumentRequest:
    raw = record["selector"]
    selector = (
        SectionBoundaries(raw["start"], raw["end"])
        if format == "text"
        else Selector(Key(part) if isinstance(part, str) else member(part) for part in raw)
    )
    claim = Claim(Ownership.SECTION if format == "text" else Ownership.STRUCTURED, selector)
    if record["action"] == "retire":
        return DocumentRequest.retire(claim)
    content: SectionContent | StructuredContent
    if isinstance(selector, SectionBoundaries):
        content = SectionContent(record.get("marker", "fixture"), record["text"], selector)
    else:
        placement = None
        if "placement" in record:
            position = record["placement"]
            placement = Placement(
                Position(position["position"]),
                member(position["anchor"]) if "anchor" in position else None,
            )
        content = StructuredContent(
            DocumentFormat(format),
            selector,
            value_from_record(record["semantic_value"])
            if "semantic_value" in record
            else freeze_value(record["value"]),
            placement,
        )
    return DocumentRequest.set(
        content, acquisition=Acquisition(record.get("acquisition", "conflict"))
    )


@pytest.mark.parametrize("case", FIXTURE["cases"], ids=lambda case: case["name"])
def test_portable_document_edits(case: dict[str, Any]) -> None:
    data = case["before"].encode() if case["before"] is not None else None
    ownership: tuple[OwnedSelection, ...] = ()
    provenance = DocumentProvenance()
    baselines: dict[Claim, SelectionSnapshot | None] = {}
    for step in case["steps"]:
        observed = step.get("observed", data)
        if isinstance(observed, str):
            observed = observed.encode()
        changes = [request(case["format"], record) for record in step.get("requests", [step])]
        result = edit_document(observed, changes, ownership=ownership, provenance=provenance)
        assert result.after == (step["expected"].encode() if step["expected"] is not None else None)
        assert [item.status for item in result.observations] == step.get(
            "statuses", [step.get("status")]
        )
        for owned in result.ownership:
            if owned.disposition is Disposition.TAKEN_OVER:
                baselines.setdefault(owned.claim, owned.baseline)
                assert owned.baseline == baselines[owned.claim]
            else:
                assert owned.baseline is None
        if not result.applicable:
            assert result.ownership == ownership
            assert result.provenance == provenance
        data, ownership, provenance = result.after, result.ownership, result.provenance


@pytest.mark.parametrize("case", FIXTURE["invalid"], ids=lambda case: case["name"])
def test_portable_ambiguous_documents(case: dict[str, Any]) -> None:
    requests = [
        request(case["format"], {"action": "set", "selector": selector, "value": 1, "text": "body"})
        for selector in case["selectors"]
    ]
    with pytest.raises(ValueError):
        edit_document(case["document"].encode(), requests)
