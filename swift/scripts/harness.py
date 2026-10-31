import base64
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SWIFT = [str(ROOT / ".cache/swift/debug/FlyrailInterop")]
PYTHON = [sys.executable, "-B", str(ROOT / "python/scripts/interop_driver.py")]
RUST = [str(ROOT / "rust/target/debug/examples/interop")]
DRIVERS = (PYTHON, RUST, SWIFT)
ENV = os.environ | {"PYTHONPATH": str(ROOT / "python/src"), "PYTHONDONTWRITEBYTECODE": "1"}
ZERO_ID = "0" * 32
TRANSACTION_DIRECTORY = re.compile(r"((?:^|/)(?:\.flyrail-)?[0-9a-f]{64}-)[0-9a-f]{32}(?=\.txn|/|$)")


def use_python_paths(*directories):
    for directory in reversed(directories):
        path = str(ROOT / directory)
        if path not in sys.path:
            sys.path.insert(0, path)


def run(driver, *arguments, input=None, timeout=60):
    result = subprocess.run(
        driver + list(map(str, arguments)),
        input=input,
        capture_output=True,
        env=ENV,
        cwd=ROOT,
        timeout=timeout,
    )
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    return result.stdout


def driver_name(driver):
    return {id(PYTHON): "python", id(RUST): "rust", id(SWIFT): "swift"}.get(id(driver), " ".join(driver))


def driver_error(driver, result):
    if result.args[len(driver)] == "encode":
        return result.stderr.decode().strip()
    return json.loads(result.stdout)["error"]["message"]


def build_rust_examples(*names):
    subprocess.run(
        ["cargo", "build", "--locked", "--manifest-path", str(ROOT / "rust/Cargo.toml"),
         *[argument for name in names for argument in ["--example", name]],
         "--features", "interop-test-seams"],
        env=ENV | {"CARGO_HOME": str(ROOT / ".cache/cargo-home")}, cwd=ROOT, check=True, timeout=240,
    )


def tree_snapshot(root, modes=False):
    return {
        str(path): (path.stat().st_mode, path.read_bytes()) if modes else path.read_bytes()
        for path in Path(root).rglob("*") if path.is_file()
    }


def kill(process):
    if process.poll() is None:
        process.kill()
    stdout, stderr = process.communicate(timeout=15)
    assert process.returncode == -signal.SIGKILL, (process.returncode, stdout, stderr)


def wait_for(path, process, timeout=30):
    deadline = time.monotonic() + timeout
    while not Path(path).exists():
        assert process.poll() is None, process.communicate()
        assert time.monotonic() < deadline, f"timed out waiting for {path}"
        time.sleep(0.005)


def retry_busy(operation, timeout=10):
    from flyrail.stores import StoreBusy

    deadline = time.monotonic() + timeout
    while True:
        try:
            return operation()
        except StoreBusy:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.01)


def write_store_record(state, key, value, expected=None):
    from flyrail._state import write_record
    from flyrail.stores import DirectoryStore

    store = DirectoryStore(state)
    retry_busy(lambda: write_record(store, key, value, expected=expected))


def store_write(state, key, value, expected):
    from flyrail.stores import DirectoryStore

    store = DirectoryStore(state)
    assert retry_busy(lambda: store.write(key, value, expected=expected)), key


def store_delete(state, key, expected):
    from flyrail.stores import DirectoryStore

    store = DirectoryStore(state)
    assert retry_busy(lambda: store.delete(key, expected=expected)), key


def normalized_path(text, root):
    prefix = Path(root).as_posix()
    if text == prefix or text.startswith(prefix + "/"):
        text = "$ROOT" + text[len(prefix):]
    return TRANSACTION_DIRECTORY.sub(rf"\g<1>{ZERO_ID}", text)


def normalized_record(value, root):
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


def comparable(value, root):
    if isinstance(value, list):
        return [comparable(item, root) for item in value]
    if isinstance(value, dict):
        return {key: comparable(item, root) for key, item in value.items() if key != "message"}
    if isinstance(value, str):
        return normalized_path(value, root)
    return value


def record_paths(value, ancestor=False):
    if isinstance(value, list):
        return [found for item in value for found in record_paths(item, ancestor)]
    if not isinstance(value, dict):
        return []
    if set(value) == {"path"} and isinstance(value["path"], str):
        return [(value["path"], ancestor)]
    fields = value.get("fields")
    if value.get("type") == "Ancestor" and isinstance(fields, dict):
        return [found for key, item in fields.items() for found in record_paths(item, key == "path")]
    return [found for item in value.values() for found in record_paths(item)]


def assert_rooted(records, root):
    parents = {path.as_posix() for path in Path(root).parents}
    for key, value in records.items():
        for path, ancestor in record_paths(value):
            # Journal ancestors walk up to "/", above the case root.
            assert path.startswith("$ROOT") or (ancestor and path in parents), (key, path)


class Application:
    def __init__(self, driver, root, state):
        self.driver = driver
        self.root = Path(root)
        self.state = Path(state)

    @property
    def target(self):
        return ["--state", str(self.state)]

    def command(self, command, *arguments):
        return [*self.driver, command, *self.target, *map(str, arguments)]

    def invoke(self, command, *arguments, timeout=120):
        result = subprocess.run(
            [*self.driver, command, *map(str, arguments)],
            capture_output=True, env=ENV, cwd=ROOT, timeout=timeout,
        )
        return result, json.loads(result.stdout) if result.stdout.strip() else None

    def write(self, command, destination, *arguments):
        return self.invoke(command, *self.target, "--destination", destination, *arguments)

    def target_command(self, command, *arguments):
        return self.invoke(command, *self.target, *arguments)

    def records(self):
        result, value = self.invoke("records", *self.target)
        assert result.returncode == 0, (result.stderr, value)
        assert all(item["bytes"] == item["canonical"] for item in value["records"]), value
        return {item["key"]: base64.b64decode(item["bytes"]) for item in value["records"]}

    def normalized_records(self):
        records = {key: normalized_record(json.loads(raw), self.root) for key, raw in self.records().items()}
        assert_rooted(records, self.root)
        return records

    def files(self, *paths):
        return [
            [
                normalized_path(Path(path).as_posix(), self.root),
                Path(path).read_bytes().hex() if Path(path).exists() else None,
                Path(path).stat().st_mode & 0o777 if Path(path).exists() else None,
            ]
            for path in paths
        ]

    def step(self, value, *paths):
        return {
            "result": comparable(value, self.root),
            "files": self.files(*paths),
            "records": self.normalized_records(),
        }


def case_base(name):
    base = ROOT / ".cache" / name / uuid.uuid4().hex
    base.mkdir(parents=True)
    return base


def compare_languages(base, scenario, drivers=DRIVERS):
    root = Path(base) / "case"
    outcomes = []
    for driver in drivers:
        root.mkdir()
        try:
            outcomes.append(scenario(Application(driver, root, root / "state")))
        except AssertionError as error:
            raise AssertionError(f"{driver_name(driver)}: {error}") from error
        finally:
            shutil.rmtree(root)
    for driver, outcome in zip(drivers[1:], outcomes[1:], strict=True):
        assert outcome == outcomes[0], (driver_name(drivers[0]), driver_name(driver), outcomes[0], outcome)
    return outcomes[0]
