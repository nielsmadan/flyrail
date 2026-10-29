import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / ".cache"
PINS = {
    "swift-system": ("1.8.1", "869129b7bf4ecc57b97d0193ad29690ca2134750"),
    "swift-docc-plugin": ("1.5.0", "647c708be89f834fa6a6d4945442793a77ddf5b6"),
    "swift-docc-symbolkit": ("1.0.0", "b45d1f2ed151d057b54504d653e0da5552844e34"),
}

CONSUMER = r'''
import Flyrail
import Foundation

enum ProbeFailure: Error { case invalid }

@main struct Probe {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let node = CommandLine.arguments[2]
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        let script = """
        let s="";process.stdin.on("data",c=>s+=c);process.stdin.on("end",()=>process.stdout.write(JSON.stringify({version:1,outcome:"context",text:JSON.parse(s).tool})))
        """
        let manifest: [String: Any] = [
            "schema_version": 2, "id": "package-probe", "version": "1",
            "artifacts": [[
                "kind": "hook", "id": "check", "protocol_version": 1, "event": "before-tool",
                "command": ["argv": [node, "-e", script]], "outcomes": ["context"],
            ]],
        ]
        let bundle = try FlyrailBundle(manifest: JSONSerialization.data(withJSONObject: manifest), entries: [])
        let runtime = try HookRuntime(node: ResourcePath(node))
        guard runtime.resources.count == 3 else { throw ProbeFailure.invalid }
        let context = try RenderContext(target: .project(agent: .claude, root: ResourcePath(root.path)),
                                        platform: .macOS, surface: .cli).hookRuntime(runtime)
        let rendered = try render(bundle, for: context)
        guard rendered.supported else { throw ProbeFailure.invalid }
        let target = try InstallationTarget(indexRoot: ResourcePath(root.appending(path: "index").path))
        let result = try sync(bundle: bundle, rendered: rendered, target: target)
        guard result.status == .applied, result.observation.matches(bundle, rendered) else {
            throw ProbeFailure.invalid
        }
        let config = try JSONSerialization.jsonObject(
            with: Data(contentsOf: root.appending(path: ".claude/settings.json"))
        ) as? [String: Any]
        guard let hooks = config?["hooks"] as? [String: [[String: Any]]],
              let entry = (hooks["PreToolUse"]?.first?["hooks"] as? [[String: Any]])?.first,
              let command = entry["command"] as? String, let args = entry["args"] as? [String] else {
            throw ProbeFailure.invalid
        }
        let child = Process()
        let input = Pipe(), output = Pipe(), error = Pipe()
        child.executableURL = URL(fileURLWithPath: command)
        child.arguments = args
        child.currentDirectoryURL = root
        child.standardInput = input
        child.standardOutput = output
        child.standardError = error
        try child.run()
        try input.fileHandleForWriting.write(contentsOf: Data(#"{"tool_name":"Bash","tool_input":{}}"#.utf8))
        try input.fileHandleForWriting.close()
        let bytes = output.fileHandleForReading.readDataToEndOfFile()
        let errors = error.fileHandleForReading.readDataToEndOfFile()
        child.waitUntilExit()
        let expected = Data(#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"Bash"}}"#.utf8)
        guard child.terminationStatus == 0, errors.isEmpty,
              bytes.trimmingASCIIWhitespace() == expected else { throw ProbeFailure.invalid }
        let removed = remove(bundleID: "package-probe", target: target)
        guard removed.status == .applied,
              try !inspectInstallation(bundleID: "package-probe", target: target).isCurrent else {
            throw ProbeFailure.invalid
        }
        print("External Git consumer: packaged resources, render/install/execute/inspect/source-free removal passed")
    }
}

extension Data {
    func trimmingASCIIWhitespace() -> Data {
        Data(drop(while: { [9, 10, 13, 32].contains($0) }).reversed()
            .drop(while: { [9, 10, 13, 32].contains($0) }).reversed())
    }
}
'''


GIT_IDENTITY = [
    "-c", "user.name=Flyrail Package QA", "-c", "user.email=package-qa@flyrail.invalid",
    "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "core.excludesFile=/dev/null",
    "-c", "init.defaultBranch=main",
]
PACKAGE_FILES = [
    "Package.swift", "Package.resolved", "LICENSE", "README.md", "AGENTS.md",
    "swift/AGENTS.md", "swift/Justfile", "swift/.swiftformat", "swift/.swiftlint.yml",
]
PACKAGE_ROOTS = ["swift/Sources", "swift/Tests"]


def isolated_environment():
    inherited = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    return inherited | {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "CLANG_MODULE_CACHE_PATH": str(CACHE / "swiftpm/clang-modules"),
        "SWIFTPM_MODULECACHE_OVERRIDE": str(CACHE / "swiftpm/modules"),
        "TMPDIR": str(CACHE / "tmp"),
    }


def run(arguments, cwd, environment=None):
    result = subprocess.run(arguments, cwd=cwd, env=environment or isolated_environment(),
                            capture_output=True, text=True, timeout=600, check=False)
    if result.returncode:
        raise RuntimeError(f"{arguments}: {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def git(arguments, cwd):
    return run(["git", *GIT_IDENTITY, *arguments], cwd)


def tracked_files(*pathspecs):
    listed = run(["git", "ls-files", "-z", "--cached", "--", *pathspecs], ROOT, os.environ.copy())
    files = sorted({Path(name) for name in listed.split("\0") if name and (ROOT / name).exists()})
    assert all((ROOT / path).is_file() and not (ROOT / path).is_symlink() for path in files)
    return files


def committed_files(repository):
    listed = git(["ls-tree", "-r", "-z", "--name-only", "HEAD"], repository)
    return sorted(Path(name) for name in listed.split("\0") if name)


def snapshot_repository(files, destination):
    destination.mkdir()
    for path in files:
        (destination / path).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / path, destination / path)
    git(["init", "--quiet", str(destination)], destination.parent)
    git(["add", "--all"], destination)
    git(["commit", "--quiet", "--no-verify", "-m", "chore: snapshot package fixture"], destination)
    assert committed_files(destination) == files, set(committed_files(destination)) ^ set(files)
    return git(["rev-parse", "HEAD"], destination).strip()


def swift(arguments, root, scratch):
    flags = ["--disable-sandbox"] if os.environ.get("FLYRAIL_SWIFT_DISABLE_SANDBOX") == "1" else []
    return run([
        "swift", *arguments, *flags, "--package-path", str(root), "--scratch-path", str(scratch),
        "--cache-path", str(CACHE / "swiftpm/cache"),
        "--config-path", str(CACHE / "swiftpm/configuration"),
        "--security-path", str(CACHE / "swiftpm/security"),
    ], root)


def package_files():
    files = tracked_files(*PACKAGE_FILES, *PACKAGE_ROOTS)
    assert {str(path) for path in files} >= set(PACKAGE_FILES), set(PACKAGE_FILES) - {str(p) for p in files}
    return files


def verify_manifest(package, scratch):
    manifest = json.loads(swift(["package", "dump-package"], package, scratch))
    assert manifest["toolsVersion"]["_version"] == "6.2.0"
    assert manifest["platforms"] == [{"options": [], "platformName": "macos", "version": "13.0"}]
    assert [{key: item[key] for key in ["name", "targets", "type"]}
            for item in manifest["products"]] == [{
        "name": "Flyrail", "targets": ["Flyrail"], "type": {"library": ["automatic"]},
    }]
    assert manifest["swiftLanguageVersions"] == ["6"]
    dependencies = {
        item["sourceControl"][0]["identity"]:
        item["sourceControl"][0]["requirement"]["exact"][0]
        for item in manifest["dependencies"]
    }
    assert dependencies == {"swift-system": "1.8.1", "swift-docc-plugin": "1.5.0"}
    targets = {target["name"]: target for target in manifest["targets"]}
    assert set(targets) == {"Flyrail", "FlyrailInteropSupport", "FlyrailInterop", "FlyrailTests"}
    assert targets["Flyrail"]["path"] == "swift/Sources/Flyrail"
    assert targets["Flyrail"]["resources"] == [{"path": "Resources", "rule": {"copy": {}}}]
    pins = json.loads((package / "Package.resolved").read_bytes())
    assert {pin["identity"]: (pin["state"]["version"], pin["state"]["revision"])
            for pin in pins["pins"]} == PINS


def main():
    (CACHE / "tmp").mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="swift-package-", dir=CACHE / "tmp") as temporary:
        qa = Path(temporary)
        archive = qa / "Flyrail.tar.gz"
        files = package_files()
        hashes = {str(path): hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in files}
        with tarfile.open(archive, "w:gz") as package:
            for path in files:
                package.add(ROOT / path, arcname=str(path), recursive=False)
        extracted = qa / "extracted"
        extracted.mkdir()
        with tarfile.open(archive) as package:
            assert sorted(package.getnames()) == sorted(hashes)
            assert all(member.isfile() for member in package.getmembers())
            package.extractall(extracted, filter="data")
        assert {str(path.relative_to(extracted)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in extracted.rglob("*") if path.is_file()} == hashes
        for fixture in (ROOT / "spec/fixtures").glob("*.json"):
            assert (extracted / "swift/Tests/FlyrailTests/Fixtures" / fixture.name).read_bytes() == fixture.read_bytes()
        runtime = extracted / "swift/Sources/Flyrail/Resources/runtime"
        assert sorted(path.name for path in runtime.iterdir()) == ["bridge.mts", "process.mts", "runner.mts"]
        for path in runtime.iterdir():
            assert path.read_bytes() == (ROOT / "python/src/flyrail/runtime" / path.name).read_bytes()
        verify_manifest(extracted, qa / "manifest-build")
        swift(["build", "-Xswiftc", "-warnings-as-errors"], extracted, qa / "archive-build")
        swift(["test", "-Xswiftc", "-warnings-as-errors"], extracted, qa / "archive-build")
        repository_files = tracked_files()
        snapshot = qa / "repository"
        revision = snapshot_repository(repository_files, snapshot)
        consumer = qa / "consumer"
        (consumer / "Sources/Probe").mkdir(parents=True)
        (consumer / "Package.swift").write_text(
            '// swift-tools-version: 6.2\nimport PackageDescription\n'
            'let package = Package(name: "Probe", platforms: [.macOS(.v13)], dependencies: ['
            f'.package(name: "Flyrail", url: "{snapshot.as_uri()}", revision: "{revision}")'
            '], targets: [.executableTarget(name: "Probe", dependencies: ['
            '.product(name: "Flyrail", package: "Flyrail")])], swiftLanguageModes: [.v6])\n'
        )
        (consumer / "Sources/Probe/Probe.swift").write_text(CONSUMER)
        shutil.rmtree(snapshot / "swift/Sources")
        swift(["build", "-Xswiftc", "-warnings-as-errors"], consumer, qa / "consumer-build")
        node = shutil.which("node")
        assert node, "Node 24+ is required for the external consumer runtime probe"
        print(run([str(qa / "consumer-build/debug/Probe"), str(qa / "target"), node], consumer).strip())
        checkouts = list((qa / "consumer-build/checkouts").iterdir())
        package_checkout = next(path for path in checkouts if (path / "swift/Sources/Flyrail").is_dir())
        assert committed_files(package_checkout) == repository_files
        assert (package_checkout / "Package.swift").read_bytes() == (ROOT / "Package.swift").read_bytes()
        print(f"Swift package QA: {len(files)} tracked Swift package files archived and verified; exact "
              "manifest/dependency/resource pins; the extracted archive builds and tests without sibling "
              f"sources; an external SwiftPM consumer of a {len(repository_files)}-file Git snapshot of the "
              "tracked repository rendered, installed, executed and removed hooks")
    assert not qa.exists(), "package QA temporary files were retained"


if __name__ == "__main__":
    main()
