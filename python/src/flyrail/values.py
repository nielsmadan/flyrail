import datetime as dt
import json
import math
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from typing import TypeAlias


def scalar_text(value: str, label: str) -> None:
    if not isinstance(value, str):
        raise TypeError(f"{label} must be a string")
    if value.isascii():
        return
    if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
        raise ValueError(f"{label} must contain Unicode scalar values")


@dataclass(frozen=True, slots=True, eq=False)
class Scalar:
    value: str | int | float | bool | dt.date | dt.time | dt.datetime | None

    def __post_init__(self) -> None:
        value = self.value
        if type(value) not in {str, int, float, bool, type(None), dt.date, dt.time, dt.datetime}:
            raise TypeError("unsupported scalar type")
        if isinstance(value, str):
            scalar_text(value, "scalar")
        if type(value) is int and not -(2**63) <= value < 2**63:
            raise ValueError("integers must fit signed 64 bits")
        if isinstance(value, float) and not math.isfinite(value):
            raise ValueError("floats must be finite")
        if isinstance(value, dt.time) and value.tzinfo is not None:
            raise ValueError("TOML local times cannot have an offset")
        if isinstance(value, dt.datetime) and value.tzinfo is not None:
            frozen = (
                value.replace(tzinfo=None)
                if value.utcoffset() is None
                else value.astimezone(dt.UTC)
            )
            object.__setattr__(self, "value", frozen)

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Scalar) and semantic_bytes(self) == semantic_bytes(other)

    def __hash__(self) -> int:
        return hash(semantic_bytes(self))


@dataclass(frozen=True, slots=True, init=False)
class ObjectValue:
    items: tuple[tuple[str, "Value"], ...]

    def __init__(self, items: Mapping[str, "Value"] | Iterable[tuple[str, "Value"]]) -> None:
        pairs = tuple(
            (key, value) for key, value in (items.items() if isinstance(items, Mapping) else items)
        )
        seen: set[str] = set()
        for key, value in pairs:
            scalar_text(key, "object key")
            validate_value(value)
            if key in seen:
                raise ValueError("duplicate object key")
            seen.add(key)
        object.__setattr__(self, "items", tuple(sorted(pairs, key=lambda pair: pair[0].encode())))


@dataclass(frozen=True, slots=True, init=False)
class ArrayValue:
    items: tuple["Value", ...]

    def __init__(self, items: Iterable["Value"]) -> None:
        values = tuple(items)
        for value in values:
            validate_value(value)
        object.__setattr__(self, "items", values)


Value: TypeAlias = Scalar | ObjectValue | ArrayValue


def validate_value(value: Value) -> None:
    if type(value) not in {Scalar, ObjectValue, ArrayValue}:
        raise TypeError("values must be Scalar, ObjectValue or ArrayValue")


def freeze_value(value: object) -> Value:
    if isinstance(value, Scalar | ObjectValue | ArrayValue):
        validate_value(value)
        return value
    if isinstance(value, dict):
        return ObjectValue((key, freeze_value(item)) for key, item in value.items())
    if isinstance(value, list | tuple):
        return ArrayValue(freeze_value(item) for item in value)
    if value is None or isinstance(value, str | int | float | bool | dt.date | dt.time):
        return Scalar(value)
    raise TypeError("unsupported configuration value")


def semantic_record(value: Value) -> object:
    validate_value(value)
    if isinstance(value, ObjectValue):
        return ["object", [[key, semantic_record(item)] for key, item in value.items]]
    if isinstance(value, ArrayValue):
        return ["array", [semantic_record(item) for item in value.items]]
    item = value.value
    if item is None:
        return ["null"]
    if isinstance(item, bool):
        return ["boolean", item]
    if isinstance(item, int):
        return ["integer", str(item)]
    if isinstance(item, float):
        return ["float", item.hex()]
    if isinstance(item, dt.datetime):
        if item.tzinfo is not None:
            item = item.astimezone(dt.UTC)
        return ["datetime", item.isoformat(timespec="microseconds")]
    if isinstance(item, dt.time):
        return ["time", item.isoformat(timespec="microseconds")]
    if isinstance(item, dt.date):
        return ["date", item.isoformat()]
    return ["string", item]


def semantic_bytes(value: Value) -> bytes:
    return json.dumps(semantic_record(value), ensure_ascii=False, separators=(",", ":")).encode()


def value_from_record(record: object) -> Value:
    if not isinstance(record, list) or not record or not isinstance(record[0], str):
        raise ValueError("semantic values must be tagged arrays")
    kind = record[0]
    if kind == "null" and len(record) == 1:
        return Scalar(None)
    if len(record) != 2:
        raise ValueError("semantic value has an invalid field count")
    raw = record[1]
    if kind == "array" and isinstance(raw, list):
        return ArrayValue(value_from_record(item) for item in raw)
    if kind == "object" and isinstance(raw, list):
        pairs: list[tuple[str, Value]] = []
        for pair in raw:
            if not isinstance(pair, list) or len(pair) != 2 or not isinstance(pair[0], str):
                raise ValueError("semantic object members must be name/value pairs")
            pairs.append((pair[0], value_from_record(pair[1])))
        return ObjectValue(pairs)
    if kind == "boolean" and type(raw) is bool:
        return Scalar(raw)
    if not isinstance(raw, str):
        raise ValueError("semantic scalar payload has the wrong type")
    value: Value
    if kind == "string":
        value = Scalar(raw)
    elif kind == "integer":
        value = Scalar(int(raw))
    elif kind == "float":
        value = Scalar(float.fromhex(raw))
    elif kind == "date":
        value = Scalar(dt.date.fromisoformat(raw))
    elif kind == "time":
        value = Scalar(dt.time.fromisoformat(raw))
    elif kind == "datetime":
        value = Scalar(dt.datetime.fromisoformat(raw))
    else:
        raise ValueError("unknown semantic scalar kind")
    if semantic_record(value) != record:
        raise ValueError("semantic scalar encoding is not canonical")
    return value
