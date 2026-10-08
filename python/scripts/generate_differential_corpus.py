import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "spec" / "fixtures" / "differential.tsv"
SEED = 0x5EEDC0DE


class Generator:
    def __init__(self, seed: int) -> None:
        self.state = seed

    def next(self) -> int:
        self.state = (1_664_525 * self.state + 1_013_904_223) & 0xFFFFFFFF
        return self.state


def semantic(value: object) -> str:
    def record(item: object) -> object:
        if item is None:
            return ["null"]
        if type(item) is bool:
            return ["boolean", item]
        if type(item) is int:
            return ["integer", str(item)]
        if type(item) is float:
            return ["float", item.hex()]
        if isinstance(item, str):
            return ["string", item]
        if isinstance(item, list):
            return ["array", [record(value) for value in item]]
        if isinstance(item, dict):
            return [
                "object",
                [
                    [key, record(value)]
                    for key, value in sorted(item.items(), key=lambda pair: pair[0].encode())
                ],
            ]
        raise TypeError(type(item))

    return json.dumps(record(value), ensure_ascii=False, separators=(",", ":")).encode().hex()


def compact(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def require_nonempty_list(value: object, message: str) -> list[object]:
    if not isinstance(value, list) or not value:
        raise ValueError(message)
    return value


def has_structured_bundle_coverage(manifest: dict[str, object]) -> bool:
    artifacts = require_nonempty_list(manifest.get("artifacts"), "schema-2 artifacts are empty")
    assets = require_nonempty_list(manifest.get("assets"), "schema-2 assets are empty")
    dependencies = require_nonempty_list(
        manifest.get("dependencies"),
        "schema-2 dependencies are empty",
    )
    artifact_ids = {
        item["id"]
        for item in artifacts
        if isinstance(item, dict) and isinstance(item.get("id"), str)
    }
    asset_ids = {
        item["id"] for item in assets if isinstance(item, dict) and isinstance(item.get("id"), str)
    }
    require(
        len(artifact_ids) == len(artifacts) and len(asset_ids) == len(assets),
        "schema-2 generated identifiers are invalid",
    )
    require(
        all(
            isinstance(item, dict)
            and item.get("dependent") in artifact_ids
            and item.get("required") in asset_ids
            for item in dependencies
        ),
        "schema-2 dependencies do not reference generated identifiers",
    )
    return True


def generated() -> bytes:
    random = Generator(SEED)
    lines = [f"# flyrail differential corpus v1 seed={SEED:08x}"]
    values: list[object] = [
        None,
        False,
        True,
        -(2**63),
        2**63 - 1,
        0.0,
        -0.0,
        5e-324,
        "Straße",
        "Σίσυφος",
        "é",
        "e\u0301",
        "\u2028\tline\n",
        ["alias", 7, True],
        {"z": 1, "ä": "literal", "a": None},
    ]
    for _index in range(9):
        number = random.next()
        values.append(
            {
                f"k{number % 17}": [number - 2**31, f"u-{number:08x}"],
                "flag": bool(number & 1),
            }
        )
    for index, value in enumerate(values):
        lines.append(f"value\tvalue-{index:02d}\t{semantic(value)}")
    temporal = [
        '["date","2024-02-29"]',
        '["time","23:59:59.123456"]',
        '["datetime","2024-02-29T22:29:59.123456+00:00"]',
        '["datetime","2024-02-29T22:29:59.123456"]',
    ]
    for index, value in enumerate(temporal):
        lines.append(f"value\ttemporal-{index}\t{value.encode().hex()}")

    edge_paths = [
        "docs/readme.txt",
        "δelta/data.txt",
        "café/note.txt",
        "emoji-😀/value.txt",
    ]
    bundle_schemas: set[int] = set()
    bundles_with_assets = 0
    for index in range(12):
        number = random.next()
        name = f"skill-{index:02d}"
        bundle_id = f"team-{index:02d}"
        version = f"{1 + index // 4}.{index % 4}"
        schema = 2 if index < 4 or random.next() % 3 == 0 else 1
        bundle_schemas.add(schema)
        entries: list[tuple[str, bytes | None, bool]] = []
        if schema == 1:
            executable = f"bin/run-{number % 7}.sh"
            manifest = {
                "id": bundle_id,
                "schema_version": 1,
                "skills": [{"executables": [executable], "name": name, "path": name}],
                "version": version,
            }
            entries.extend(
                (
                    (f"{name}/SKILL.md", f"# {name}\nseed={number:08x}\n".encode(), False),
                    (
                        f"{name}/{executable}",
                        f"#!/bin/sh\nprintf '%s\\n' {number}\n".encode(),
                        True,
                    ),
                )
            )
            for extra_index in range(1 + random.next() % 3):
                path = edge_paths[(index + extra_index) % len(edge_paths)]
                entries.append(
                    (
                        f"{name}/{path}",
                        f"payload-{random.next():08x}-{extra_index}\r\n".encode(),
                        False,
                    )
                )
        else:
            asset_id = f"support-{index:02d}"
            artifact_id = f"guide-{index:02d}"
            source_path = f"instructions/{index:02d}.md"
            asset_path = f"assets/{index:02d}"
            manifest = {
                "artifacts": [
                    {
                        "id": artifact_id,
                        "kind": "instruction",
                        "path": source_path,
                    }
                ],
                "assets": [{"family": "instructions", "id": asset_id, "path": asset_path}],
                "dependencies": [{"dependent": artifact_id, "required": asset_id}],
                "id": bundle_id,
                "schema_version": 2,
                "version": version,
            }
            entries.extend(
                (
                    (source_path, f"# guide\nseed={number:08x}\n".encode(), False),
                    (f"{asset_path}/data.txt", f"asset-{random.next():08x}\n".encode(), False),
                    (f"{asset_path}/empty", None, False),
                )
            )
            if has_structured_bundle_coverage(manifest):
                bundles_with_assets += 1
        columns = ["bundle", f"bundle-{index:02d}", str(schema), compact(manifest).hex()]
        columns.append(str(len(entries)))
        for path, data, is_executable in entries:
            columns.extend((path, "-" if data is None else data.hex(), str(int(is_executable))))
        lines.append("\t".join(columns))

    render_payloads = [
        "plain",
        "Straße",
        "Σίσυφος",
        "line\r\nwithout-final",
        "emoji 😀",
        "e\u0301 and é",
        'quote " slash \\',
        "control\tvalue",
    ]
    render_orders: set[str] = set()
    context_orders: set[str] = set()
    for index, payload in enumerate(render_payloads):
        artifact_order = "file-first" if random.next() & 1 else "section-first"
        context_order = "alpha-first" if random.next() & 1 else "zeta-first"
        render_orders.add(artifact_order)
        context_orders.add(context_order)
        lines.append(
            "\t".join(
                (
                    "render",
                    f"render-{index:02d}",
                    f"doc-{index:02d}.txt",
                    payload.encode().hex(),
                    str(random.next()),
                    artifact_order,
                    context_order,
                )
            )
        )

    edit_rows = [
        ("section-absent", "section", "-", "set:first:conflict,set:second:conflict,retire"),
        (
            "section-crlf-no-final",
            "section",
            b"# foreign\r\nlast".hex(),
            "set:alpha:conflict,set:alpha:conflict,set:beta:conflict,retire",
        ),
        (
            "section-takeover",
            "section",
            b"foreign".hex(),
            "set:owned:takeover,retire,set:updated:takeover,retire",
        ),
        ("json-absent", "json", "-", "set:one:conflict,set:two:conflict,retire"),
        (
            "jsonc-comments",
            "jsonc",
            b'{\r\n  // keep\r\n  "foreign": 1\r\n}'.hex(),
            "set:one:conflict,set:two:conflict,set:two:conflict,retire",
        ),
        (
            "json-takeover",
            "json",
            b'{"tool":"foreign"}'.hex(),
            "set:owned:takeover,set:updated:conflict,retire",
        ),
        ("toml-absent", "toml", "-", "set:one:conflict,retire"),
        (
            "toml-crlf-no-final",
            "toml",
            b"foreign = 1\r\n# keep".hex(),
            "set:one:conflict,set:two:conflict,retire",
        ),
    ]
    edit_shapes: set[tuple[str, ...]] = set()
    edit_acquisitions: set[str] = set()
    for name, format_name, initial, encoded_operations in edit_rows:
        operations = encoded_operations.split(",")
        if random.next() & 1:
            set_indexes = [
                index for index, operation in enumerate(operations) if operation.startswith("set:")
            ]
            selected = set_indexes[random.next() % len(set_indexes)]
            operations.insert(selected + 1, operations[selected])
        edit_shapes.add(tuple(operation.split(":", 1)[0] for operation in operations))
        columns = ["edit", name, format_name, initial, str(len(operations))]
        for operation in operations:
            parts = operation.split(":")
            if parts[0] == "set":
                edit_acquisitions.add(parts[2])
                columns.extend(parts)
            else:
                columns.extend(("retire", "-", "-"))
        lines.append("\t".join(columns))

    lifecycle_rows = [
        ("absent-file", "file", "-", "one", "two", "conflict"),
        ("takeover-file", "file", b"foreign\r\n".hex(), "one", "two", "takeover"),
        ("adopt-file", "file", b"one".hex(), "one", "two", "adopt"),
        ("section-crlf", "section", b"# foreign\r\nlast".hex(), "one", "two", "conflict"),
        ("json-document", "json", b'{"foreign":1}'.hex(), "one", "two", "conflict"),
        ("toml-document", "toml", b"foreign = 1".hex(), "one", "two", "conflict"),
        ("shared-sections", "shared", b"# foreign\n".hex(), "one", "two", "conflict"),
        ("aliased-file", "alias", "-", "one", "two", "conflict"),
        ("casefold-file", "casefold", "-", "one", "two", "conflict"),
    ]
    for row in lifecycle_rows:
        lines.append("\t".join(("lifecycle", *row)))
    require(bundle_schemas == {1, 2}, "bundle schema coverage is incomplete")
    require(bundles_with_assets >= 4, "asset and dependency coverage is incomplete")
    require(
        render_orders == {"file-first", "section-first"}, "artifact order coverage is incomplete"
    )
    require(context_orders == {"alpha-first", "zeta-first"}, "context order coverage is incomplete")
    require(len(edit_shapes) >= 3, "edit operation coverage is incomplete")
    require(
        edit_acquisitions == {"conflict", "takeover"},
        "edit acquisition coverage is incomplete",
    )
    return ("\n".join(lines) + "\n").encode()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    expected = generated()
    if arguments.check:
        if not CORPUS.is_file() or CORPUS.read_bytes() != expected:
            raise SystemExit("differential corpus is stale; regenerate it")
        return 0
    CORPUS.write_bytes(expected)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
