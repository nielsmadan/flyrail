import argparse
import json
import os
import platform
import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path

HARNESSES = ("claude", "codex", "cursor", "opencode", "pi")
SKILL = b"---\r\nname: review\r\ndescription: Check fixture\r\n---\r\n\r\nKeep {{literal}}.\r\n"
SKILL_ASSET = b"\x00\xff\x80\n"
MANIFEST = """# Owned by the fixture user
[[source]]
name = "local"
path = "."
use = ["instructions", "mcp", "skills", "hooks", "module-config"]

[claude]
instructions = ["local/welcome"]
hooks = ["local/welcome"]

[codex]
instructions = ["local/welcome"]
hooks = []

[cursor]
instructions = ["local/welcome"]
hooks = []

[opencode]
instructions = ["local/welcome"]

[pi]
instructions = ["local/welcome"]
"""
USER_HOOK = {
    "SessionStart": [
        {"hooks": [{"type": "command", "command": "echo fixture-user-hook"}]}
    ]
}
MCP = """# Owned by the fixture user
[user-local]
transport = "stdio"
command = "user-command"
"""
REGISTRY = "home/.config/loadout/config.toml"
SOURCE = "source space ü"
TOKEN = "FLYRAIL_INTEROP_TEST_TOKEN"
URL = "https://example.test/mcp"
LABEL = "Review changes with flyrail."
NATIVE = b"export default async function fixture() { return {}; }\n"
HANDLER = """import { readFileSync, writeFileSync } from 'node:fs';
const input = JSON.parse(readFileSync(0, 'utf8'));
writeFileSync(process.argv[2], JSON.stringify(input));
const response = input.event === 'before-tool'
  ? { version: 1, outcome: 'block', reason: 'review required' }
  : { version: 1, outcome: 'continue' };
process.stdout.write(JSON.stringify(response));
"""
SERVER = """import { readFileSync } from 'node:fs';
const request = JSON.parse(readFileSync(0, 'utf8'));
process.stdout.write(JSON.stringify({
  jsonrpc: '2.0', id: request.id, result: {
    protocolVersion: '2025-11-25', capabilities: {},
    serverInfo: { name: process.env.FLYRAIL_INTEROP_MCP_NAME, version: '1' }
  }
}) + String.fromCharCode(10));
"""
SKILL_LOCATIONS = (
    ".claude/skills",
    ".codex/skills",
    ".cursor/skills",
    ".config/opencode/skills",
    ".pi/agent/skills",
    ".agents/skills",
)
INSTRUCTIONS = (
    ".claude/CLAUDE.md",
    ".codex/AGENTS.md",
    ".cursor/rules/loadout.mdc",
    ".config/opencode/AGENTS.md",
    ".pi/agent/AGENTS.md",
)


def write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(content, str):
        path.write_text(content, encoding="utf-8")
    else:
        path.write_bytes(content)


def dump(path, value):
    write(path, json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def fixture(root):
    source = root / SOURCE
    write(root / REGISTRY, f"source = '{source}'\n")
    write(source / "loadout.toml", MANIFEST)
    write(source / "instructions/welcome.md", "Existing fixture instructions.\n")
    dump(source / "hooks/welcome.json", USER_HOOK)
    write(source / "mcp.toml", MCP)


def command_capture(command, request, cwd, name, evidence, env=None):
    dump(evidence / f"{name}.request.json", request)
    result = subprocess.run(
        command,
        input=json.dumps(request) + "\n",
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=20,
        cwd=cwd,
        env=env,
        check=False,
    )
    write(evidence / f"{name}.stdout", result.stdout)
    write(evidence / f"{name}.stderr", result.stderr)
    write(evidence / f"{name}.status", f"{result.returncode}\n")
    assert result.returncode == 0, (name, result.returncode, result.stderr)
    assert result.stderr == "", (name, result.stderr)
    return json.loads(result.stdout)


def bundle_fixtures(root, node):
    def hook(event):
        return {
            "kind": "hook",
            "id": event,
            "event": event,
            "protocol_version": 1,
            "command": {
                "argv": [
                    node,
                    {"asset": "handler", "path": "handler.mjs"},
                    str(root / "canonical.json"),
                ]
            },
            "outcomes": ["block" if event == "before-tool" else "continue"],
        }

    def native(agent, destination):
        return {
            "kind": "native",
            "id": f"native-{agent}",
            "family": "hooks",
            "audience": {"agent": agent, "scope": "user"},
            "destination": destination,
            "content": {"kind": "file", "path": "native-fixture.mjs"},
        }

    handler = {"id": "handler", "family": "hooks", "path": "handler"}
    for bundle_id, events in (
        ("interop", ("before-tool", "session-start", "after-tool")),
        ("extra", ("prompt", "stop")),
    ):
        bundle = root / bundle_id
        assets = [handler]
        dependencies = [{"dependent": event, "required": "handler"} for event in events]
        artifacts = [hook(event) for event in events]
        write(bundle / "handler/handler.mjs", HANDLER)
        if bundle_id == "interop":
            artifacts += [
                {"kind": "instruction", "id": "guide", "text": LABEL},
                {"kind": "skill", "id": "skill", "name": "review", "path": "review"},
                {
                    "kind": "mcp",
                    "id": "stdio",
                    "name": "review-local",
                    "transport": {
                        "kind": "stdio",
                        "command": {
                            "argv": [
                                node,
                                {"asset": "server", "path": "server.mjs"},
                                "--fixture stdio",
                            ],
                            "env": {"FLYRAIL_INTEROP_MCP_NAME": "interop fixture"},
                        },
                    },
                },
                {
                    "kind": "mcp",
                    "id": "http",
                    "name": "review-http",
                    "transport": {
                        "kind": "http",
                        "url": URL,
                        "bearer_token_env": TOKEN,
                    },
                },
                native("opencode", ".config/opencode/plugins/native-fixture.mjs"),
                native("pi", ".pi/agent/extensions/native-fixture.mjs"),
            ]
            assets.append({"id": "server", "family": "mcp", "path": "server"})
            dependencies.append({"dependent": "stdio", "required": "server"})
            write(bundle / "review/SKILL.md", SKILL)
            write(bundle / "review/assets/sample.bin", SKILL_ASSET)
            write(bundle / "server/server.mjs", SERVER)
            write(bundle / "native-fixture.mjs", NATIVE)
        dump(
            bundle / "flyrail.json",
            {
                "schema_version": 2,
                "id": bundle_id,
                "version": "1",
                "artifacts": artifacts,
                "assets": assets,
                "dependencies": dependencies,
            },
        )


def flyrail_phase(mode, root, node):
    from flyrail import (
        Agent,
        Bundle,
        DirectoryStore,
        HookRuntime,
        HookShell,
        LoadoutLocation,
        OperationStatus,
        Platform,
        apply_loadout,
        discover_loadout,
        inspect_loadout,
        preview_loadout,
        preview_loadout_removal,
    )

    source = root / SOURCE
    store = DirectoryStore(root / "state")
    if mode == "remove":
        location = LoadoutLocation(source, "default", "local", source)
        for bundle_id in ("extra", "interop"):
            assert inspect_loadout(bundle_id, location, store).is_current
            removal = preview_loadout_removal(bundle_id, location, store)
            assert removal.applicable, removal.lifecycle
            result = apply_loadout(removal)
            assert (
                result.lifecycle.status is OperationStatus.APPLIED
                and result.sync_required
            ), result
        return

    settings = {
        "platform": Platform.MACOS if platform.system() == "Darwin" else Platform.LINUX,
        "hook_runtime": HookRuntime(node=Path(node), shell=HookShell.POSIX),
        "execution_root": root,
    }
    for bundle_id in ("interop", "extra"):
        bundle = Bundle.from_directory(root / bundle_id)
        agents = [
            Agent(agent)
            for agent in HARNESSES
            if bundle_id == "interop" or agent != "opencode"
        ]
        original = (source / "loadout.toml").read_bytes()
        for expected in (OperationStatus.APPLIED, OperationStatus.UNCHANGED):
            setup = discover_loadout(root / "home", env={}).setup
            assert setup is not None
            preview = preview_loadout(
                bundle, setup.select("local", agents), store, **settings
            )
            assert preview.applicable, (preview.diagnostic, preview.lifecycle.error)
            assert (source / "loadout.toml").read_bytes() == original
            result = apply_loadout(preview)
            assert result.lifecycle.status is expected, result
            if expected is OperationStatus.APPLIED:
                assert result.sync_required
                assert inspect_loadout(bundle.id, preview.location, store).is_current
                original = (source / "loadout.toml").read_bytes()


def loadout_phase(root, snapshot):
    from loadout import __version__
    from loadout.destinations import resolve_destination
    from loadout.emit import RenderContext, render_global
    from loadout.machine import registered_at
    from loadout.output import Copied, FrozenFile, Merged

    assert __version__ == "0.11.0", __version__
    home = root / "home"
    source = root / SOURCE

    def destination(template, label):
        path = resolve_destination(template, label, {"HOME": str(home)})
        assert path.resolve().is_relative_to(home), path
        return path

    def no_upstream(name):
        raise AssertionError(f"Fixture unexpectedly requested an upstream kit: {name}")

    context = RenderContext(
        destination=destination,
        upstream=no_upstream,
        registration=registered_at(source),
    )
    for path, output in render_global(source, context=context).items():
        relative = path.relative_to(home)
        if isinstance(output, Merged):
            content = output.document
        elif isinstance(output, Copied):
            content = output.read_bytes()
        elif isinstance(output, FrozenFile):
            content = output.content
        else:
            assert isinstance(output, str), output
            content = output
        write(snapshot / relative, content)
    print(
        "render_global outputs",
        *(
            sorted(
                str(x.relative_to(snapshot)) for x in snapshot.rglob("*") if x.is_file()
            )
        ),
        sep="\n",
    )


def verify_baseline(snapshot, *, installed):
    for path in INSTRUCTIONS:
        instructions = (snapshot / path).read_text()
        assert "Existing fixture instructions." in instructions, path
        if installed:
            assert instructions.count(LABEL) == 1 and instructions.index(
                LABEL
            ) > instructions.index("Existing fixture instructions."), path
    claude = json.loads((snapshot / ".claude/settings.json").read_text())
    assert claude["hooks"]["SessionStart"][0] == USER_HOOK["SessionStart"][0]


def verify_skills(snapshot):
    installed = {
        path for path in SKILL_LOCATIONS if (snapshot / path / "review").is_dir()
    }
    assert installed == set(SKILL_LOCATIONS) - {".agents/skills"}, installed
    for relative in installed:
        assert (snapshot / relative / "review/SKILL.md").read_bytes() == SKILL, relative
        assert (
            snapshot / relative / "review/assets/sample.bin"
        ).read_bytes() == SKILL_ASSET, relative


def verify_mcp(snapshot, node, root, evidence):
    claude = json.loads((snapshot / ".claude.json").read_text())["mcpServers"]
    codex = tomllib.loads((snapshot / ".codex/config.toml").read_text())["mcp_servers"]
    cursor = json.loads((snapshot / ".cursor/mcp.json").read_text())["mcpServers"]
    opencode = json.loads((snapshot / ".config/opencode/opencode.json").read_text())[
        "mcp"
    ]
    pi = json.loads((snapshot / ".pi/agent/mcp.json").read_text())["mcpServers"]
    for name, servers, header in (
        ("claude", claude, f"Bearer ${{{TOKEN}}}"),
        ("codex", codex, None),
        ("cursor", cursor, f"Bearer ${{env:{TOKEN}}}"),
        ("opencode", opencode, f"Bearer {{env:{TOKEN}}}"),
        ("pi", pi, f"Bearer ${{{TOKEN}}}"),
    ):
        assert set(servers) == {"user-local", "review-local", "review-http"}, name
        local, http = servers["review-local"], servers["review-http"]
        if name == "opencode":
            assert local["type"] == "local"
            argv = local["command"]
            environment = local["environment"]
        else:
            argv = [local["command"], *local["args"]]
            environment = local["env"]
        assert argv[0] == node and argv[2:] == ["--fixture stdio"], (name, argv)
        assert Path(argv[1]).read_bytes() == SERVER.encode(), (name, argv)
        assert environment == {"FLYRAIL_INTEROP_MCP_NAME": "interop fixture"}, name
        assert http["url"] == URL, name
        if header is None:
            assert http["bearer_token_env_var"] == TOKEN, name
        else:
            assert http["headers"] == {"Authorization": header}, name
    result = command_capture(
        [claude["review-local"]["command"], *claude["review-local"]["args"]],
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        root,
        "mcp-initialize",
        evidence,
        env={"PATH": os.environ["PATH"], **claude["review-local"]["env"]},
    )
    assert result == {
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "serverInfo": {"name": "interop fixture", "version": "1"},
        },
    }


def verify_hooks(snapshot, node, bun, root, evidence, repository, hook_tools):
    cases = json.loads((repository / "spec/fixtures/hooks.json").read_text())["cases"]
    configuration = {
        "claude": ".claude/settings.json",
        "codex": ".codex/hooks.json",
        "cursor": ".cursor/hooks.json",
        "opencode": ".config/opencode/plugins",
        "pi": ".pi/agent/extensions",
    }
    for agent in ("opencode", "pi"):
        assert (
            snapshot / configuration[agent] / "native-fixture.mjs"
        ).read_bytes() == NATIVE
    selected = [
        case
        for case in cases
        if case["host"] in HARNESSES
        and (
            case["event"] == "before-tool"
            and case["response"]["outcome"] == "block"
            or case["event"] != "before-tool"
            and case["response"]["outcome"] == "continue"
        )
    ]
    assert len(selected) == 23
    for case in selected:
        agent = case["host"]
        payload = case["native_input"]
        expected = case["expected"]
        event = case["native_event"]
        target = snapshot / configuration[agent]
        capture = root / "canonical.json"
        capture.unlink(missing_ok=True)
        env = {"PATH": os.environ["PATH"], "FLYRAIL_HOOK_TOOLS": str(hook_tools)}
        if agent in {"opencode", "pi"}:
            bundle_id = "extra" if case["event"] in {"prompt", "stop"} else "interop"
            [entry] = [
                module
                for module in target.glob("flyrail-*.ts")
                if f"/.flyrail-assets/{bundle_id}/" in module.read_text()
            ]
            output = case["native_output"]
            request = {
                "host": agent,
                "entry": str(entry),
                "event": event,
                "input": payload,
                "output": output,
                "context": {"cwd": str(root), "mode": "print", "hasUI": False},
            }
            command = [
                node if agent == "pi" else bun,
                str(repository / "python/tests/hook_host.mjs"),
            ]
        else:
            entries = json.loads(target.read_text())["hooks"][event]
            [registration] = [
                entry for entry in entries if ".flyrail-assets" in json.dumps(entry)
            ]
            if agent == "cursor":
                assert registration["type"] == "command", registration
                command = ["/bin/sh", "-c", registration["command"]]
            else:
                [registration] = registration["hooks"]
                assert registration["type"] == "command", registration
                command = (
                    [registration["command"], *registration["args"]]
                    if agent == "claude"
                    else ["/bin/sh", "-c", registration["command"]]
                )
            request = payload

        name = case["name"]
        actual = command_capture(command, request, root, name, evidence, env=env)
        if agent in {"pi", "opencode"}:
            assert event in actual["registered"], (agent, actual)
            actual = {key: actual[key] for key in expected}
        assert actual == expected, (agent, actual, expected)
        packet = json.loads(capture.read_text())
        assert packet["version"] == 1 and packet["event"] == case["event"], (
            agent,
            packet,
        )
        assert {key: packet.get(key) for key in case["canonical"]} == case[
            "canonical"
        ], (
            name,
            packet,
        )
        context = dict(case["context"])
        if agent in {"pi", "opencode"}:
            context["cwd"] = str(root)
        if agent == "pi":
            context.update(execution_mode="print", has_ui=False)
        assert packet["context"] == context, (name, packet)
        write(evidence / f"{name}.canonical.json", capture.read_bytes())
        print(
            f"{name}: rendered registration ran handler and returned expected native response"
        )


def verify_removal(baseline, after):
    before_paths = {
        path.relative_to(baseline) for path in baseline.rglob("*") if path.is_file()
    }
    after_paths = {
        path.relative_to(after) for path in after.rglob("*") if path.is_file()
    }
    assert before_paths == after_paths, (before_paths, after_paths)
    for relative in before_paths:
        assert (baseline / relative).read_bytes() == (after / relative).read_bytes(), (
            relative
        )


def producer_command(producer, repository, root):
    test = "TestLoadoutInteropProducer"
    if producer == "go":
        return [
            "just",
            "--command",
            "go",
            "test",
            "./flyrail",
            "-run",
            f"^{test}$",
            "-count=1",
            "-v",
        ], repository / "go"
    if producer == "rust":
        return [
            "just",
            "--command",
            "cargo",
            "test",
            "--test",
            "loadout_interop",
            "--",
            "--ignored",
            "--nocapture",
        ], repository / "rust"
    swift = ["swift", "test"]
    if os.environ.get("FLYRAIL_SWIFT_DISABLE_SANDBOX") == "1":
        swift.append("--disable-sandbox")
    for flag, name in (
        ("--scratch-path", "swift-build"),
        ("--cache-path", "swift-cache"),
        ("--config-path", "swift-config"),
        ("--security-path", "swift-security"),
    ):
        swift.extend([flag, str(root / name)])
    return [
        *swift,
        "-Xswiftc",
        "-warnings-as-errors",
        "--filter",
        "loadoutInteropProducer",
    ], repository


def run_producer(producer, args, repository, loadout, evidence, node, bun):
    evidence.mkdir(parents=True, exist_ok=False)
    flyrail_python = args.flyrail_python or repository / "python/.venv/bin/python"
    loadout_python = args.loadout_python or loadout / ".venv/bin/python"
    hook_tools = args.hook_tools or repository / ".cache/hook-tooling"

    with tempfile.TemporaryDirectory(prefix="fixture-", dir=evidence) as workspace:
        root = Path(workspace).resolve()
        fixture(root)
        bundle_fixtures(root, node)

        def phase(mode, label):
            consumer = mode == "render"
            env = {
                "PATH": os.environ["PATH"],
                "PYTHONPATH": str(
                    loadout / "src" if consumer else repository / "python/src"
                ),
                "PYTHONDONTWRITEBYTECODE": "1",
            }
            cwd = root
            timeout = 120
            if consumer or producer == "python":
                command = [
                    str(loadout_python if consumer else flyrail_python),
                    str(Path(__file__).resolve()),
                    "--repository",
                    str(repository),
                    "--loadout",
                    str(loadout),
                    "--evidence",
                    str(evidence),
                    "--phase",
                    mode,
                    "--root",
                    str(root),
                    "--snapshot",
                    str(evidence / label),
                ]
            else:
                command, cwd = producer_command(producer, repository, root)
                command = ["timeout", "600", *command]
                timeout = 615
                temporary = root / "tmp"
                temporary.mkdir(exist_ok=True)
                env = {
                    **os.environ,
                    "TMPDIR": str(temporary),
                    "FLYRAIL_LOADOUT_INTEROP_ROOT": str(root),
                    "FLYRAIL_LOADOUT_INTEROP_NODE": node,
                    "FLYRAIL_LOADOUT_INTEROP_PHASE": mode,
                    "FLYRAIL_LOADOUT_INTEROP_SOURCE": str(root / SOURCE),
                }
            outcome = subprocess.run(
                command,
                env=env,
                text=True,
                encoding="utf-8",
                capture_output=True,
                timeout=timeout,
                cwd=cwd,
                check=False,
            )
            write(evidence / f"{label}.stdout", outcome.stdout)
            write(evidence / f"{label}.stderr", outcome.stderr)
            write(evidence / f"{label}.status", f"{outcome.returncode}\n")
            assert outcome.returncode == 0, (
                producer,
                mode,
                outcome.stdout,
                outcome.stderr,
            )
            print(outcome.stdout, end="", flush=True)

        phase("render", "baseline")
        verify_baseline(evidence / "baseline", installed=False)
        phase("install", "install")
        phase("render", "installed")
        installed = evidence / "installed"
        verify_baseline(installed, installed=True)
        verify_skills(installed)
        verify_mcp(installed, node, root, evidence)
        verify_hooks(
            installed, node, bun, root, evidence, repository, hook_tools.resolve()
        )
        registration = root / REGISTRY
        write(registration, "source = '~/absent'\n")
        shutil.rmtree(root / "interop")
        shutil.rmtree(root / "extra")
        phase("remove", "remove")
        source = root / SOURCE
        assert (source / "loadout.toml").read_text() == MANIFEST
        assert (source / "mcp.toml").read_text() == MCP
        phase("render", "removed")
        verify_removal(evidence / "baseline", evidence / "removed")
        print(
            f"{producer}: source removal restored every rendered destination and byte to baseline"
        )

    assert not root.exists(), root


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--repository", type=Path, default=Path(__file__).resolve().parents[1]
    )
    parser.add_argument("--loadout", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument(
        "--producer",
        action="append",
        choices=["python", "go", "rust", "swift"],
        help="Flyrail producer to verify. Repeat for multiple producers. Defaults to Python.",
    )
    parser.add_argument("--phase", choices=["install", "render", "remove"])
    parser.add_argument("--root", type=Path)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--flyrail-python", type=Path)
    parser.add_argument("--loadout-python", type=Path)
    parser.add_argument("--hook-tools", type=Path)
    args = parser.parse_args()
    repository = args.repository.resolve()
    loadout = args.loadout.resolve()
    evidence = args.evidence.resolve()
    node = shutil.which("node")
    bun = shutil.which("bun")
    assert node and bun
    if args.phase == "render":
        loadout_phase(args.root, args.snapshot)
        return
    if args.phase:
        flyrail_phase(args.phase, args.root, node)
        return

    producers = args.producer or ["python"]
    if len(producers) > 1:
        evidence.mkdir(parents=True, exist_ok=False)
    for producer in producers:
        print(f"Run pinned Loadout interoperability checks with {producer}", flush=True)
        output = evidence if len(producers) == 1 else evidence / producer
        run_producer(producer, args, repository, loadout, output, node, bun)


if __name__ == "__main__":
    main()
