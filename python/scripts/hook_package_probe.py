import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import flyrail
from flyrail import (
    AssetRef,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    Dependency,
    Family,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HookRuntime,
    InstallationTarget,
    OperationStatus,
    Platform,
    RenderContext,
    SupportAsset,
    Target,
    TreeContent,
    remove,
    render,
    sync,
)


def main() -> None:
    assert Path(flyrail.__file__).is_relative_to(Path(sys.prefix))
    executable = shutil.which("node")
    assert executable is not None
    root = Path.cwd() / "installed hook ü"
    root.mkdir()
    source = (
        b'let s="";process.stdin.on("data",c=>s+=c);process.stdin.on("end",()=>'
        b'process.stdout.write(JSON.stringify({version:1,outcome:"context",'
        b"text:JSON.parse(s).tool})))"
    )
    bundle = Bundle.from_artifacts(
        BundleIdentity("installed-runtime", "one"),
        [
            HookArtifact(
                "observer",
                HookEvent.BEFORE_TOOL,
                Command([executable, AssetRef("script", "hook.cjs")], cwd=AssetRef("script")),
                outcomes=[HookOutcome.CONTEXT],
            )
        ],
        assets=[
            SupportAsset("script", Family.HOOKS, TreeContent([BundleEntry("hook.cjs", source)]))
        ],
        dependencies=[Dependency("observer", "script")],
    )
    runtime = HookRuntime(Path(executable))
    ctx = RenderContext(
        Target.project("claude", root),
        Platform.WINDOWS if os.name == "nt" else Platform.LINUX,
        hook_runtime=runtime,
    )
    rendered = render(bundle, ctx)
    assert rendered.supported
    target = InstallationTarget(root / "index")
    assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
    entry = json.loads((root / ".claude/settings.json").read_text())["hooks"]["PreToolUse"][0][
        "hooks"
    ][0]
    environment = os.environ.copy()
    environment.pop("PYTHONPATH", None)
    result = subprocess.run(  # noqa: S603
        [entry["command"], *entry["args"]],
        input='{"tool_name":"Bash","tool_input":{}}',
        text=True,
        capture_output=True,
        cwd=root,
        env=environment,
        check=True,
        timeout=10,
    )
    assert result.stderr == ""
    assert json.loads(result.stdout) == {
        "hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": "Bash"}
    }
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    print("Installed wheel runtime, package assets, argv bootstrap and source-free removal passed.")


if __name__ == "__main__":
    main()
