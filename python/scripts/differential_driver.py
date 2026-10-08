import argparse
import json
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from flyrail._codec import decode, encode
from flyrail.artifacts import Family, InstructionArtifact
from flyrail.authority import Claim, Ownership, ResourceAuthority
from flyrail.bundle import Bundle
from flyrail.configuration import InstallationResult, InstallationTarget
from flyrail.content import (
    DocumentFormat,
    FileContent,
    FileMode,
    Key,
    SectionContent,
    Selector,
    StructuredContent,
)
from flyrail.editors import (
    Acquisition,
    DocumentProvenance,
    DocumentRequest,
    OwnedSelection,
    edit_document,
)
from flyrail.lifecycle import inspect_installation, recover_installation, remove, sync
from flyrail.models import BundleEntry, BundleIdentity
from flyrail.rendered import Dependency, RenderedArtifact, RenderedBundle, model_value
from flyrail.values import Scalar, Value, semantic_bytes, value_from_record


def emit(category: str, case: str, field: str, value: bytes) -> None:
    print(f"{category}\t{case}\t{field}\t{value.hex()}")


def rows(path: Path) -> list[list[str]]:
    return [
        line.split("\t")
        for line in path.read_text(encoding="utf-8").splitlines()
        if line and not line.startswith("#")
    ]


def stable_json(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def parse_value(raw: str) -> Value:
    return value_from_record(json.loads(bytes.fromhex(raw)))


def run_values(corpus: list[list[str]]) -> None:
    for _, name, raw in (row for row in corpus if row[0] == "value"):
        emit("value", name, "semantic", semantic_bytes(parse_value(raw)))


def run_bundles(corpus: list[list[str]]) -> None:
    for row in (row for row in corpus if row[0] == "bundle"):
        _, name, _schema, manifest, count, *raw_entries = row
        if len(raw_entries) != int(count) * 3:
            raise ValueError(f"{name}: invalid bundle entry count")
        entries = tuple(
            BundleEntry(
                raw_entries[index],
                None if raw_entries[index + 1] == "-" else bytes.fromhex(raw_entries[index + 1]),
                raw_entries[index + 2] == "1",
            )
            for index in range(0, len(raw_entries), 3)
        )
        bundle = Bundle.from_memory(bytes.fromhex(manifest), entries)
        emit("bundle", name, "digest", bundle.content_digest.encode())
        emit(
            "bundle",
            name,
            "canonical",
            semantic_bytes(model_value((bundle.artifacts, bundle.assets, bundle.dependencies))),
        )


def rendered_case(root: Path, row: list[str]) -> tuple[RenderedBundle, list[Claim]]:
    _, name, destination, payload, seed, artifact_order, context_order = row
    suffix = name.rsplit("-", 1)[-1]
    first = RenderedArtifact(
        f"base-{suffix}",
        Family.INSTRUCTIONS,
        root / destination,
        FileContent(bytes.fromhex(payload), FileMode.READABLE),
    )
    section = SectionContent(f"section-{suffix}", f"value-{seed}\n")
    second = RenderedArtifact(
        f"section-{suffix}",
        Family.INSTRUCTIONS,
        root / "AGENTS.md",
        section,
    )
    artifacts = (first, second) if artifact_order == "file-first" else (second, first)
    context = (
        (("alpha", "Straße"), ("zeta", seed))
        if context_order == "alpha-first"
        else (("zeta", seed), ("alpha", "Straße"))
    )
    rendered = RenderedBundle(
        artifacts,
        (Dependency(second.id, first.id),),
        routing_context=context,
    )
    claims = {
        first.id: Claim(Ownership.FILE),
        second.id: Claim(Ownership.SECTION, section.selector),
    }
    return rendered, [claims[artifact.id] for artifact in rendered.artifacts]


def run_rendered(corpus: list[list[str]], root: Path) -> None:
    for row in (row for row in corpus if row[0] == "render"):
        name = row[1]
        case_root = root / name
        case_root.mkdir()
        rendered, claims = rendered_case(case_root, row)
        emit("render", name, "digest", rendered.content_digest.encode())
        emit(
            "render",
            name,
            "canonical",
            semantic_bytes(model_value((rendered.artifacts, rendered.dependencies))),
        )
        emit("render", name, "context", semantic_bytes(model_value(rendered.routing_context)))
        for index, (artifact, claim) in enumerate(zip(rendered.artifacts, claims, strict=True)):
            authority = ResourceAuthority(artifact.destination)
            emit("render", name, f"claim-{index}", claim.identity(authority).encode())


def edit_content(format_name: str, text: str) -> SectionContent | StructuredContent:
    if format_name == "section":
        return SectionContent("managed", text)
    format_value = {
        "json": DocumentFormat.JSON,
        "jsonc": DocumentFormat.JSONC,
        "toml": DocumentFormat.TOML,
    }[format_name]
    return StructuredContent(format_value, Selector((Key("tool"),)), Scalar(text))


def snapshot_edit(
    name: str,
    step: int,
    data: bytes | None,
    ownership: tuple[OwnedSelection, ...],
    provenance: DocumentProvenance,
    statuses: list[str],
) -> None:
    emit("edit", name, f"{step}-bytes", b"-" if data is None else data)
    emit("edit", name, f"{step}-status", ",".join(statuses).encode())
    emit("edit", name, f"{step}-provenance", encode(provenance))
    emit("edit", name, f"{step}-ownership-count", str(len(ownership)).encode())
    for index, item in enumerate(ownership):
        emit("edit", name, f"{step}-ownership-{index}", encode(item))


def run_edits(corpus: list[list[str]]) -> None:
    for row in (row for row in corpus if row[0] == "edit"):
        _, name, format_name, initial, count, *raw_operations = row
        if len(raw_operations) != int(count) * 3:
            raise ValueError(f"{name}: invalid edit operation count")
        data = None if initial == "-" else bytes.fromhex(initial)
        ownership: tuple[OwnedSelection, ...] = ()
        provenance = DocumentProvenance()
        requests = []
        claim = None
        for index in range(0, len(raw_operations), 3):
            operation, value, acquisition_name = raw_operations[index : index + 3]
            if operation == "set":
                request = DocumentRequest.set(
                    edit_content(format_name, value),
                    acquisition=Acquisition(acquisition_name),
                )
                claim = request.claim
            elif operation == "retire" and claim is not None:
                request = DocumentRequest.retire(claim)
            else:
                raise ValueError(f"{name}: invalid edit operation")
            requests.append(request)
        for step, request in enumerate(requests, 1):
            edit = edit_document(data, (request,), ownership=ownership, provenance=provenance)
            data = edit.after
            ownership = edit.ownership
            provenance = edit.provenance
            snapshot_edit(
                name,
                step,
                data,
                ownership,
                provenance,
                [item.status.value for item in edit.observations],
            )


def lifecycle_bundle(version: str, ids: tuple[str, ...]) -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity("team", version),
        tuple(InstructionArtifact(identifier, identifier) for identifier in ids),
    )


def lifecycle_rendered(root: Path, kind: str, text: str) -> tuple[RenderedBundle, tuple[str, ...]]:
    destination = root / (
        "owned"
        if kind in {"file", "alias"}
        else "Straße"
        if kind == "casefold"
        else "config.toml"
        if kind == "toml"
        else "config.json"
        if kind == "json"
        else "AGENTS.md"
    )
    if kind == "shared":
        artifacts = tuple(
            RenderedArtifact(
                identifier,
                Family.INSTRUCTIONS,
                destination,
                SectionContent(identifier, f"{identifier}-{text}\n"),
            )
            for identifier in ("alpha", "zeta")
        )
        return RenderedBundle(reversed(artifacts)), ("alpha", "zeta")
    if kind == "alias":
        alias_content = FileContent(text.encode(), FileMode.READABLE)
        artifacts = tuple(
            RenderedArtifact(identifier, Family.INSTRUCTIONS, destination, alias_content)
            for identifier in ("alpha", "zeta")
        )
        return RenderedBundle(artifacts), ("alpha", "zeta")
    content: FileContent | SectionContent | StructuredContent
    if kind in {"file", "casefold"}:
        content = FileContent(text.encode(), FileMode.READABLE)
    else:
        content = edit_content(kind, text)
    return (
        RenderedBundle(
            (RenderedArtifact("managed", Family.INSTRUCTIONS, destination, content),),
            routing_context=(("seed", "1592639710"), ("surface", "differential")),
        ),
        ("managed",),
    )


def error_record(error: object | None, root: Path) -> object:
    if error is None:
        return None
    path = getattr(error, "path", None)
    return {
        "code": getattr(getattr(error, "code", None), "value", None),
        "message": getattr(error, "message", str(error)),
        "path": None if path is None else relative_path(Path(path), root),
    }


def relative_path(path: Path, root: Path) -> str:
    try:
        return "$ROOT/" + path.relative_to(root).as_posix()
    except ValueError:
        return path.as_posix()


def result_record(result: InstallationResult, root: Path) -> bytes:
    observation = result.observation
    return stable_json(
        {
            "error": error_record(result.error, root),
            "observation": {
                "bundle_digest": observation.bundle_digest,
                "current": observation.is_current,
                "error": error_record(observation.error, root),
                "pending": observation.pending,
                "render_digest": observation.render_digest,
                "resources": [
                    {
                        "claims": [
                            {
                                "artifact": claim.artifact_id,
                                "bundle": claim.bundle_id,
                                "bundle_digest": claim.bundle_digest,
                                "claim": claim.claim_id,
                                "render_digest": claim.render_digest,
                                "status": claim.status.value,
                                "version": claim.version,
                            }
                            for claim in resource.claims
                        ],
                        "destination": relative_path(resource.destination, root),
                        "error": error_record(resource.error, root),
                        "revision": resource.revision_digest,
                        "state_root": relative_path(resource.state_root, root),
                    }
                    for resource in observation.resources
                ],
                "version": observation.version,
            },
            "resources": [
                {
                    "destination": relative_path(item.resource.destination, root),
                    "error": error_record(item.error, root),
                    "status": item.status.value,
                }
                for item in result.resources
            ],
            "status": result.status.value,
        }
    )


def normalize_state(value: object, root: Path) -> object:
    if isinstance(value, list):
        return [normalize_state(item, root) for item in value]
    if not isinstance(value, dict):
        return value
    result = {key: normalize_state(item, root) for key, item in value.items()}
    if set(result) == {"path"} and isinstance(result["path"], str):
        result["path"] = relative_path(Path(result["path"]), root)
    fields = result.get("fields")
    if isinstance(fields, dict):
        if result.get("type") in {"Node", "Ancestor"}:
            for key in ("device", "inode", "uid", "gid"):
                fields[key] = 0
            fields["security"] = {"bytes": ""}
        if result.get("type") == "Receipt":
            fields["transaction_id"] = "0" * 32
    return result


def state_snapshot(name: str, step: str, root: Path, target: InstallationTarget) -> None:
    candidates = [target.index_root / "index.json", target.index_root / "index.json.next"]
    candidates.extend(root.rglob("receipt.json"))
    for path in sorted(set(candidates)):
        if not path.is_file():
            continue
        raw = path.read_bytes()
        value = decode(raw)
        if encode(value) != raw:
            raise ValueError(f"state record is not canonical: {path}")
        normalized = stable_json(normalize_state(json.loads(raw), root)) + b"\n"
        emit("lifecycle", name, f"{step}-state-{relative_path(path, root)}", normalized)


def lifecycle_result(
    name: str, step: str, result: InstallationResult, root: Path, target: InstallationTarget
) -> None:
    emit("lifecycle", name, f"{step}-result", result_record(result, root))
    state_snapshot(name, step, root, target)
    payloads = []
    for path in sorted(root.rglob("*")):
        if path.is_file() and ".flyrail" not in path.as_posix() and path.name != "lock":
            payloads.append((relative_path(path, root), path.read_bytes().hex()))
    emit("lifecycle", name, f"{step}-payloads", stable_json(payloads))


def run_lifecycle(corpus: list[list[str]], root: Path) -> None:
    for row in (row for row in corpus if row[0] == "lifecycle"):
        _, name, kind, initial, first, second, acquisition_name = row
        case_root = root / name
        case_root.mkdir()
        target = InstallationTarget(case_root / "index")
        first_rendered, ids = lifecycle_rendered(case_root, kind, first)
        destination = first_rendered.artifacts[0].destination
        if initial != "-":
            destination.write_bytes(
                first.encode() if acquisition_name == "adopt" else bytes.fromhex(initial)
            )
        acquisition = Acquisition(acquisition_name)
        created = sync(
            lifecycle_bundle("1", ids),
            first_rendered,
            target,
            acquisition=acquisition,
        )
        lifecycle_result(name, "1-create", created, case_root, target)
        unchanged = sync(lifecycle_bundle("1", ids), first_rendered, target)
        lifecycle_result(name, "2-unchanged", unchanged, case_root, target)
        second_rendered, _ = lifecycle_rendered(case_root, kind, second)
        updated = sync(lifecycle_bundle("2", ids), second_rendered, target)
        lifecycle_result(name, "3-update", updated, case_root, target)
        shutil.copyfile(target.index_root / "index.json", target.index_root / "index.json.next")
        recovered = recover_installation("team", target)
        lifecycle_result(name, "4-recover", recovered, case_root, target)
        observed = inspect_installation("team", target)
        emit(
            "lifecycle",
            name,
            "5-inspect",
            stable_json(
                {
                    "current": observed.is_current,
                    "pending": observed.pending,
                    "revision": [item.revision_digest for item in observed.resources],
                }
            ),
        )
        removed = remove("team", target)
        lifecycle_result(name, "6-remove", removed, case_root, target)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        arguments.root = arguments.root.resolve()
        arguments.root.mkdir(parents=True)
        corpus = rows(arguments.corpus)
        run_values(corpus)
        run_bundles(corpus)
        run_rendered(corpus, arguments.root)
        run_edits(corpus)
        run_lifecycle(corpus, arguments.root)
    except (OSError, TypeError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
