import json
import subprocess
from pathlib import Path

import flyrail
from flyrail import EnvRef, HookArtifact
from flyrail.values import ArrayValue, ObjectValue, Scalar, value_from_record


MODELS = {
    model.__name__: model for model in [
        ArrayValue, flyrail.BundleEntry, flyrail.Dependency, flyrail.DocumentSchema, flyrail.FileContent,
        flyrail.Key, flyrail.Member, ObjectValue, flyrail.RenderedArtifact, Scalar, flyrail.Selector,
        flyrail.StructuredContent, flyrail.TreeContent,
    ]
}
FIELD_TYPES = {
    "RenderedArtifact": {"family": flyrail.Family, "destination": Path},
    "FileContent": {"mode": flyrail.FileMode},
    "StructuredContent": {"format": flyrail.DocumentFormat},
    "DocumentSchema": {"format": flyrail.DocumentFormat},
    "Dependency": {"mode": flyrail.DependencyMode},
}
CONTEXT_PATHS = ["execution_root", "instruction_path", "mcp_path", "native_root", "asset_root"]


def thaw(value):
    if isinstance(value, Scalar):
        return value.value
    if isinstance(value, ArrayValue):
        return tuple(thaw(item) for item in value.items)
    assert isinstance(value, ObjectValue)
    return {key: thaw(item) for key, item in value.items}


def model(value):
    if isinstance(value, tuple):
        return tuple(model(item) for item in value)
    if not isinstance(value, dict):
        return value
    if value.keys() == {"bytes"}:
        return bytes.fromhex(value["bytes"])
    name = value["type"]
    assert name in MODELS, f"unexpected rendered model type: {name}"
    fields = {key: model(item) for key, item in value["fields"].items()}
    for key, constructor in FIELD_TYPES.get(name, {}).items():
        fields[key] = constructor(fields[key])
    return MODELS[name](**fields)


def optional_path(value):
    return None if value is None else str(value)


class SwiftHookRenderer:
    def __init__(self, executable, fallback):
        self.executable = executable
        self.fallback = fallback
        self.calls = []

    def __call__(self, bundle, context):
        if not bundle.artifacts or not all(isinstance(hook, HookArtifact) for hook in bundle.artifacts):
            return self.fallback(bundle, context)
        assert not bundle.assets and not bundle.dependencies
        hooks = []
        for hook in bundle.artifacts:
            command = {
                "argv": list(hook.command.argv),
                "env": {key: {"env": value.name} if isinstance(value, EnvRef) else value
                        for key, value in hook.command.env},
            }
            if hook.command.cwd is not None:
                command["cwd"] = hook.command.cwd
            hooks.append({
                "kind": "hook", "id": hook.id, "protocol_version": hook.protocol_version,
                "event": str(hook.event), "command": command, "timeout_ms": hook.timeout_ms,
                "outcomes": list(hook.outcomes), "tools": list(hook.tools),
            })
        runtime = context.hook_runtime
        request = {
            "manifest": {"schema_version": 2, "id": bundle.id, "version": bundle.version, "artifacts": hooks},
            "root": str(context.root), "agent": str(context.target.agent),
            "scope": str(context.target.scope), "platform": str(context.platform),
            "surface": str(context.surface), "runtime": runtime is not None,
            "node": optional_path(runtime.node) if runtime is not None else None,
            "shell": optional_path(runtime.shell) if runtime is not None else None,
            **{name: optional_path(getattr(context, name)) for name in CONTEXT_PATHS},
        }
        result = subprocess.run(
            [str(self.executable), "render-hooks"], input=json.dumps(request),
            capture_output=True, text=True, check=True, timeout=15,
        )
        assert result.stderr == "", result.stderr
        artifacts, dependencies, notices, routes, digest = thaw(value_from_record(json.loads(result.stdout)))
        rendered = flyrail.RenderedBundle(
            model(artifacts), model(dependencies),
            [flyrail.Notice(item[0], flyrail.NoticeKind(item[1]), item[2], item[3]) for item in notices],
            routing_context=routes,
        )
        assert rendered.content_digest == digest
        self.calls.append((str(context.target.agent), str(context.target.scope), str(context.root)))
        return rendered
