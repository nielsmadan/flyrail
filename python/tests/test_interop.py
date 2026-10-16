import base64
import json
import os
import re
import select
import shutil
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from functools import cache
from pathlib import Path
from typing import Any, cast

import pytest
from interop_vectors import REJECTED, fixture_records, prepared

from flyrail._codec import decode
from flyrail._resource_models import Receipt

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / "python" / "scripts" / "interop_driver.py"
RUST = ROOT / "rust"
RUST_BINARY = RUST / "target" / "debug" / "examples" / "interop"
CARGO_HOME = ROOT / ".cache" / "cargo-home"
FIXTURE_RECORDS = fixture_records()
MAX_PAYLOAD_SIZE = 1024 * 1024
LANGUAGES = ("python", "rust")
ZERO_ID = "0" * 32
TRANSACTION_DIRECTORY = re.compile(
    r"((?:^|/)(?:\.flyrail-)?[0-9a-f]{64}-)[0-9a-f]{32}(?=\.txn|/|$)"
)


@cache
def rust_example() -> Path:
    cargo = shutil.which("cargo")
    if cargo is None:
        if os.environ.get("CI"):
            pytest.fail("cargo is required for interop in CI")
        pytest.skip("interop skipped: cargo is unavailable")
    environment = {**os.environ, "CARGO_HOME": str(CARGO_HOME)}
    result = subprocess.run(  # noqa: S603
        [
            cargo,
            "build",
            "--example",
            "interop",
            "--features",
            "interop-test-seams",
        ],
        cwd=RUST,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return RUST_BINARY


def run_driver(payload: bytes) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(  # noqa: S603
        [sys.executable, str(DRIVER), "encode"],
        input=payload,
        capture_output=True,
        check=False,
    )


def run_rust(binary: Path, type_name: str, payload: bytes) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(  # noqa: S603
        [str(binary), "encode", "--type", type_name],
        input=payload,
        capture_output=True,
        check=False,
    )


def executable(implementation: str, binary: Path) -> list[str]:
    return [sys.executable, str(DRIVER)] if implementation == "python" else [str(binary)]


def invoke(
    implementation: str, binary: Path, command: str, *arguments: str
) -> tuple[subprocess.CompletedProcess[bytes], dict[str, Any]]:
    result = subprocess.run(  # noqa: S603
        [*executable(implementation, binary), command, *arguments],
        capture_output=True,
        check=False,
    )
    value = cast(dict[str, Any], json.loads(result.stdout))
    return result, value


def assert_success(result: subprocess.CompletedProcess[bytes], value: dict[str, Any]) -> None:
    assert result.returncode == 0, (result.stderr, value)
    assert value["status"] in {"applied", "unchanged"}, value
    assert value["error"] is None


def read_child_value(child: subprocess.Popen[bytes]) -> dict[str, Any]:
    assert child.stdout is not None
    readable, _, _ = select.select([child.stdout], [], [], 10)
    assert readable, "child did not acknowledge before timeout"
    return cast(dict[str, Any], json.loads(child.stdout.readline()))


def wait_for_path(path: Path, child: subprocess.Popen[bytes]) -> None:
    deadline = time.monotonic() + 10
    while not path.exists() and child.poll() is None and time.monotonic() < deadline:
        time.sleep(0.005)
    assert path.exists() and child.poll() is None


def normalized_path(text: str, root: Path) -> str:
    prefix = root.as_posix()
    if text == prefix or text.startswith(prefix + "/"):
        text = "$ROOT" + text[len(prefix) :]
    return TRANSACTION_DIRECTORY.sub(rf"\g<1>{ZERO_ID}", text)


def normalized_record(value: object, root: Path) -> object:
    if isinstance(value, list):
        return [normalized_record(item, root) for item in value]
    if not isinstance(value, dict):
        return value
    result = {key: normalized_record(item, root) for key, item in value.items()}
    if set(result) == {"path"} and isinstance(result["path"], str):
        result["path"] = normalized_path(result["path"], root)
    fields = result.get("fields")
    if isinstance(fields, dict):
        if result.get("type") in {"Node", "Ancestor"}:
            for key in ("device", "inode", "uid", "gid"):
                fields[key] = 0
            fields["security"] = {"bytes": ""}
        if result.get("type") in {"Receipt", "Preparation"}:
            fields["transaction_id"] = ZERO_ID
    return result


def comparable(value: object, root: Path) -> object:
    if isinstance(value, list):
        return [comparable(item, root) for item in value]
    if isinstance(value, dict):
        return {key: comparable(item, root) for key, item in value.items() if key != "message"}
    if isinstance(value, str):
        return normalized_path(value, root)
    return value


@dataclass(frozen=True)
class Application:
    implementation: str
    binary: Path
    root: Path
    state: Path

    @property
    def target(self) -> tuple[str, str]:
        return ("--state", str(self.state))

    def invoke(
        self, command: str, *arguments: str
    ) -> tuple[subprocess.CompletedProcess[bytes], dict[str, Any]]:
        return invoke(self.implementation, self.binary, command, *arguments)

    def command(self, command: str, *arguments: str) -> list[str]:
        return [*executable(self.implementation, self.binary), command, *self.target, *arguments]

    def write(
        self, command: str, destination: Path, *arguments: str
    ) -> tuple[subprocess.CompletedProcess[bytes], dict[str, Any]]:
        return self.invoke(command, *self.target, "--destination", str(destination), *arguments)

    def records(self) -> dict[str, bytes]:
        result, value = self.invoke("records", *self.target)
        assert result.returncode == 0, (result.stderr, value)
        assert all(item["bytes"] == item["canonical"] for item in value["records"])
        return {item["key"]: base64.b64decode(item["bytes"]) for item in value["records"]}

    def normalized_records(self) -> dict[str, object]:
        return {
            key: normalized_record(json.loads(raw), self.root)
            for key, raw in self.records().items()
        }

    def files(self, *paths: Path) -> list[object]:
        return [
            [
                normalized_path(path.as_posix(), self.root),
                path.read_bytes().hex() if path.exists() else None,
                path.stat().st_mode & 0o777 if path.exists() else None,
            ]
            for path in paths
        ]

    def step(self, value: dict[str, Any], *paths: Path) -> dict[str, object]:
        return {
            "result": comparable(value, self.root),
            "files": self.files(*paths),
            "records": self.normalized_records(),
        }


def compare_languages(tmp_path: Path, scenario: Callable[[Application], list[object]]) -> None:
    binary = rust_example()
    root = tmp_path / "case"
    outcomes = []
    for implementation in LANGUAGES:
        root.mkdir()
        outcomes.append(scenario(Application(implementation, binary, root, root / "state")))
        shutil.rmtree(root)
    assert outcomes[0] == outcomes[1]


def resource_records(records: dict[str, object], name: str) -> list[object]:
    return [value for key, value in records.items() if key.endswith(f"/{name}")]


@pytest.mark.parametrize(
    "name,raw,type_name",
    FIXTURE_RECORDS,
    ids=[name for name, _, _ in FIXTURE_RECORDS],
)
def test_fixture_records_are_byte_identical(
    tmp_path: Path, name: str, raw: bytes, type_name: str
) -> None:
    binary = rust_example()
    vector = prepared(name, raw, tmp_path)
    python_result = run_driver(vector)
    rust_result = run_rust(binary, type_name, vector)
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    assert python_result.stdout == vector
    assert rust_result.stdout == vector
    assert run_driver(rust_result.stdout).stdout == vector
    assert run_rust(binary, type_name, python_result.stdout).stdout == vector


def test_fixture_records_cover_every_state_record_type() -> None:
    assert {type_name for _, _, type_name in FIXTURE_RECORDS} == {
        "Journal",
        "Preparation",
        "Receipt",
    }


def test_both_drivers_reject_an_unknown_state_record() -> None:
    binary = rust_example()
    python_result = run_driver(REJECTED)
    assert python_result.returncode == 1
    assert python_result.stdout == b""
    assert b"unknown state record" in python_result.stderr
    rust_result = run_rust(binary, "Receipt", REJECTED)
    assert rust_result.returncode == 1
    assert rust_result.stdout == b""
    assert b"decode Receipt: unknown state record" in rust_result.stderr


def test_rust_driver_rejects_a_record_of_another_type(tmp_path: Path) -> None:
    binary = rust_example()
    receipts = [item for item in FIXTURE_RECORDS if item[2] == "Receipt"]
    name, raw, _ = receipts[0]
    vector = prepared(name, raw, tmp_path)
    result = run_rust(binary, "Journal", vector)
    assert result.returncode == 1
    assert result.stdout == b""
    assert b"decode Journal:" in result.stderr


def test_normalization_zeroes_transaction_ids_and_case_roots(tmp_path: Path) -> None:
    identifier = "0123456789abcdef0123456789abcdef"
    key = "f" * 64
    staged = tmp_path / "state" / "staging" / f"{key}-{identifier}"
    fallback = tmp_path / f".flyrail-{key}-{identifier}.txn"
    record = {
        "type": "Preparation",
        "fields": {
            "directory": {"path": staged.as_posix()},
            "transaction_id": identifier,
            "resource": {"type": "Receipt", "fields": {"transaction_id": identifier}},
        },
    }
    assert normalized_record(record, tmp_path) == {
        "type": "Preparation",
        "fields": {
            "directory": {"path": f"$ROOT/state/staging/{key}-{ZERO_ID}"},
            "transaction_id": ZERO_ID,
            "resource": {"type": "Receipt", "fields": {"transaction_id": ZERO_ID}},
        },
    }
    assert normalized_path((fallback / "staging").as_posix(), tmp_path) == (
        f"$ROOT/.flyrail-{key}-{ZERO_ID}.txn/staging"
    )
    assert comparable({"message": "text", "path": staged.as_posix()}, tmp_path) == {
        "path": f"$ROOT/state/staging/{key}-{ZERO_ID}"
    }


def test_python_and_rust_take_over_update_and_remove_alike(tmp_path: Path) -> None:
    def scenario(app: Application) -> list[object]:
        destination = app.root / "AGENTS.md"
        original = b"original\n"
        destination.write_bytes(original)
        destination.chmod(0o600)
        steps: list[object] = []
        written, value = app.write(
            "write", destination, "--payload", "first\n", "--acquisition", "takeover"
        )
        assert_success(written, value)
        assert destination.read_bytes() == b"first\n"
        steps.append(app.step(value, destination))
        inspected, observation = app.invoke("inspect", *app.target)
        assert inspected.returncode == 0, inspected.stderr
        assert observation["is_current"] is True
        assert observation["version"] == "1"
        steps.append(comparable(observation, app.root))
        updated, value = app.write("update", destination, "--version", "2", "--payload", "second\n")
        assert_success(updated, value)
        assert destination.read_bytes() == b"second\n"
        steps.append(app.step(value, destination))
        removed, value = app.invoke("remove", *app.target)
        assert_success(removed, value)
        assert destination.read_bytes() == original
        assert destination.stat().st_mode & 0o777 == 0o600
        steps.append(app.step(value, destination))
        return steps

    compare_languages(tmp_path, scenario)


def test_python_and_rust_adopt_equal_bytes_alike(tmp_path: Path) -> None:
    def scenario(app: Application) -> list[object]:
        destination = app.root / "adopted"
        destination.write_bytes(b"same bytes")
        identity = (destination.stat().st_dev, destination.stat().st_ino)
        written, value = app.write(
            "write", destination, "--payload", "same bytes", "--acquisition", "adopt"
        )
        assert_success(written, value)
        assert (destination.stat().st_dev, destination.stat().st_ino) == identity
        receipts = [decode(raw) for key, raw in app.records().items() if key.endswith("/receipt")]
        assert len(receipts) == 1
        receipt = receipts[0]
        assert isinstance(receipt, Receipt)
        assert receipt.claims[0].disposition == "adopted"
        steps: list[object] = [app.step(value, destination)]
        removed, value = app.invoke("remove", *app.target)
        assert_success(removed, value)
        assert destination.exists() is False
        steps.append(app.step(value, destination))
        return steps

    compare_languages(tmp_path, scenario)


def test_python_and_rust_restore_each_baseline_on_retarget(tmp_path: Path) -> None:
    def scenario(app: Application) -> list[object]:
        first = app.root / "first"
        second = app.root / "second"
        first.write_bytes(b"first baseline")
        second.write_bytes(b"second baseline")
        first.chmod(0o600)
        second.chmod(0o640)
        steps: list[object] = []
        for destination, version, data in (
            (first, "1", "first generated"),
            (second, "2", "second generated"),
        ):
            result, value = app.write(
                "update",
                destination,
                "--version",
                version,
                "--payload",
                data,
                "--acquisition",
                "takeover",
            )
            assert_success(result, value)
            steps.append(app.step(value, first, second))
        assert first.read_bytes() == b"first baseline"
        assert first.stat().st_mode & 0o777 == 0o600
        assert second.read_bytes() == b"second generated"
        inspected, observation = app.invoke("inspect", *app.target)
        assert inspected.returncode == 0 and observation["version"] == "2"
        steps.append(comparable(observation, app.root))
        removed, value = app.invoke("remove", *app.target)
        assert_success(removed, value)
        assert second.read_bytes() == b"second baseline"
        assert second.stat().st_mode & 0o777 == 0o640
        steps.append(app.step(value, first, second))
        return steps

    compare_languages(tmp_path, scenario)


@pytest.mark.parametrize("phase", ["prepare", "receipt-committed"])
def test_python_and_rust_recover_an_interrupted_writer_alike(tmp_path: Path, phase: str) -> None:
    def scenario(app: Application) -> list[object]:
        destination = app.root / "large"
        destination.write_bytes(b"baseline")
        steps: list[object] = []
        initial, value = app.write(
            "write", destination, "--payload", "installed", "--acquisition", "takeover"
        )
        assert_success(initial, value)
        ready = app.root / "ready"
        release = app.root / "release"
        child = subprocess.Popen(  # noqa: S603
            app.command(
                "update",
                "--destination",
                str(destination),
                "--version",
                "2",
                "--payload",
                "updated",
                "--barrier-phase",
                phase,
                "--barrier-ready",
                str(ready),
                "--barrier-release",
                str(release),
            ),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            wait_for_path(ready, child)
            records = app.normalized_records()
            assert len(resource_records(records, "preparation")) == 1
            assert len(resource_records(records, "journal")) == 1
            steps.append(records)
            competing, busy = app.write(
                "update",
                destination,
                "--version",
                "3",
                "--payload",
                "competing",
                "--lock-timeout",
                "0.01",
            )
            assert competing.returncode == 0
            assert busy["status"] == "failed"
            assert busy["error"]["code"] == "busy"
            steps.append(comparable(busy, app.root))
            child.kill()
            child.communicate(timeout=10)
            expected = b"installed" if phase == "prepare" else b"updated"
            assert destination.read_bytes() == expected
            inspected, observation = app.invoke("inspect", *app.target)
            assert inspected.returncode == 0
            assert observation["pending"] is True
            recovery_paths = observation["resources"][0]["recovery_paths"]
            assert [Path(path).parent for path in recovery_paths] == [app.state / "staging"]
            steps.append(comparable(observation, app.root))
            recovered, value = app.invoke("recover", *app.target)
            assert_success(recovered, value)
            steps.append(app.step(value, destination))
            inspected, observation = app.invoke("inspect", *app.target)
            assert inspected.returncode == 0
            assert observation["pending"] is True
            assert observation["version"] == "1"
            assert observation["resources"][0]["recovery_paths"] == []
            assert destination.read_bytes() == expected
            steps.append(comparable(observation, app.root))
            removed, value = app.invoke("remove", *app.target)
            assert_success(removed, value)
            assert destination.read_bytes() == b"baseline"
            steps.append(app.step(value, destination))
        finally:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=10)
        return steps

    compare_languages(tmp_path, scenario)


def test_python_and_rust_do_not_report_an_index_failure_clean(tmp_path: Path) -> None:
    def scenario(app: Application) -> list[object]:
        destination = app.root / "resource"
        destination.write_bytes(b"foreign")
        steps: list[object] = []
        initial, value = app.write(
            "write", destination, "--payload", "first", "--acquisition", "takeover"
        )
        assert_success(initial, value)
        result, value = app.write(
            "update",
            destination,
            "--version",
            "2",
            "--payload",
            "second",
            "--fail-index-completion",
        )
        assert result.returncode == 0
        assert value["status"] == "partial"
        assert value["error"]["code"] == "io_error"
        steps.append(app.step(value, destination))
        inspected, observation = app.invoke("inspect", *app.target)
        assert inspected.returncode == 0
        assert observation["is_current"] is False
        assert observation["pending"] is True
        assert observation["version"] == "1"
        assert destination.read_bytes() == b"second"
        steps.append(comparable(observation, app.root))
        recovered, value = app.invoke("recover", *app.target)
        assert_success(recovered, value)
        steps.append(app.step(value, destination))
        inspected, observation = app.invoke("inspect", *app.target)
        assert inspected.returncode == 0
        assert observation["is_current"] is False
        assert observation["pending"] is True
        assert observation["version"] == "1"
        assert observation["resources"][0]["recovery_paths"] == []
        assert destination.read_bytes() == b"second"
        steps.append(comparable(observation, app.root))
        removed, value = app.invoke("remove", *app.target)
        assert_success(removed, value)
        assert destination.read_bytes() == b"foreign"
        steps.append(app.step(value, destination))
        return steps

    compare_languages(tmp_path, scenario)


def refused_foreign_security(app: Application, destination: Path) -> list[object]:
    result, value = app.write(
        "write", destination, "--payload", "generated", "--acquisition", "takeover"
    )
    assert result.returncode == 0
    assert value["status"] == "failed"
    assert value["error"]["code"] == "unsupported"
    assert destination.read_bytes() == b"foreign"
    assert destination.stat().st_mode & 0o777 == 0o640
    return [app.step(value, destination)]


@pytest.mark.parametrize("attribute", ["acl", "xattr"])
@pytest.mark.skipif(sys.platform != "darwin", reason="native macOS ACL and xattrs")
def test_python_and_rust_refuse_foreign_macos_security_alike(
    tmp_path: Path, attribute: str
) -> None:
    def scenario(app: Application) -> list[object]:
        destination = app.root / attribute
        destination.write_bytes(b"foreign")
        destination.chmod(0o640)
        if attribute == "acl":
            command = ["chmod", "+a", "everyone allow read", str(destination)]
            cleanup = ["chmod", "-N", str(destination)]
        else:
            command = ["xattr", "-w", "flyrail.interop", "value", str(destination)]
            cleanup = ["xattr", "-d", "flyrail.interop", str(destination)]
        subprocess.run(command, check=True, capture_output=True)  # noqa: S603
        try:
            return refused_foreign_security(app, destination)
        finally:
            subprocess.run(cleanup, check=True, capture_output=True)  # noqa: S603

    compare_languages(tmp_path, scenario)


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux xattrs")
def test_python_and_rust_refuse_foreign_linux_xattrs_alike(tmp_path: Path) -> None:
    probe = tmp_path / "probe"
    probe.write_bytes(b"")
    try:
        os.setxattr(probe, "user.flyrail.interop", b"value")  # type: ignore[attr-defined,unused-ignore]
    except OSError as error:
        pytest.skip(f"filesystem does not support user xattrs: {error}")

    def scenario(app: Application) -> list[object]:
        destination = app.root / "xattr"
        destination.write_bytes(b"foreign")
        destination.chmod(0o640)
        os.setxattr(destination, "user.flyrail.interop", b"value")  # type: ignore[attr-defined,unused-ignore]
        steps = refused_foreign_security(app, destination)
        assert os.getxattr(destination, "user.flyrail.interop") == b"value"  # type: ignore[attr-defined,unused-ignore]
        return steps

    compare_languages(tmp_path, scenario)


def section_block(marker: str, text: str) -> bytes:
    return f"<!-- flyrail:{marker}:start -->\n{text}<!-- flyrail:{marker}:end -->\n".encode()


@pytest.mark.parametrize("first", LANGUAGES)
def test_python_and_rust_applications_share_a_file_through_separate_stores(
    tmp_path: Path, first: str
) -> None:
    binary = rust_example()
    second = "rust" if first == "python" else "python"
    apps = {
        name: Application(name, binary, tmp_path, tmp_path / f"{name}-state") for name in LANGUAGES
    }
    shared = tmp_path / "AGENTS.md"
    baseline = b"# Project\n"
    shared.write_bytes(baseline)
    for name in (first, second):
        result, value = apps[name].write(
            "write", shared, "--section", name, "--payload", f"{name} guidance\n"
        )
        assert_success(result, value)
    data = shared.read_bytes()
    assert data.startswith(baseline)
    for name in LANGUAGES:
        assert section_block(name, f"{name} guidance\n") in data
        inspected, observation = apps[name].invoke("inspect", *apps[name].target)
        assert inspected.returncode == 0
        assert observation["is_current"] is True
    survivor = section_block(second, f"{second} guidance\n")
    removed, value = apps[first].invoke("remove", *apps[first].target)
    assert_success(removed, value)
    data = shared.read_bytes()
    assert section_block(first, f"{first} guidance\n") not in data
    assert data.count(survivor) == 1
    assert data.startswith(baseline)
    inspected, observation = apps[second].invoke("inspect", *apps[second].target)
    assert inspected.returncode == 0
    assert observation["is_current"] is True
    assert observation["resources"][0]["claims"][0]["disposition"] == "current"
    removed, value = apps[second].invoke("remove", *apps[second].target)
    assert_success(removed, value)
    assert shared.read_bytes() == baseline

    whole = tmp_path / "owned"
    for name in (first, second):
        apps[name] = Application(name, binary, tmp_path, tmp_path / f"{name}-whole-state")
    result, value = apps[first].write("write", whole, "--payload", f"{first} file\n")
    assert_success(result, value)
    whole_mode = whole.stat().st_mode & 0o777
    result, value = apps[second].write("write", whole, "--payload", f"{second} file\n")
    assert result.returncode == 0
    assert value["status"] == "failed"
    assert value["error"]["code"] == "conflict"
    assert whole.read_bytes() == f"{first} file\n".encode()
    assert whole.stat().st_mode & 0o777 == whole_mode
    inspected, observation = apps[first].invoke("inspect", *apps[first].target)
    assert inspected.returncode == 0
    assert observation["is_current"] is True


@pytest.mark.parametrize("implementation", LANGUAGES)
def test_both_drivers_hold_a_resource_lock_in_their_store(
    tmp_path: Path, implementation: str
) -> None:
    app = Application(implementation, rust_example(), tmp_path, tmp_path / "state")
    destination = tmp_path / "held"
    release = tmp_path / "release"
    child = subprocess.Popen(  # noqa: S603
        app.command(
            "hold",
            "--destination",
            str(destination),
            "--barrier",
            str(release),
            "--lock-timeout",
            "1",
        ),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        assert read_child_value(child) == {"status": "reserved"}
        result, value = app.write("write", destination, "--lock-timeout", "0.01")
        assert result.returncode == 0
        assert value["status"] == "failed"
        assert value["error"]["code"] == "busy"
        release.write_text("release\n")
        assert read_child_value(child) == {"status": "admitted"}
        stdout, stderr = child.communicate(timeout=10)
        assert child.returncode == 0, (stdout, stderr)
        result, value = app.write("write", destination)
        assert_success(result, value)
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=10)


@pytest.mark.parametrize("implementation", LANGUAGES)
@pytest.mark.parametrize(
    "arguments",
    [
        ("remove", "--state"),
        ("remove", "--state", "--replace-modified"),
        ("remove", "--unknown", "value"),
        ("remove", "--state", "one", "--state", "two"),
        ("remove", "--state", "state", "--lock-timeout", "-1"),
        ("remove", "--state", "state", "--lock-timeout", "nan"),
        ("remove", "--state", "state", "--lock-timeout", "inf"),
        (
            "write",
            "--state",
            "state",
            "--destination",
            "destination",
            "--large",
            "-1",
        ),
        (
            "write",
            "--state",
            "state",
            "--destination",
            "destination",
            "--large",
            str(1 << 64),
        ),
    ],
)
def test_both_drivers_reject_invalid_arguments(
    tmp_path: Path, implementation: str, arguments: tuple[str, ...]
) -> None:
    binary = rust_example()
    resolved = tuple(
        str(tmp_path / item) if item in {"state", "destination"} else item for item in arguments
    )
    result = subprocess.run(  # noqa: S603
        [*executable(implementation, binary), *resolved],
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert result.returncode != 101


@pytest.mark.parametrize("implementation", LANGUAGES)
def test_both_drivers_bound_large_payloads_without_tracebacks(
    tmp_path: Path, implementation: str
) -> None:
    binary = rust_example()
    result = subprocess.run(  # noqa: S603
        [
            *executable(implementation, binary),
            "write",
            "--state",
            str(tmp_path / "state"),
            "--destination",
            str(tmp_path / "destination"),
            "--large",
            str(MAX_PAYLOAD_SIZE + 1),
        ],
        capture_output=True,
        check=False,
    )
    assert result.returncode == 1
    assert b"Traceback" not in result.stderr
    value = cast(dict[str, Any], json.loads(result.stdout))
    assert value["error"] == {
        "code": None,
        "message": f"large payload size must not exceed {MAX_PAYLOAD_SIZE}",
        "path": None,
    }


@pytest.mark.parametrize("implementation", LANGUAGES)
def test_both_drivers_accept_the_maximum_large_payload(tmp_path: Path, implementation: str) -> None:
    app = Application(implementation, rust_example(), tmp_path, tmp_path / "state")
    destination = tmp_path / "destination"
    result, value = app.write("write", destination, "--large", str(MAX_PAYLOAD_SIZE))
    assert_success(result, value)
    data = destination.read_bytes()
    assert len(data) == MAX_PAYLOAD_SIZE
    assert data[:24] == b"flyrail-interop-payload\n"
    inspected, observation = app.invoke("inspect", *app.target)
    assert inspected.returncode == 0
    assert observation["is_current"] is True
    removed, value = app.invoke("remove", *app.target)
    assert_success(removed, value)
    assert destination.exists() is False


@pytest.mark.parametrize("implementation", LANGUAGES)
def test_both_drivers_accept_boolean_flags_in_any_order(
    tmp_path: Path, implementation: str
) -> None:
    binary = rust_example()
    result, value = invoke(
        implementation,
        binary,
        "write",
        "--replace-modified",
        "--state",
        str(tmp_path / "state"),
        "--destination",
        str(tmp_path / "destination"),
    )
    assert_success(result, value)
