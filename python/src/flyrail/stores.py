import errno
import math
import os
import re
import secrets
import sqlite3
import threading
import time
from abc import ABC, abstractmethod
from collections.abc import Iterator
from contextlib import AbstractContextManager, contextmanager
from pathlib import Path
from typing import NoReturn

from flyrail._security import ensure_private

_KEY = re.compile(r"[a-z0-9][a-z0-9._-]*(?:/[a-z0-9][a-z0-9._-]*)*")


class StoreBusy(Exception):
    def __init__(self, name: str) -> None:
        super().__init__(f"state store lock is busy: {name}")
        self.name = name


def validate_key(key: str, label: str = "state key") -> None:
    if not isinstance(key, str):
        raise TypeError(f"{label} must be a string")
    if len(key) > 200 or _KEY.fullmatch(key) is None:
        raise ValueError(f"{label} must be slash-separated lowercase segments")


def validate_timeout(timeout: float) -> None:
    if isinstance(timeout, bool) or not isinstance(timeout, int | float):
        raise TypeError("lock timeout must be a number")
    if timeout < 0 or not math.isfinite(timeout):
        raise ValueError("lock timeout must be finite and nonnegative")


def _value(value: object, label: str) -> bytes:
    if type(value) is not bytes:
        raise TypeError(f"{label} must be bytes")
    return value


class StateStore(ABC):
    @property
    def location(self) -> Path | None:
        return None

    @property
    def staging_directory(self) -> Path | None:
        return None

    @abstractmethod
    def read(self, key: str) -> bytes | None: ...

    @abstractmethod
    def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool: ...

    @abstractmethod
    def delete(self, key: str, *, expected: bytes) -> bool: ...

    @abstractmethod
    def keys(self, prefix: str) -> Iterator[str]: ...

    @abstractmethod
    def lock(self, name: str, timeout: float) -> AbstractContextManager[None]: ...


class MemoryStore(StateStore):
    def __init__(self) -> None:
        self._values: dict[str, bytes] = {}
        self._guard = threading.Lock()
        self._locks: dict[str, threading.Lock] = {}

    def read(self, key: str) -> bytes | None:
        validate_key(key)
        with self._guard:
            return self._values.get(key)

    def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
        validate_key(key)
        _value(value, "state value")
        if expected is not None:
            _value(expected, "expected state value")
        with self._guard:
            if self._values.get(key) != expected:
                return False
            self._values[key] = value
            return True

    def delete(self, key: str, *, expected: bytes) -> bool:
        validate_key(key)
        _value(expected, "expected state value")
        with self._guard:
            if self._values.get(key) != expected:
                return False
            del self._values[key]
            return True

    def keys(self, prefix: str) -> Iterator[str]:
        if not isinstance(prefix, str):
            raise TypeError("key prefix must be a string")
        with self._guard:
            selected = sorted(key for key in self._values if key.startswith(prefix))
        return iter(selected)

    @contextmanager
    def lock(self, name: str, timeout: float) -> Iterator[None]:
        validate_key(name, "lock name")
        validate_timeout(timeout)
        with self._guard:
            selected = self._locks.setdefault(name, threading.Lock())
        acquired = (
            selected.acquire(blocking=False) if timeout == 0 else selected.acquire(timeout=timeout)
        )
        if not acquired:
            raise StoreBusy(name)
        try:
            yield
        finally:
            selected.release()


def _private_directory(path: Path) -> None:
    missing: list[Path] = []
    probe = path
    while not probe.exists():
        missing.append(probe)
        parent = probe.parent
        if parent == probe:
            break
        probe = parent
    for directory in reversed(missing):
        try:
            directory.mkdir(mode=0o700)
        except FileExistsError:
            continue
        _sync_directory(directory.parent)


def _require_private(*paths: Path) -> None:
    for path in paths:
        try:
            ensure_private(path)
        except FileNotFoundError:
            continue
        except OSError as error:
            if error.errno != errno.ENOTSUP:
                raise
            raise PermissionError(
                errno.EACCES, "state store must be private to its owner", os.fspath(path)
            ) from error


def _sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _absolute(path: object, label: str) -> Path:
    raw = os.fspath(path)  # type: ignore[call-overload]
    if not isinstance(raw, str) or not raw or "\0" in raw:
        raise ValueError(f"{label} must be a nonempty string path without NUL")
    return Path(raw).absolute()


class _FileLocks:
    def __init__(self, directory: Path, trusted: tuple[Path, ...]) -> None:
        self._directory = directory
        self._trusted = trusted

    @contextmanager
    def hold(self, name: str, timeout: float) -> Iterator[None]:
        import fcntl

        _require_private(*self._trusted)
        _private_directory(self._directory)
        descriptor = os.open(
            self._directory / name.replace("/", "~"), os.O_RDWR | os.O_CREAT, 0o600
        )
        try:
            deadline = time.monotonic() + timeout
            while True:
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise StoreBusy(name) from None
                    time.sleep(min(0.02, remaining))
            try:
                yield
            finally:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)


_MUTATION_TIMEOUT = 10.0


class DirectoryStore(StateStore):
    def __init__(self, path: str | os.PathLike[str]) -> None:
        self._root = _absolute(path, "directory store path")
        self._records = self._root / "records"
        self._trusted = (self._root, self._records, self._root / "locks", self._root / "staging")
        self._locks = _FileLocks(self._root / "locks", self._trusted)
        self._mutation = _FileLocks(self._root, self._trusted)

    @property
    def location(self) -> Path:
        return self._root

    @property
    def staging_directory(self) -> Path:
        return self._root / "staging"

    def _path(self, key: str) -> Path:
        validate_key(key)
        return self._records / key.replace("/", "~")

    def read(self, key: str) -> bytes | None:
        path = self._path(key)
        _require_private(*self._trusted)
        try:
            return path.read_bytes()
        except FileNotFoundError:
            return None

    def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
        path = self._path(key)
        _value(value, "state value")
        if expected is not None:
            _value(expected, "expected state value")
        with self._mutation.hold("mutation", _MUTATION_TIMEOUT):
            if self.read(key) != expected:
                return False
            _private_directory(self._records)
            temporary = self._records / f".{path.name}.{secrets.token_hex(8)}.tmp"
            descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            try:
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(value)
                    stream.flush()
                    os.fsync(stream.fileno())
                os.replace(temporary, path)
            except BaseException:
                temporary.unlink(missing_ok=True)
                raise
            _sync_directory(self._records)
            return True

    def delete(self, key: str, *, expected: bytes) -> bool:
        path = self._path(key)
        _value(expected, "expected state value")
        with self._mutation.hold("mutation", _MUTATION_TIMEOUT):
            if self.read(key) != expected:
                return False
            path.unlink()
            _sync_directory(self._records)
            return True

    def keys(self, prefix: str) -> Iterator[str]:
        if not isinstance(prefix, str):
            raise TypeError("key prefix must be a string")
        _require_private(*self._trusted)
        try:
            names = [path.name for path in self._records.iterdir() if not path.name.startswith(".")]
        except FileNotFoundError:
            names = []
        keys = (name.replace("~", "/") for name in names)
        return iter(sorted(key for key in keys if key.startswith(prefix)))

    def lock(self, name: str, timeout: float) -> AbstractContextManager[None]:
        validate_key(name, "lock name")
        validate_timeout(timeout)
        return self._locks.hold(name, timeout)


def _raise_sqlite_error(error: sqlite3.Error, path: Path) -> NoReturn:
    if getattr(error, "sqlite_errorcode", 0) & 0xFF == sqlite3.SQLITE_BUSY:
        raise StoreBusy(os.fspath(path)) from error
    raise OSError(errno.EIO, str(error), os.fspath(path)) from error


class SQLiteStore(StateStore):
    def __init__(self, path: str | os.PathLike[str]) -> None:
        self._path = _absolute(path, "SQLite store path")
        locks = self._path.with_name(self._path.name + ".locks")
        self._trusted = (self._path, locks, self.staging_directory)
        self._locks = _FileLocks(locks, self._trusted)

    @property
    def location(self) -> Path:
        return self._path

    @property
    def staging_directory(self) -> Path:
        return self._path.with_name(self._path.name + ".staging")

    @contextmanager
    def _connection(self) -> Iterator[sqlite3.Connection]:
        _require_private(*self._trusted)
        try:
            _private_directory(self._path.parent)
            descriptor = os.open(self._path, os.O_CREAT | os.O_WRONLY, 0o600)
            os.close(descriptor)
        except OSError as error:
            raise OSError(
                errno.EIO, f"unable to open database file: {error}", os.fspath(self._path)
            ) from error
        try:
            connection = sqlite3.connect(
                self._path, timeout=_MUTATION_TIMEOUT, isolation_level=None
            )
        except sqlite3.Error as error:
            _raise_sqlite_error(error, self._path)
        try:
            connection.execute("PRAGMA synchronous=FULL")
            connection.execute(
                "CREATE TABLE IF NOT EXISTS flyrail_state"
                " (key TEXT PRIMARY KEY, value BLOB NOT NULL)"
            )
            yield connection
        except sqlite3.Error as error:
            _raise_sqlite_error(error, self._path)
        finally:
            connection.close()

    @staticmethod
    def _current(connection: sqlite3.Connection, key: str) -> bytes | None:
        row = connection.execute("SELECT value FROM flyrail_state WHERE key = ?", (key,)).fetchone()
        return None if row is None else bytes(row[0])

    def read(self, key: str) -> bytes | None:
        validate_key(key)
        if not self._path.exists():
            return None
        with self._connection() as connection:
            return self._current(connection, key)

    def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
        validate_key(key)
        _value(value, "state value")
        if expected is not None:
            _value(expected, "expected state value")
        with self._connection() as connection:
            connection.execute("BEGIN IMMEDIATE")
            if self._current(connection, key) != expected:
                connection.execute("ROLLBACK")
                return False
            connection.execute(
                "INSERT OR REPLACE INTO flyrail_state (key, value) VALUES (?, ?)", (key, value)
            )
            connection.execute("COMMIT")
            return True

    def delete(self, key: str, *, expected: bytes) -> bool:
        validate_key(key)
        _value(expected, "expected state value")
        if not self._path.exists():
            return False
        with self._connection() as connection:
            connection.execute("BEGIN IMMEDIATE")
            if self._current(connection, key) != expected:
                connection.execute("ROLLBACK")
                return False
            connection.execute("DELETE FROM flyrail_state WHERE key = ?", (key,))
            connection.execute("COMMIT")
            return True

    def keys(self, prefix: str) -> Iterator[str]:
        if not isinstance(prefix, str):
            raise TypeError("key prefix must be a string")
        if not self._path.exists():
            return iter(())
        with self._connection() as connection:
            rows = connection.execute(
                "SELECT key FROM flyrail_state WHERE substr(key, 1, ?) = ? ORDER BY key",
                (len(prefix), prefix),
            ).fetchall()
        return iter([row[0] for row in rows])

    def lock(self, name: str, timeout: float) -> AbstractContextManager[None]:
        validate_key(name, "lock name")
        validate_timeout(timeout)
        return self._locks.hold(name, timeout)
