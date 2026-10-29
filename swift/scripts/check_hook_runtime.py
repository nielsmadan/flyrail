import argparse
import importlib
import json
import os
import shutil
import subprocess
from importlib.resources import files as package_files
from pathlib import Path

from harness import ROOT, use_python_paths

RENDERED_TESTS = {"test_hooks", "test_hook_lifecycle", "test_hook_schema_handover"}
COVERED_TESTS = {
    "test_neutral_host_protocol_executes_installed_bridge",
    "test_hook_destinations_and_source_free_upgrade",
    "test_real_extension_loader_works_in_commonjs_project",
}


def tools():
    binaries = [shutil.which(name) for name in ["node", "npm", "bun"]]
    assert all(binaries), "Node 24+, npm and Bun 1.4.2+ are required; integration cannot skip"
    versions = [
        subprocess.run([binary, "--version"], check=True, capture_output=True, text=True,
                       timeout=15).stdout.strip().removeprefix("v")
        for binary in binaries
    ]
    if os.environ.get("CI"):
        assert versions == ["24.21.0", "11.19.0", "1.4.2"], versions
    else:
        assert int(versions[0].split(".")[0]) >= 24, versions
        assert tuple(map(int, versions[2].split(".")[:3])) >= (1, 4, 2), versions
    return binaries


def parameters(item):
    callspec = getattr(item, "callspec", None)
    return {} if callspec is None else callspec.params


def install_hook_tooling(npm, environment):
    tooling = ROOT / ".cache/hook-tooling"
    pinned = (ROOT / "python/runtime-tooling/package-lock.json").read_bytes()
    lock = tooling / "package-lock.json"
    if lock.is_file() and lock.read_bytes() == pinned and (tooling / "node_modules/jiti").is_dir():
        return
    tooling.mkdir(parents=True, exist_ok=True)
    for name in ["package.json", "package-lock.json"]:
        shutil.copyfile(ROOT / "python/runtime-tooling" / name, tooling / name)
    subprocess.run(
        [npm, "ci", "--prefix", str(tooling), "--cache", str(ROOT / ".cache/npm"), "--ignore-scripts"],
        env=environment, check=True, timeout=300,
    )


def check_wires(runtime, wires, node, bun):
    harness = ROOT / "rust/tests/hook_runtime_harness.mjs"
    for binary in [node, bun]:
        for wire in json.loads(wires.read_bytes()):
            result = subprocess.run(
                [binary, str(harness)],
                input=json.dumps({
                    "action": "typed", "runtime": str(runtime), "cwd": str(runtime.parent),
                    "node": node, "input": wire["input"], "responses": wire["responses"],
                }),
                text=True, capture_output=True, check=True, timeout=15,
            )
            assert result.stderr == "", result.stderr
            observed = json.loads(result.stdout)
            for field in ["inputBase64", "responseBase64", "effectsBase64"]:
                assert observed.get(field) == wire[field], (binary, field, observed, wire)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--wires", type=Path, required=True)
    args = parser.parse_args()
    use_python_paths("python/tests", "python/scripts")
    node, npm, bun = tools()
    for name in ["runner.mts", "bridge.mts", "process.mts"]:
        raw = (args.runtime / name).read_bytes()
        assert raw == (ROOT / "python/src/flyrail/runtime" / name).read_bytes()
        assert raw == (ROOT / "rust/src/runtime" / name).read_bytes()
    check_wires(args.runtime, args.wires, node, bun)

    import flyrail.hooks
    import pytest
    from check import tool_environment
    from hook_render_adapter import SwiftHookRenderer

    environment = tool_environment()
    install_hook_tooling(npm, environment)

    def runtime_files(package):
        return args.runtime.parent if package == "flyrail" else package_files(package)

    tests = sorted((ROOT / "python/tests").glob("test_hook*.py"))
    renderer = SwiftHookRenderer(ROOT / ".cache/swift/debug/FlyrailInterop", flyrail.render)
    fixture_cases = set()
    host_scenarios = set()
    collected_hosts = set()

    class Coverage:
        def pytest_collection_modifyitems(self, items):
            collected_hosts.update(
                item.nodeid for item in items
                if item.originalname in COVERED_TESTS and "case" not in parameters(item)
            )

        @pytest.hookimpl(hookwrapper=True)
        def pytest_runtest_call(self, item):
            before = len(renderer.calls)
            yield
            if item.originalname in COVERED_TESTS:
                assert len(renderer.calls) > before, item.nodeid
                if "case" in parameters(item):
                    fixture_cases.add(parameters(item)["case"]["name"])
                else:
                    host_scenarios.add(item.nodeid)

    base = args.runtime.parent / "pytest"
    with pytest.MonkeyPatch.context() as patch:
        for key, value in environment.items():
            if os.environ.get(key) != value:
                patch.setenv(key, value)
        patch.setattr(flyrail.hooks, "files", runtime_files)
        patch.setattr(importlib.import_module("test_hook_execution"), "files", runtime_files)
        for path in tests:
            module = importlib.import_module(path.stem)
            if path.stem in RENDERED_TESTS:
                assert module.render is flyrail.render
                patch.setattr(module, "render", renderer)
        status = pytest.main([
            "-q", "--no-cov", "--basetemp", str(base), "-o", "cache_dir=" + str(base / "cache"),
            *map(str, tests),
        ], plugins=[Coverage()])
    assert status == 0, status
    expected = {case["name"] for case in json.loads(
        (ROOT / "spec/fixtures/hooks.json").read_bytes()
    )["cases"]}
    assert expected and fixture_cases == expected, expected ^ fixture_cases
    assert collected_hosts and host_scenarios == collected_hosts, host_scenarios ^ collected_hosts
    assert {host for host, _, _ in renderer.calls} == {"claude", "codex", "cursor", "copilot", "pi", "opencode"}
    print(f"Swift hook rendering: {len(renderer.calls)} host installations/graphs rendered by Swift; the shared "
          f"Python hook tests installed, synced and asserted them for all {len(expected)} fixture cases and "
          f"{len(host_scenarios)} host scenarios; Swift runtime copy byte-identical to Python/Rust with "
          "Node/Bun UTF-16 wire parity; isolated temporary cleanup passed")


if __name__ == "__main__":
    main()
