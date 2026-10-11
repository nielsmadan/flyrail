import time
from collections.abc import Callable

from flyrail.stores import StateStore, StoreBusy


def _expect(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def _excludes(store: StateStore, name: str, timeout: float) -> bool:
    try:
        with store.lock(name, timeout):
            return False
    except StoreBusy:
        return True


def check_store(open_store: Callable[[], StateStore]) -> None:
    first, second = open_store(), open_store()
    _expect(first.read("conformance/absent") is None, "absent keys must read as None")
    _expect(
        first.write("conformance/a", b"one", expected=None), "creating an absent key must succeed"
    )
    _expect(
        second.read("conformance/a") == b"one", "a write must be visible through another handle"
    )
    _expect(
        not second.write("conformance/a", b"two", expected=None),
        "creating an existing key must fail",
    )
    _expect(
        not second.write("conformance/a", b"two", expected=b"other"),
        "a stale expected value must fail",
    )
    _expect(first.read("conformance/a") == b"one", "a failed write must leave the value unchanged")
    _expect(
        second.write("conformance/a", b"two", expected=b"one"),
        "a matching expected value must succeed",
    )
    _expect(
        first.write("conformance/empty", b"", expected=None), "creating an empty value must succeed"
    )
    _expect(second.read("conformance/empty") == b"", "empty values must round-trip")
    binary = bytes(range(256))
    _expect(
        first.write("conformance/binary", binary, expected=None),
        "creating a binary value must succeed",
    )
    _expect(second.read("conformance/binary") == binary, "binary values must round-trip exactly")
    _expect(
        first.write("conformance/ab", b"x", expected=None), "creating a sibling key must succeed"
    )
    _expect(
        list(second.keys("conformance/a")) == ["conformance/a", "conformance/ab"],
        "keys must list prefix matches in sorted order",
    )
    _expect(not first.delete("conformance/a", expected=b"one"), "a stale delete must fail")
    _expect(first.delete("conformance/a", expected=b"two"), "a matching delete must succeed")
    _expect(second.read("conformance/a") is None, "a deleted key must read as None")
    _expect(not first.delete("conformance/a", expected=b"two"), "deleting an absent key must fail")
    _expect(
        list(second.keys("conformance/"))
        == ["conformance/ab", "conformance/binary", "conformance/empty"],
        "deleted keys must not be listed",
    )
    for key in ("", "../escape", "Upper/case", "trailing/", "a//b"):
        try:
            first.read(key)
        except ValueError:
            continue
        raise AssertionError(f"invalid key {key!r} must raise ValueError")
    with first.lock("conformance/lock", 0):
        _expect(_excludes(second, "conformance/lock", 0), "a held lock must exclude another handle")
        started = time.monotonic()
        _expect(
            _excludes(second, "conformance/lock", 0.2), "a held lock must exclude a bounded wait"
        )
        _expect(time.monotonic() - started >= 0.15, "a bounded lock wait must wait before failing")
        _expect(
            not _excludes(second, "conformance/other", 0), "different lock names must not exclude"
        )
    _expect(not _excludes(second, "conformance/lock", 0), "a released lock must be acquirable")
