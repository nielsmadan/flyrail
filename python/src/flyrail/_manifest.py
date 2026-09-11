import json

from flyrail._validation import portable_path_key
from flyrail.models import BundleIdentity, SkillSpec


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def _reject_constant(value: str) -> object:
    raise ValueError(f"non-JSON numeric constant: {value}")


def _object(value: object, required: set[str], optional: set[str]) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError("manifest and skill records must be JSON objects")
    if not required <= value.keys() or value.keys() - required - optional:
        raise ValueError(f"expected fields {sorted(required)}; optional fields {sorted(optional)}")
    return value


def _string(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be a string")
    return value


def read_manifest(data: bytes) -> tuple[BundleIdentity, tuple[SkillSpec, ...]]:
    raw: object = json.loads(
        data.decode("utf-8"), object_pairs_hook=_unique_object, parse_constant=_reject_constant
    )
    manifest = _object(raw, {"schema_version", "id", "version", "skills"}, set())
    if type(manifest["schema_version"]) is not int or manifest["schema_version"] != 1:
        raise ValueError("schema_version must be the integer 1")
    identity = BundleIdentity(
        _string(manifest["id"], "bundle id"), _string(manifest["version"], "version")
    )
    records = manifest["skills"]
    if not isinstance(records, list) or not records:
        raise ValueError("skills must be a nonempty JSON array")
    specs: list[SkillSpec] = []
    names: set[str] = set()
    paths: set[str] = set()
    for record in records:
        fields = _object(record, {"name", "path"}, {"executables"})
        executables = fields.get("executables", [])
        if not isinstance(executables, list):
            raise ValueError("executables must be a JSON array")
        spec = SkillSpec(
            _string(fields["name"], "skill name"),
            _string(fields["path"], "skill path"),
            [_string(value, "executable path") for value in executables],
        )
        key = portable_path_key(spec.path)
        if spec.name in names:
            raise ValueError(f"duplicate skill name: {spec.name}")
        if any(
            key == path or key.startswith(path + "/") or path.startswith(key + "/")
            for path in paths
        ):
            raise ValueError("skill source paths must not overlap")
        names.add(spec.name)
        paths.add(key)
        specs.append(spec)
    return identity, tuple(specs)
