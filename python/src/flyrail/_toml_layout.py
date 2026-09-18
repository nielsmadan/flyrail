import json
import re
import tomllib

from flyrail._json_document import PathParts

_KEY = r"""(?:[A-Za-z0-9_-]+|'[^'\x00-\x1f\x7f]*'|"(?:[^"\\\x00-\x1f\x7f]|\\[^\r\n])*")"""
_HEADER = re.compile(rf"[ \t]*{_KEY}(?:[ \t]*\.[ \t]*{_KEY})*[ \t]*")


def header_path(name: str) -> tuple[str, ...]:
    if _HEADER.fullmatch(name) is None:
        raise ValueError("invalid TOML restoration header")
    try:
        name.encode("utf-8")
        node = tomllib.loads(f"[{name}]\n")
    except (ValueError, UnicodeError) as error:
        raise ValueError("invalid TOML restoration header") from error
    parts: list[str] = []
    while node:
        key, node = next(iter(node.items()))
        parts.append(key)
    return tuple(parts)


def _unique_fields(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate TOML restoration layout field")
        result[key] = value
    return result


def layout_headers(layout: bytes) -> tuple[tuple[PathParts, str], ...]:
    if not layout:
        return ()
    try:
        record = json.loads(layout.decode("utf-8"), object_pairs_hook=_unique_fields)
    except (ValueError, UnicodeError) as error:
        raise ValueError("invalid TOML restoration layout") from error
    if (
        not isinstance(record, dict)
        or set(record) != {"version", "headers"}
        or type(record["version"]) is not int
        or record["version"] != 1
        or not isinstance(record["headers"], list)
    ):
        raise ValueError("invalid TOML restoration layout")
    headers: list[tuple[PathParts, str]] = []
    seen: set[PathParts] = set()
    for entry in record["headers"]:
        if not isinstance(entry, list) or len(entry) != 2:
            raise ValueError("invalid TOML restoration header entry")
        relative, name = entry
        if (
            not isinstance(relative, list)
            or not relative
            or any(
                type(part) not in {str, int} or (type(part) is int and part < 0)
                for part in relative
            )
        ):
            raise ValueError("invalid TOML restoration path")
        path = tuple(relative)
        if path in seen:
            raise ValueError("duplicate TOML restoration path")
        seen.add(path)
        if not isinstance(name, str):
            raise ValueError("invalid TOML restoration header")
        header_path(name)
        headers.append((path, name))
    return tuple(headers)
