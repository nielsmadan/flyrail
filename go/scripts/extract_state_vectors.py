import dataclasses
import hashlib
import json
import sys
from pathlib import Path
from typing import cast

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python" / "src"))
sys.path.insert(0, str(ROOT / "python" / "tests"))

from interop_vectors import fixture_records, relocate

from flyrail._codec import decode, encode
from flyrail._resource_models import (
    Ancestor,
    Generation,
    Index,
    Node,
    OwnedClaim,
    Preparation,
    Receipt,
    ResourceKind,
    ResourceRef,
    Revision,
)
from flyrail._resource_transaction import Journal
from flyrail.authority import Claim, Ownership, ResourceAuthority
from flyrail.content import DocumentFormat, DocumentSchema, Key, Member, SectionBoundaries, Selector
from flyrail.editors import (
    ArrayPosition,
    CreatedContainer,
    Disposition,
    DocumentProvenance,
    OwnedSelection,
    SelectionSnapshot,
)
from flyrail.values import ArrayValue, ObjectValue, Scalar

PIN = Path("/pin")
COMMAND = "python/.venv/bin/python -B go/scripts/extract_state_vectors.py > go/flyrail/testdata/state_vectors.tsv"


def relocated_fixtures() -> list[tuple[str, str, bytes]]:
    result = []
    for name, raw, type_name in fixture_records():
        record = relocate(json.loads(raw), PIN)
        data = (
            json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
        ).encode()
        result.append((f"fixture-{type_name.lower()}-{name}", type_name, data))
    return result


def reference(name: str, kind: ResourceKind = ResourceKind.DOCUMENT) -> ResourceRef:
    return ResourceRef(PIN / name, kind)


def owned(claim: Claim, resource: ResourceRef, **fields: object) -> OwnedClaim:
    return OwnedClaim(
        claim,
        claim.identity(ResourceAuthority(resource.destination)),
        "team",
        "guide",
        "1.0",
        "a" * 64,
        "b" * 64,
        **fields,
    )


def whole_resource_receipts() -> list[tuple[str, object]]:
    transaction = "0123456789abcdef0123456789abcdef"
    first = "c" * 64
    second = "d" * 64
    file_revision = Revision((Node("", b"body\n", 0o644, 501, 20, 1, 7),))
    old_file = Revision((Node("", b"old\n", 0o600, 501, 20, 1, 8, b"\x00acl"),))
    tree = Revision(
        (
            Node("", None, 0o755, 501, 20, 1, 2),
            Node("a", b"x", 0o644, 501, 20, 1, 3),
            Node("b", None, 0o700, 501, 20, 1, 4),
        )
    )
    old_tree = Revision((Node("", None, 0o755, 501, 20, 1, 9), Node("c", b"", 0o600, 501, 20, 1, 10)))
    requirements = ((PIN / "dep" / "one", first), (PIN / "dep" / "two", second))
    container = reference("skills", ResourceKind.SKILL_CONTAINER)
    tree_resource = reference("assets", ResourceKind.TREE)
    document = reference("tool.json")
    skills = Receipt(
        container,
        transaction,
        (
            owned(
                Claim(Ownership.SUBTREE, "alpha"),
                container,
                installed=tree,
                requirements=requirements,
                stable_requirements=(requirements[1],),
            ),
            owned(
                Claim(Ownership.SUBTREE, "beta"),
                container,
                installed=tree,
                disposition=Disposition.TAKEN_OVER,
                baseline=old_tree,
                requirements=requirements[:1],
            ),
        ),
        created_directories=(Node("alpha", None, 0o755, 501, 20, 1, 3), Node("beta", None, 0o700, 501, 20, 1, 4)),
    )
    tree_receipt = Receipt(
        tree_resource,
        transaction,
        (owned(Claim(Ownership.TREE), tree_resource, installed=tree, disposition=Disposition.ADOPTED),),
    )
    file_receipt = Receipt(
        document,
        transaction,
        (
            owned(
                Claim(Ownership.FILE),
                document,
                installed=file_revision,
                disposition=Disposition.TAKEN_OVER,
                baseline=old_file,
                requirements=requirements[:1],
                stable_requirements=requirements[:1],
            ),
        ),
    )
    return [
        ("receipt-skill-container", skills),
        ("receipt-tree", tree_receipt),
        ("receipt-file-taken-over", file_receipt),
    ]


def generated() -> list[tuple[str, object]]:
    digest = "a" * 64
    other = "b" * 64
    claims = [
        Claim(Ownership.FILE),
        Claim(Ownership.TREE),
        Claim(Ownership.SUBTREE, "review/scripts"),
        Claim(Ownership.SECTION, SectionBoundaries("<!-- café:start -->", "<!-- café:end -->")),
        Claim(
            Ownership.STRUCTURED,
            Selector(
                (
                    Key("hooks"),
                    Member(Scalar(1.5)),
                    Member(Scalar("café \x1f 日本語"), ("name",)),
                    Member(ObjectValue((("a", ArrayValue((Scalar(-3),))),))),
                )
            ),
        ),
    ]
    toml_claim = Claim(Ownership.STRUCTURED, Selector((Key("tool"), Key("servers"))))
    toml_value = ObjectValue((("env", ObjectValue((("A", Scalar("1")),))),))
    toml_layout = b'{"version":1,"headers":[[["env"],"tool.servers.env"]]}'
    toml_selection = OwnedSelection(
        toml_claim,
        DocumentFormat.TOML,
        SelectionSnapshot(toml_value, b"[tool.servers.env]\nA = \"1\"\n", b"A = \"1\"\n", None, toml_layout),
        Disposition.ADOPTED,
    )
    schema_selection = OwnedSelection(
        Claim(Ownership.STRUCTURED, Selector((Key("$schema"),))),
        DocumentFormat.JSONC,
        SelectionSnapshot(Scalar("https://example.invalid/s.json"), b'"x"', b'"x"'),
        Disposition.CREATED,
    )
    section_selection = OwnedSelection(
        Claim(Ownership.SECTION, "guide"),
        None,
        SelectionSnapshot(b"new\n", b"new\n", b"new\n"),
        Disposition.TAKEN_OVER,
        SelectionSnapshot(b"old\n", b"old\n", b"old\n"),
        b"\n",
    )
    provenance = DocumentProvenance(
        b"",
        (
            CreatedContainer(Selector((Key("mcpServers"),)), b"{}"),
            CreatedContainer(Selector((Key("hooks"), Key("events"))), b"[]"),
        ),
        (digest, other),
        (schema_selection,),
        b" \n\t",
    )
    tree = Revision(
        (
            Node("", None, 0o755, 501, 20, 1, 2, b"\x00acl"),
            Node("a", b"x", 0o644, 501, 20, 1, 3),
            Node("b", None, 0o700, 501, 20, 1, 4),
            Node("b/c", b"", 0o600, 2**40, 20, 2**63, 5 * 10**30),
        )
    )
    container = reference("skills", ResourceKind.SKILL_CONTAINER)
    generation = Generation("1.0", digest, other, ((reference("AGENTS.md"), (digest, other)), (container, ())))
    index = Index(
        "team",
        digest,
        generation,
        Generation("2", other, digest, ((reference("config.json"), (other,)),)),
        None,
        (reference("old.md"),),
    )
    journal = cast(Journal, decode(dict(relocated_records())["fixture-journal-create-section"]))
    journal = dataclasses.replace(
        journal,
        ancestors=(
            Ancestor(PIN, 1, 2, 0o1777, 0, 0, b""),
            Ancestor(PIN / "deep", 2**64, 10**30, 0o700, 501, 20, b"\x01\x02"),
        ),
    )
    preparation = Preparation(reference("AGENTS.md"), "0123456789abcdef0123456789abcdef", PIN / "x.txn")
    return [
        *((f"claim-{claim.kind}", claim) for claim in claims),
        ("selection-toml-layout", toml_selection),
        ("selection-section-takeover", section_selection),
        ("provenance-full", provenance),
        ("provenance-default", DocumentProvenance()),
        ("position-anchored", ArrayPosition(Member(Scalar("a"), ("id",)), None, b"\n  ")),
        ("schema", DocumentSchema(DocumentFormat.JSON, "$schema", Scalar(True))),
        ("revision-tree", tree),
        ("revision-empty", Revision()),
        ("generation", generation),
        ("index", index),
        ("index-empty", Index("team", other)),
        ("journal-ancestors", journal),
        ("preparation", preparation),
        ("resource-container", container),
        *whole_resource_receipts(),
    ]


def relocated_records() -> list[tuple[str, bytes]]:
    return [(name, data) for name, _, data in relocated_fixtures()]


def main() -> int:
    rows = list(relocated_fixtures())
    for name, value in generated():
        rows.append((f"generated-{name}", type(value).__name__, encode(value)))
    sys.stdout.write(f"# Generated by: {COMMAND}\n")
    sys.stdout.write("# name\ttype\tcanonical record hex\tsha256 of the record\n")
    for name, type_name, data in rows:
        value = decode(data)
        if type(value).__name__ != type_name or encode(value) != data:
            raise SystemExit(f"{name} does not round-trip in Python")
        sys.stdout.write(f"{name}\t{type_name}\t{data.hex()}\t{hashlib.sha256(data).hexdigest()}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
