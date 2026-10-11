import os
import sqlite3
import stat
import subprocess
import sys
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

from flyrail import DirectoryStore, MemoryStore, SQLiteStore, StateStore, StoreBusy
from flyrail import stores as flyrail_stores
from flyrail.testing import check_store


def test_memory_store_meets_the_contract() -> None:
    store = MemoryStore()
    check_store(lambda: store)


class _IgnoresExpected(MemoryStore):
    def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
        current = self.read(key)
        super().write(key, value, expected=current)
        return True


def test_conformance_kit_rejects_a_store_that_ignores_expected_values() -> None:
    store = _IgnoresExpected()
    with pytest.raises(AssertionError, match="creating an existing key must fail"):
        check_store(lambda: store)


class _NeverLocks(MemoryStore):
    @contextmanager
    def lock(self, name: str, timeout: float) -> Iterator[None]:
        yield


def test_conformance_kit_rejects_a_lock_that_does_not_exclude() -> None:
    store = _NeverLocks()
    with pytest.raises(AssertionError, match="a held lock must exclude another handle"):
        check_store(lambda: store)


def test_memory_store_rejects_text_values() -> None:
    with pytest.raises(TypeError, match="bytes"):
        MemoryStore().write("index/a", "text", expected=None)  # type: ignore[arg-type]


@pytest.mark.parametrize("timeout", [-1, float("inf"), float("nan")])
def test_memory_store_rejects_invalid_lock_timeouts(timeout: float) -> None:
    with (
        pytest.raises(ValueError, match="finite and nonnegative"),
        MemoryStore().lock("a", timeout),
    ):
        pass


def test_memory_store_rejects_a_boolean_lock_timeout() -> None:
    with pytest.raises(TypeError, match="number"), MemoryStore().lock("a", True):
        pass


def test_memory_store_lock_reports_busy_by_name() -> None:
    store = MemoryStore()
    with (
        store.lock("resource/x", 0),
        pytest.raises(StoreBusy, match="resource/x"),
        store.lock("resource/x", 0),
    ):
        pass


def test_memory_store_is_a_state_store() -> None:
    assert isinstance(MemoryStore(), StateStore)


def test_directory_store_meets_the_contract(tmp_path: Path) -> None:
    check_store(lambda: DirectoryStore(tmp_path / "state"))


def test_sqlite_store_meets_the_contract(tmp_path: Path) -> None:
    check_store(lambda: SQLiteStore(tmp_path / "state.sqlite3"))


def test_directory_store_keeps_private_records(tmp_path: Path) -> None:
    store = DirectoryStore(tmp_path / "state")
    store.write("index/app/" + "0" * 64, b"record", expected=None)
    records = tmp_path / "state" / "records"
    assert sorted(path.name for path in records.iterdir()) == ["index~app~" + "0" * 64]
    assert stat.S_IMODE(records.stat().st_mode) == 0o700
    assert stat.S_IMODE((records / ("index~app~" + "0" * 64)).stat().st_mode) == 0o600


def test_reading_an_absent_sqlite_store_creates_nothing(tmp_path: Path) -> None:
    assert SQLiteStore(tmp_path / "state.sqlite3").read("index/a") is None
    assert list(SQLiteStore(tmp_path / "state.sqlite3").keys("")) == []
    assert sorted(tmp_path.iterdir()) == []


def test_sqlite_store_reports_io_failure_as_oserror(tmp_path: Path) -> None:
    (tmp_path / "state.sqlite3").mkdir(mode=0o700)
    with pytest.raises(OSError, match="unable to open"):
        SQLiteStore(tmp_path / "state.sqlite3").write("index/a", b"x", expected=None)


def test_store_locations_are_absolute(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    assert DirectoryStore("state").location == tmp_path / "state"
    assert SQLiteStore("state.sqlite3").location == tmp_path / "state.sqlite3"


def test_sqlite_store_keeps_a_private_database_file(tmp_path: Path) -> None:
    store = SQLiteStore(tmp_path / "state.sqlite3")
    store.write("index/a", b"record", expected=None)
    assert stat.S_IMODE((tmp_path / "state.sqlite3").stat().st_mode) == 0o600


def test_sqlite_store_lock_contention_raises_store_busy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "state.sqlite3"
    store = SQLiteStore(path)
    store.write("index/seed", b"seed", expected=None)
    blocker = sqlite3.connect(path, isolation_level=None)
    blocker.execute("BEGIN IMMEDIATE")
    try:
        monkeypatch.setattr(flyrail_stores, "_MUTATION_TIMEOUT", 0.1)
        with pytest.raises(StoreBusy):
            store.write("index/a", b"contended", expected=None)
    finally:
        blocker.execute("ROLLBACK")
        blocker.close()


def _operations(store: StateStore) -> dict[str, Callable[[], object]]:
    def hold() -> None:
        with store.lock("index/a", 0):
            pass

    return {
        "read": lambda: store.read("index/a"),
        "write": lambda: store.write("index/a", b"record", expected=None),
        "delete": lambda: store.delete("index/a", expected=b"record"),
        "keys": lambda: list(store.keys("")),
        "lock": hold,
    }


@pytest.mark.skipif(os.name == "nt", reason="POSIX private modes")
@pytest.mark.parametrize("operation", ["read", "write", "delete", "keys", "lock"])
@pytest.mark.parametrize("unsafe", ["", "records", "locks"])
def test_directory_store_refuses_a_location_others_can_write(
    tmp_path: Path, unsafe: str, operation: str
) -> None:
    root = tmp_path / "state"
    for name in ("records", "locks"):
        (root / name).mkdir(mode=0o700, parents=True)
    root.chmod(0o700)
    (root / unsafe).chmod(0o777)
    with pytest.raises(PermissionError, match="private to its owner") as raised:
        _operations(DirectoryStore(root))[operation]()
    assert raised.value.filename == os.fspath(root / unsafe)
    assert sorted(path.relative_to(root).as_posix() for path in root.rglob("*")) == [
        "locks",
        "records",
    ]


@pytest.mark.skipif(os.name == "nt", reason="POSIX private modes")
@pytest.mark.parametrize("operation", ["read", "write", "delete", "keys", "lock"])
@pytest.mark.parametrize("unsafe", ["state.sqlite3", "state.sqlite3.locks"])
def test_sqlite_store_refuses_a_database_others_can_write(
    tmp_path: Path, unsafe: str, operation: str
) -> None:
    path = tmp_path / "state.sqlite3"
    SQLiteStore(path).write("index/a", b"record", expected=None)
    (tmp_path / "state.sqlite3.locks").mkdir(mode=0o700)
    (tmp_path / unsafe).chmod(0o666 if unsafe == "state.sqlite3" else 0o777)
    with pytest.raises(PermissionError, match="private to its owner") as raised:
        _operations(SQLiteStore(path))[operation]()
    assert raised.value.filename == os.fspath(tmp_path / unsafe)
    (tmp_path / unsafe).chmod(0o600 if unsafe == "state.sqlite3" else 0o700)
    assert SQLiteStore(path).read("index/a") == b"record"


def test_sqlite_extended_busy_codes_raise_store_busy(tmp_path: Path) -> None:
    error = sqlite3.OperationalError("database is locked")
    error.sqlite_errorcode = sqlite3.SQLITE_BUSY | (2 << 8)
    with pytest.raises(StoreBusy):
        flyrail_stores._raise_sqlite_error(error, tmp_path / "state.sqlite3")


_HOLD = """
import sys
from flyrail import {cls}
store = {cls}(sys.argv[1])
with store.lock("resource/held", 0):
    print("held", flush=True)
    sys.stdin.readline()
"""


@pytest.mark.integration
@pytest.mark.parametrize(
    ("cls", "name"), [("DirectoryStore", "state"), ("SQLiteStore", "state.sqlite3")]
)
def test_store_locks_exclude_another_process(tmp_path: Path, cls: str, name: str) -> None:
    path = tmp_path / name
    store: StateStore = DirectoryStore(path) if cls == "DirectoryStore" else SQLiteStore(path)
    holder = subprocess.Popen(  # noqa: S603
        [sys.executable, "-c", _HOLD.format(cls=cls), os.fspath(path)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert holder.stdout is not None and holder.stdout.readline() == "held\n"
        with pytest.raises(StoreBusy), store.lock("resource/held", 0):
            pass
    finally:
        holder.communicate("\n", timeout=10)
    with store.lock("resource/held", 0):
        pass
