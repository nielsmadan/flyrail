import json
from collections.abc import Callable
from dataclasses import fields, is_dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._manifest import _reject_constant, _unique_object
from flyrail.authority import Claim, Ownership
from flyrail.content import DocumentFormat, DocumentSchema, Key, Member, SectionBoundaries, Selector
from flyrail.editors import (
    ArrayPosition,
    CreatedContainer,
    Disposition,
    DocumentProvenance,
    OwnedSelection,
    SelectionSnapshot,
)
from flyrail.values import ArrayValue, ObjectValue, Scalar, semantic_record, value_from_record

_TYPES: dict[str, Callable[..., object]] = {
    cls.__name__: cls
    for cls in (
        Claim,
        Ownership,
        DocumentFormat,
        DocumentSchema,
        Key,
        Member,
        SectionBoundaries,
        Selector,
        ArrayPosition,
        CreatedContainer,
        Disposition,
        DocumentProvenance,
        OwnedSelection,
        SelectionSnapshot,
    )
}


def register(cls: type[object]) -> None:
    _TYPES[cls.__name__] = cls


def record(value: object) -> object:
    if isinstance(value, Scalar | ObjectValue | ArrayValue):
        return {"value": semantic_record(value)}
    if isinstance(value, StrEnum):
        return {"enum": type(value).__name__, "value": str(value)}
    if is_dataclass(value) and not isinstance(value, type):
        return {
            "type": type(value).__name__,
            "fields": {field.name: record(getattr(value, field.name)) for field in fields(value)},
        }
    if isinstance(value, bytes):
        return {"bytes": value.hex()}
    if isinstance(value, Path):
        return {"path": value.as_posix()}
    if isinstance(value, tuple):
        return [record(item) for item in value]
    if value is None or type(value) in {str, int, bool}:
        return value
    raise TypeError("unsupported state value")


def _decode(value: object) -> object:
    if isinstance(value, list):
        return tuple(_decode(item) for item in value)
    if not isinstance(value, dict):
        if value is None or type(value) in {str, int, bool}:
            return value
        raise ValueError("invalid state scalar")
    if value.keys() == {"value"}:
        return value_from_record(value["value"])
    if value.keys() == {"bytes"} and isinstance(value["bytes"], str):
        return bytes.fromhex(value["bytes"])
    if value.keys() == {"path"} and isinstance(value["path"], str):
        path = Path(value["path"])
        if not path.is_absolute() or ".." in path.parts:
            raise ValueError("state paths must be absolute without traversal")
        return path
    if value.keys() == {"enum", "value"} and value["enum"] in _TYPES:
        return _TYPES[value["enum"]](value["value"])
    if value.keys() == {"type", "fields"} and value["type"] in _TYPES:
        constructor = _TYPES[value["type"]]
        arguments = value["fields"]
        if not isinstance(arguments, dict) or not is_dataclass(constructor):
            raise ValueError("invalid state record")
        if arguments.keys() != {field.name for field in fields(constructor)}:
            raise ValueError("state record fields do not match schema")
        return constructor(**{key: _decode(item) for key, item in arguments.items()})
    raise ValueError("unknown state record")


def encode(value: object) -> bytes:
    return (
        json.dumps(record(value), sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    ).encode()


def decode(data: bytes) -> object:
    try:
        raw = json.loads(data, object_pairs_hook=_unique_object, parse_constant=_reject_constant)
        value = _decode(raw)
        if record(value) != raw:
            raise ValueError("state record is not canonical")
        return value
    except (RecursionError, TypeError, KeyError, AttributeError) as error:
        raise ValueError("invalid state record") from error
