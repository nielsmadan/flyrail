import re
import unicodedata

_IDENTIFIER = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
_RESERVED_NAMES = frozenset(
    {"con", "prn", "aux", "nul", "conin$", "conout$"}
    | {f"{prefix}{number}" for prefix in ("com", "lpt") for number in "123456789¹²³"}
)
_INVALID_PATH_CHARACTERS = frozenset('<>:"\\|?*')


def validate_identifier(value: str, label: str) -> None:
    if not isinstance(value, str):
        raise TypeError(f"{label} must be a string")
    if not 1 <= len(value) <= 64 or _IDENTIFIER.fullmatch(value) is None:
        raise ValueError(
            f"{label} must be 1-64 lowercase ASCII letters/digits with internal hyphens"
        )
    if value in _RESERVED_NAMES:
        raise ValueError(f"{label} is a reserved Windows name")


def validate_version(value: str) -> None:
    if not isinstance(value, str):
        raise TypeError("version must be a string")
    if not value.strip():
        raise ValueError("version must be nonblank")
    if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
        raise ValueError("version must contain only Unicode scalar values")


def validate_relative_path(value: str, label: str) -> None:
    if not isinstance(value, str):
        raise TypeError(f"{label} must be a string")
    if not value or any(part in {"", ".", ".."} for part in value.split("/")):
        raise ValueError(f"{label} must be an unambiguous relative path")
    if any(
        character in _INVALID_PATH_CHARACTERS or unicodedata.category(character) == "Cc"
        for character in value
    ):
        raise ValueError(f"{label} contains a nonportable character")
    if unicodedata.normalize("NFC", value) != value or any(
        0xD800 <= ord(character) <= 0xDFFF for character in value
    ):
        raise ValueError(f"{label} must contain NFC-normalized Unicode scalar values")
    for part in value.split("/"):
        if part.endswith((".", " ")):
            raise ValueError(f"{label} contains a component ending in a dot or space")
        if part.split(".", 1)[0].rstrip(" ").casefold() in _RESERVED_NAMES:
            raise ValueError(f"{label} contains a reserved Windows name")


def portable_path_key(value: str) -> str:
    return unicodedata.normalize("NFC", value.casefold())
