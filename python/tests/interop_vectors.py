import json
import sys
import tempfile
from pathlib import Path
from typing import Any, cast

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from conformance import FIXTURES

from flyrail._codec import decode
from flyrail.authority import Claim, ResourceAuthority, state_path

TEMPORARY_ROOT = Path(__file__).resolve().parents[2] / ".cache" / "tmp"
REJECTED = b'{"fields":{},"type":"Nope"}\n'


def canonical(record: object) -> bytes:
    return (
        json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    ).encode()


def relocate(record: object, root: Path) -> object:
    if isinstance(record, list):
        return [relocate(item, root) for item in record]
    if not isinstance(record, dict):
        return record
    if set(record) == {"path"}:
        original = cast(str, record["path"])
        name = original.rsplit("/", 1)[-1]
        if name.startswith(".flyrail-"):
            target = root / ("config.json" if "config.json" in original else "AGENTS.md")
            return {"path": state_path(target).as_posix()}
        return {"path": (root / name).as_posix()}
    result = {key: relocate(value, root) for key, value in record.items()}
    if result.get("type") == "ResourceRef":
        fields = cast(dict[str, object], result["fields"])
        destination = Path(cast(dict[str, str], fields["destination"])["path"])
        fields["state_root"] = {"path": state_path(destination).as_posix()}
    if result.get("type") == "Receipt":
        fields = cast(dict[str, object], result["fields"])
        resource = cast(dict[str, object], fields["resource"])
        resource_fields = cast(dict[str, object], resource["fields"])
        destination = Path(
            cast(dict[str, str], resource_fields["destination"])["path"],
        )
        authority = ResourceAuthority(destination)
        claims = cast(list[dict[str, object]], fields["claims"])
        for claim in claims:
            claim_fields = cast(dict[str, object], claim["fields"])
            claim_record = canonical(claim_fields["claim"])
            parsed_claim = cast(Claim, decode(claim_record))
            claim_fields["claim_id"] = parsed_claim.identity(authority)
    return result


def fixture_records() -> list[tuple[str, bytes, str]]:
    data = json.loads((FIXTURES / "configurations.json").read_text(encoding="utf-8"))
    result = []
    for kind, type_name in (("receipts", "Receipt"), ("journals", "Journal")):
        for case in cast(list[dict[str, Any]], data[kind]):
            result.append((case["name"], canonical(case["record"]), type_name))
    return result


def prepared(name: str, raw: bytes, root: Path) -> bytes:
    try:
        decode(raw)
    except ValueError:
        case_root = root / name
        case_root.mkdir(parents=True, exist_ok=True)
        return canonical(relocate(json.loads(raw), case_root))
    return raw


def main() -> int:
    TEMPORARY_ROOT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=TEMPORARY_ROOT) as directory:
        root = Path(directory)
        for name, raw, type_name in fixture_records():
            sys.stdout.write(f"{name}\t{type_name}\t{prepared(name, raw, root).hex()}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
