import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python/src"))

from flyrail.authority import Claim, Ownership
from flyrail.bundle import Bundle
from flyrail.content import (
    DocumentFormat,
    Key,
    Member,
    Placement,
    Position,
    SectionBoundaries,
    SectionContent,
    Selector,
    StructuredContent,
)
from flyrail.destinations import RenderContext, capabilities
from flyrail.editors import Acquisition, DocumentProvenance, DocumentRequest, edit_document
from flyrail.hooks import HookRuntime, HookShell
from flyrail.models import BundleEntry
from flyrail.rendered import model_value
from flyrail.targets import Agent, Platform, Surface, Target
from flyrail.translation import render, render_many
from flyrail.values import freeze_value, semantic_bytes, value_from_record


def main() -> None:
    if sys.argv[1] == "--edits":
        editing_records(Path(sys.argv[2]))
        return
    corpus = Path(sys.argv[1])
    for line in corpus.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        category, name, *columns = line.split("\t")
        if category == "value":
            if len(columns) != 1:
                raise ValueError(f"{name}: invalid differential value row")
            value = value_from_record(json.loads(bytes.fromhex(columns[0])))
            print(f"value\t{name}\tsemantic\t{semantic_bytes(value).hex()}")
        elif category == "bundle":
            manifest, count, *raw_entries = columns[1:]
            if len(raw_entries) != int(count) * 3:
                raise ValueError(f"{name}: invalid differential bundle row")
            entries = tuple(
                BundleEntry(
                    raw_entries[index],
                    None
                    if raw_entries[index + 1] == "-"
                    else bytes.fromhex(raw_entries[index + 1]),
                    raw_entries[index + 2] == "1",
                )
                for index in range(0, len(raw_entries), 3)
            )
            bundle = Bundle.from_memory(bytes.fromhex(manifest), entries)
            print(f"bundle\t{name}\tdigest\t{bundle.content_digest.encode().hex()}")
            canonical = semantic_bytes(
                model_value((bundle.artifacts, bundle.assets, bundle.dependencies))
            )
            print(f"bundle\t{name}\tcanonical\t{canonical.hex()}")
    if len(sys.argv) > 2:
        translation_records(Path(sys.argv[2]))
    if len(sys.argv) > 3:
        from lifecycle_reference import lifecycle_records
        lifecycle_records(corpus, Path(sys.argv[3]))


def editing_member(record):
    return Member(
        value_from_record(record["semantic_identity"])
        if "semantic_identity" in record
        else freeze_value(record["identity"]),
        record.get("key", []),
    )


def editing_request(format, record):
    raw = record["selector"]
    selector = (
        SectionBoundaries(raw["start"], raw["end"])
        if format == "text"
        else Selector(Key(part) if isinstance(part, str) else editing_member(part) for part in raw)
    )
    claim = Claim(Ownership.SECTION if format == "text" else Ownership.STRUCTURED, selector)
    if record["action"] == "retire":
        return DocumentRequest.retire(claim)
    if format == "text":
        content = SectionContent(record.get("marker", "fixture"), record["text"], selector)
    else:
        placement = record.get("placement")
        content = StructuredContent(
            DocumentFormat(format),
            selector,
            value_from_record(record["semantic_value"])
            if "semantic_value" in record
            else freeze_value(record["value"]),
            None
            if placement is None
            else Placement(
                Position(placement["position"]),
                editing_member(placement["anchor"]) if "anchor" in placement else None,
            ),
        )
    return DocumentRequest.set(
        content, acquisition=Acquisition(record.get("acquisition", "conflict"))
    )


def editing_selector_record(selector):
    if isinstance(selector, SectionBoundaries):
        return {"start": selector.start, "end": selector.end}
    return [
        part.name if isinstance(part, Key) else editing_member_record(part)
        for part in selector.parts
    ]


def editing_member_record(member):
    return {"identity": semantic_bytes(member.identity).hex(), "key": list(member.key)}


def editing_claim_record(claim):
    return {"kind": claim.kind.value, "selector": editing_selector_record(claim.selector)}


def editing_snapshot_record(snapshot):
    if snapshot is None:
        return None
    position = snapshot.position
    return {
        "value": (
            {"bytes": snapshot.value.hex()}
            if isinstance(snapshot.value, bytes)
            else {"semantic": semantic_bytes(snapshot.value).hex()}
        ),
        "syntax": snapshot.syntax.hex(),
        "owned_syntax": snapshot.owned_syntax.hex(),
        "layout": snapshot.layout.hex(),
        "position": None
        if position is None
        else {
            "previous": None
            if position.previous is None
            else editing_member_record(position.previous),
            "following": None
            if position.following is None
            else editing_member_record(position.following),
            "prefix": position.prefix.hex(),
        },
    }


def editing_ownership_record(owned):
    return {
        "claim": editing_claim_record(owned.claim),
        "format": owned.format.value if owned.format else None,
        "installed": editing_snapshot_record(owned.installed),
        "disposition": owned.disposition.value,
        "baseline": editing_snapshot_record(owned.baseline),
        "separator": owned.separator.hex(),
    }


def editing_result_record(edit):
    provenance = edit.provenance
    return {
        "before": None if edit.before is None else edit.before.hex(),
        "after": None if edit.after is None else edit.after.hex(),
        "applicable": edit.applicable,
        "ownership": [editing_ownership_record(owned) for owned in edit.ownership],
        "provenance": {
            "empty_document": None
            if provenance.empty_document is None
            else provenance.empty_document.hex(),
            "containers": [
                {
                    "selector": editing_selector_record(container.selector),
                    "empty_syntax": container.empty_syntax.hex(),
                }
                for container in provenance.containers
            ],
            "trailing_separators": list(provenance.trailing_separators),
            "schema_fields": [editing_ownership_record(owned) for owned in provenance.schema_fields],
            "schema_empty_document": None
            if provenance.schema_empty_document is None
            else provenance.schema_empty_document.hex(),
        },
        "observations": [
            {
                "claim": editing_claim_record(observation.claim),
                "status": observation.status.value,
                "current": editing_snapshot_record(observation.current),
                "message": observation.message,
            }
            for observation in edit.observations
        ],
    }


def editing_records(corpus):
    for case in json.loads(corpus.read_text(encoding="utf-8")):
        data = (
            bytes.fromhex(case["before_hex"])
            if "before_hex" in case
            else None
            if case["before"] is None
            else case["before"].encode()
        )
        ownership = ()
        provenance = DocumentProvenance()
        for index, step in enumerate(case["steps"]):
            observed = (
                step["observed"].encode()
                if isinstance(step.get("observed"), str)
                else step.get("observed", data)
            )
            try:
                editor = edit_document
                arguments = {}
                if case.get("schema"):
                    from flyrail._schema import edit_schema_document
                    from flyrail.content import DocumentSchema

                    editor = edit_schema_document
                    arguments = {
                        "active_schemas": tuple(
                            DocumentSchema(
                                DocumentFormat(case["format"]),
                                schema["key"],
                                freeze_value(schema["value"]),
                            )
                            for schema in step.get("schemas", [])
                        ),
                        "validate": step.get("validate", True),
                    }
                result = editor(
                    observed,
                    [editing_request(case["format"], item) for item in step.get("requests", [step])],
                    ownership=ownership,
                    provenance=provenance,
                    **arguments,
                )
                record = editing_result_record(result)
                data, ownership, provenance = result.after, result.ownership, result.provenance
            except (ValueError, TypeError) as error:
                record = {"error": str(error)}
            print(
                f"{case['name']}\t{index}\t"
                + json.dumps(record, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
            )


def translation_records(fixture: Path) -> None:
    root = Path("/fixture/project")
    document = json.loads(fixture.read_text(encoding="utf-8"))
    for item in document["cases"]:
        manifest = {
            "schema_version": 2,
            "id": "translation-demo",
            "version": "same",
            "artifacts": [item["artifact"]],
        }
        bundle = Bundle.from_memory(
            json.dumps(manifest, ensure_ascii=False, separators=(",", ":")).encode(), ()
        )
        agent = Agent(item["agent"])
        target = (
            Target.project(agent, root)
            if item["scope"] == "project"
            else Target.user(agent, home=root)
        )
        context = RenderContext(
            target,
            Platform.LINUX,
            Surface(item["surface"]),
            execution_root=root / item["execution_root"] if item.get("execution_root") else None,
            mcp_path=root / item["mcp_path"] if item.get("mcp_path") else None,
        )
        rendered = render(bundle, context)
        source_id = item["artifact"]["id"]
        artifact = next((value for value in rendered.artifacts if value.id == source_id), None)
        encoded = "-" if artifact is None else semantic_bytes(model_value(artifact)).hex()
        notices = ",".join(f"{notice.kind.value}:{notice.code}" for notice in rendered.notices)
        print(
            f"translation\t{item['name']}\t{int(rendered.supported)}\t{encoded}\t{notices}"
        )
        print(f"rendering\t{item['name']}\t{rendered.content_digest}")
        descriptor = ";".join(
            "|".join(
                (
                    capability.family.value,
                    str(int(capability.portable)),
                    str(int(capability.native)),
                    ",".join(capability.limitations),
                    ",".join(capability.prerequisites),
                )
            )
            for capability in sorted(capabilities(context), key=lambda value: value.family.value)
        )
        print(f"capabilities\t{item['name']}\t{descriptor}")
    expanded_render_records(document, root)


def emit_render_record(name: str, rendered: object) -> None:
    artifacts = ",".join(
        hashlib.sha256(semantic_bytes(model_value(value))).hexdigest()
        for value in rendered.artifacts
    )
    dependencies = ",".join(
        hashlib.sha256(semantic_bytes(model_value(value))).hexdigest()
        for value in rendered.dependencies
    )
    notices = ",".join(
        ":".join(
            (
                value.artifact_id.encode().hex(),
                value.kind.value,
                value.code.encode().hex(),
                value.message.encode().hex(),
            )
        )
        for value in rendered.notices
    )
    routes = ",".join(
        f"{key.encode().hex()}:{value.encode().hex()}"
        for key, value in rendered.routing_context
    )
    print(
        f"render-record\t{name}\t{int(rendered.supported)}\t"
        f"{rendered.content_digest}\t{artifacts}\t{dependencies}\t{notices}\t{routes}"
    )


def expanded_render_records(document: dict[str, object], root: Path) -> None:
    asset_bundle = Bundle.from_memory(
        document["asset_manifest"],
        (
            BundleEntry("runtime/server.js", b'console.log("MCP");'),
            BundleEntry("runtime/data"),
        ),
    )
    asset_context = RenderContext(
        Target.project(Agent.CODEX, root),
        Platform.LINUX,
        asset_root=root / "assets",
    )
    emit_render_record("asset-manifest", render(asset_bundle, asset_context))

    for item in document["combined_cases"]:
        bundle = Bundle.from_memory(
            {
                "schema_version": 2,
                "id": "combined",
                "version": "same",
                "artifacts": item["artifacts"],
            }
        )
        contexts = [
            RenderContext(Target.project(Agent(value["agent"]), root), Platform.LINUX)
            for value in item["contexts"]
        ]
        emit_render_record(f"combined-{item['name']}", render_many(bundle, contexts))

    hook_bundle = Bundle.from_memory(
        {
            "schema_version": 2,
            "id": "hook-differential",
            "version": "same",
            "artifacts": [
                {
                    "kind": "hook",
                    "id": "start",
                    "protocol_version": 1,
                    "event": "session-start",
                    "command": {"argv": ["/bin/echo", "space ü😀"]},
                    "timeout_ms": 1000,
                    "outcomes": ["continue"],
                    "tools": [],
                }
            ],
        }
    )
    runtime = HookRuntime(Path("/opt/node/bin/node"), HookShell.POSIX)
    for agent in (
        Agent.CLAUDE,
        Agent.CODEX,
        Agent.OPENCODE,
        Agent.PI,
        Agent.CURSOR,
        Agent.COPILOT,
    ):
        selected = RenderContext(
            Target.project(agent, root),
            Platform.LINUX,
            hook_runtime=runtime,
        )
        emit_render_record(f"hook-{agent.value}", render(hook_bundle, selected))

    relocation_bundle = Bundle.from_memory(
        {
            "schema_version": 2,
            "id": "relocation",
            "version": "same",
            "artifacts": [
                {"kind": "instruction", "id": "guide", "text": "Relocated."},
                {
                    "kind": "mcp",
                    "id": "tools",
                    "name": "tools",
                    "transport": {
                        "kind": "stdio",
                        "command": {"argv": ["server"]},
                    },
                },
            ],
        }
    )
    relocated = RenderContext(
        Target.user(
            Agent.OPENCODE,
            home=root,
            env={
                "XDG_CONFIG_HOME": "~/configuration",
                "OPENCODE_CONFIG": "~/selected/opencode.jsonc",
            },
        ),
        Platform.LINUX,
    )
    emit_render_record("relocation-opencode", render(relocation_bundle, relocated))

    dependency_bundle = Bundle.from_memory(
        {
            "schema_version": 2,
            "id": "audience-dependency",
            "version": "same",
            "artifacts": [
                {"kind": "instruction", "id": "guide", "text": "Use it."},
                {
                    "kind": "native",
                    "id": "native",
                    "family": "mcp",
                    "audience": {"agent": "claude", "scope": "project"},
                    "destination": "native.json",
                    "content": {
                        "kind": "structured",
                        "format": "json",
                        "selector": ["native"],
                        "value": {"enabled": True},
                    },
                },
            ],
            "dependencies": [{"dependent": "guide", "required": "native"}],
        }
    )
    emit_render_record(
        "dependency-audience",
        render(
            dependency_bundle,
            RenderContext(Target.project(Agent.CODEX, root), Platform.LINUX),
        ),
    )

    droid_bundle = Bundle.from_memory(
        {
            "schema_version": 2,
            "id": "mixed-droid",
            "version": "same",
            "artifacts": [
                {
                    "kind": "native",
                    "id": "claude-only",
                    "family": "instructions",
                    "audience": {"agent": "claude", "scope": "project"},
                    "destination": "CLAUDE.local.md",
                    "content": {
                        "kind": "structured",
                        "format": "json",
                        "selector": ["claude"],
                        "value": {"enabled": True},
                    },
                },
                {"kind": "instruction", "id": "guide", "text": "Portable"},
            ],
        }
    )
    emit_render_record(
        "mixed-droid",
        render_many(
            droid_bundle,
            (
                RenderContext(Target.project(Agent.CLAUDE, root), Platform.LINUX),
                RenderContext(Target.project(Agent.DROID, root), Platform.LINUX),
            ),
        ),
    )


if __name__ == "__main__":
    main()
