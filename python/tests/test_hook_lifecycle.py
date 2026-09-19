import errno
import json
import os
import re
from dataclasses import replace
from pathlib import Path

import pytest
from test_configuration_lifecycle import receipt
from test_hooks import bundle_of, context, install, invoke, make_hook
from test_translation import assert_unsupported_without_writes, codes

import flyrail._lifecycle as lifecycle
import flyrail._resource_transaction as tx
from flyrail import (
    Acquisition,
    Bundle,
    BundleIdentity,
    Dependency,
    DependencyMode,
    Family,
    FileContent,
    HookOutcome,
    HookRuntime,
    HookShell,
    InstallationTarget,
    OperationStatus,
    Platform,
    RenderedArtifact,
    RenderedBundle,
    Surface,
    TreeContent,
    inspect_installation,
    remove,
    render,
    sync,
)

pytestmark = pytest.mark.integration


@pytest.mark.parametrize(
    "host,event,outcome",
    [
        ("codex", "before-tool", "ask"),
        ("cursor", "before-tool", "ask"),
        ("pi", "before-tool", "ask"),
        ("opencode", "before-tool", "ask"),
        ("cursor", "session-start", "block"),
        ("cursor", "prompt", "context"),
        ("copilot", "prompt", "block"),
        ("copilot", "prompt", "context"),
        ("opencode", "stop", "continue"),
        ("opencode", "prompt", "continue"),
        ("pi", "stop", "context"),
        ("claude", "stop", "block"),
        ("codex", "after-tool", "block"),
        ("pi", "session-start", "context"),
    ],
)
def test_unsupported_semantics_have_no_writes(
    tmp_path: Path, host: str, event: str, outcome: str
) -> None:
    bundle = bundle_of(make_hook(event=event, outcomes=(HookOutcome(outcome),)))
    rendered = render(bundle, context(tmp_path, host))
    assert codes(rendered) == {"hook-outcome"}
    assert_unsupported_without_writes(
        tmp_path, bundle, rendered, InstallationTarget(tmp_path / "index")
    )


@pytest.mark.parametrize(
    "kind",
    [
        "missing-runtime",
        "posix-shell",
        "cmd-shell",
        "cmd-literal",
        "shim",
        "interpolation",
        "node-modules",
        "surface",
        "environment",
        "budget",
        "count",
        "cursor-windows",
    ],
)
def test_runtime_prerequisites_are_preflight_constraints(tmp_path: Path, kind: str) -> None:
    ctx = context(tmp_path, "codex")
    base_runtime = ctx.hook_runtime
    assert base_runtime is not None
    hooks = [make_hook()]
    if kind == "missing-runtime":
        ctx = replace(ctx, hook_runtime=None)
    if kind == "posix-shell":
        ctx = replace(ctx, platform=Platform.LINUX, hook_runtime=replace(base_runtime, shell=None))
    if kind == "cmd-shell":
        ctx = replace(
            ctx,
            platform=Platform.WINDOWS,
            hook_runtime=replace(base_runtime, shell=HookShell.POSIX),
        )
    if kind == "cmd-literal":
        ctx = replace(
            ctx,
            platform=Platform.WINDOWS,
            asset_root=tmp_path / "percent%value",
            hook_runtime=replace(base_runtime, shell=HookShell.CMD),
        )
    if kind == "shim":
        ctx = replace(
            context(tmp_path, "claude"),
            platform=Platform.WINDOWS,
            hook_runtime=HookRuntime(tmp_path / "node.cmd"),
        )
    if kind == "interpolation":
        ctx = replace(context(tmp_path, "claude"), asset_root=tmp_path / "${CLAUDE_PROJECT_DIR}")
    if kind == "node-modules":
        ctx = replace(ctx, asset_root=tmp_path / "node_modules/assets")
    if kind == "surface":
        ctx = replace(context(tmp_path, "copilot"), surface=Surface.VSCODE)
    if kind == "environment":
        ctx = replace(ctx, platform=Platform.WINDOWS)
        hooks = [make_hook(env={"Mode": "a", "MODE": "b"})]
    if kind == "budget":
        hooks = [make_hook("first", timeout=300000), make_hook("second")]
    if kind == "count":
        hooks = [make_hook(f"handler-{number}", timeout=1) for number in range(65)]
    if kind == "cursor-windows":
        ctx = replace(context(tmp_path, "cursor"), platform=Platform.WINDOWS)
    bundle = bundle_of(*hooks)
    rendered = render(bundle, ctx)
    assert_unsupported_without_writes(
        tmp_path, bundle, rendered, InstallationTarget(tmp_path / "index")
    )


@pytest.mark.skipif(os.name == "nt", reason="Cursor native shell platform boundary")
@pytest.mark.parametrize(
    "original",
    [
        None,
        b"",
        b"  \n",
        b"{}",
        b'{"version":1}',
        b'{\n "version": 1, "hooks": {"stop": [{"command":"foreign"}]}\n}\n',
    ],
)
def test_cursor_schema_two_bundles_preserve_original_document(
    tmp_path: Path, original: bytes | None
) -> None:
    ctx = context(tmp_path, "cursor")
    path = tmp_path / ".cursor/hooks.json"
    if original is not None:
        path.parent.mkdir()
        path.write_bytes(original)
    bundles = [
        Bundle.from_artifacts(BundleIdentity(name, "1"), [make_hook()])
        for name in ["first", "second"]
    ]
    targets = [InstallationTarget(tmp_path / f"index-{name}") for name in ["first", "second"]]
    for bundle, target in zip(bundles, targets, strict=True):
        assert sync(bundle, render(bundle, ctx), target).status is OperationStatus.APPLIED
        assert all(item.schema_requirements for item in receipt(path).claims)
    assert len(receipt(path).claims) == 2
    assert remove("first", targets[0]).status is OperationStatus.APPLIED
    assert inspect_installation("second", targets[1]).is_current
    assert remove("second", targets[1]).status is OperationStatus.APPLIED
    assert (path.read_bytes() if path.exists() else None) == original


@pytest.mark.skipif(os.name == "nt", reason="Cursor native shell platform boundary")
def test_cursor_changed_schema_blocks_updates_but_allows_source_free_removal(
    tmp_path: Path,
) -> None:
    bundle, ctx, rendered, target = install(tmp_path, "cursor", (make_hook(),))
    path = next(item.destination for item in rendered.artifacts if item.id == "handler")
    path.write_text(re.sub(r'("version"\s*:\s*)1', r"\g<1>2", path.read_text()))
    assert not inspect_installation(bundle.id, target).is_current
    before = path.read_bytes()
    assert (
        sync(
            bundle, rendered, target, acquisition=Acquisition.TAKEOVER, replace_modified=True
        ).status
        is OperationStatus.FAILED
    )
    assert path.read_bytes() == before
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert json.loads(path.read_text()) == {"version": 2}
    assert render(bundle, ctx).supported


@pytest.mark.skipif(os.name == "nt", reason="Cursor native shell platform boundary")
def test_cursor_retained_scaffold_relinquishes_cleanup_rights(tmp_path: Path) -> None:
    bundle, _, rendered, target = install(tmp_path, "cursor", (make_hook(),))
    path = next(item.destination for item in rendered.artifacts if item.id == "handler")
    path.write_text(path.read_text().rstrip()[:-1] + ' ,"foreign":{}}')
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert json.loads(path.read_text()) == {"version": 1, "foreign": {}}
    assert receipt(path).provenance.schema_fields == ()
    value = json.loads(path.read_text())
    del value["foreign"]
    path.write_text(json.dumps(value))
    assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert json.loads(path.read_text()) == {"version": 1}


@pytest.mark.parametrize("agent", ["claude", "pi", "opencode"])
def test_failed_launcher_upgrade_retains_old_runtime_then_removes_source_free(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, agent: str
) -> None:
    first, ctx, old, target = install(tmp_path, agent, (make_hook(),))
    old_runtime = next(
        item.destination for item in old.artifacts if isinstance(item.content, TreeContent)
    )
    second = bundle_of(make_hook(output={"version": 1, "outcome": "block", "reason": "new"}))
    new = render(second, ctx)
    launcher = next(
        item.destination for item in new.artifacts if isinstance(item.content, FileContent)
    )
    original = tx.publish

    def fail(journal: tx.Journal) -> None:
        if journal.resource.destination == launcher:
            raise OSError(errno.EIO, "launcher failed")
        original(journal)

    monkeypatch.setattr(lifecycle, "publish", fail)
    result = sync(second, new, target)
    assert result.status is OperationStatus.PARTIAL
    assert old_runtime.is_dir()
    event = {"claude": "PreToolUse", "pi": "tool_call", "opencode": "tool.execute.before"}[agent]
    raw = (
        {"tool_name": "Bash", "tool_input": {}}
        if agent == "claude"
        else {"toolName": "bash", "input": {}}
        if agent == "pi"
        else {"tool": "bash", "sessionID": "s", "callID": "c"}
    )
    result_value, error = invoke(tmp_path, old, agent, event, raw, {"args": {}})
    assert error == ""
    assert (
        result_value == {}
        if agent == "claude"
        else result_value["result"] is None
        if agent == "pi"
        else result_value["blocked"] is None
    )
    monkeypatch.setattr(lifecycle, "publish", original)
    assert remove(first.id, target).status is OperationStatus.APPLIED
    assert not launcher.exists() and not old_runtime.exists()


def test_stable_dependency_does_not_weaken_revision_edges(tmp_path: Path) -> None:
    bundle = bundle_of(make_hook())
    target = InstallationTarget(tmp_path / "index")
    entry = RenderedArtifact("entry", Family.HOOKS, tmp_path / "entry", FileContent(b"one"))
    resource = RenderedArtifact("config", Family.HOOKS, tmp_path / "config", FileContent(b"entry"))
    stable = RenderedBundle(
        [entry, resource], [Dependency("config", "entry", DependencyMode.STABLE_REFERENCE)]
    )
    assert sync(bundle, stable, target).status is OperationStatus.APPLIED
    updated = RenderedBundle(
        [replace(entry, content=FileContent(b"two")), resource], stable.dependencies
    )
    assert sync(bundle, updated, target).status is OperationStatus.APPLIED
    assert (tmp_path / "entry").read_bytes() == b"two"
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    ordinary = RenderedBundle([entry, resource], [Dependency("config", "entry")])
    assert sync(bundle, ordinary, target).status is OperationStatus.APPLIED
    changed = RenderedBundle(updated.artifacts, ordinary.dependencies)
    assert sync(bundle, changed, target).status is OperationStatus.FAILED
    assert (tmp_path / "entry").read_bytes() == b"one"


def test_render_uses_detached_runtime_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import flyrail.hooks as preparation

    ctx = context(tmp_path, "claude")
    bundle = bundle_of(make_hook())
    expected = render(bundle, ctx)

    def forbidden(package: str) -> None:
        raise AssertionError("render must not read package resources")

    monkeypatch.setattr(preparation, "files", forbidden)
    assert render(bundle, ctx) == expected


def test_render_many_preserves_stable_edges(tmp_path: Path) -> None:
    from flyrail import DependencyMode, render_many

    ctx = context(tmp_path, "claude")
    result = render_many(bundle_of(make_hook()), [ctx, ctx])
    assert result.supported
    assert any(edge.mode is DependencyMode.STABLE_REFERENCE for edge in result.dependencies)


@pytest.mark.parametrize("original", [b'{"version":2}', b'{"version":"1"}', b'{"version":null}'])
def test_schema_mismatch_never_takes_over(tmp_path: Path, original: bytes) -> None:
    from flyrail import (
        DocumentFormat,
        DocumentSchema,
        Family,
        Key,
        RenderedArtifact,
        RenderedBundle,
        Scalar,
        Selector,
        StructuredContent,
    )

    path = tmp_path / "hooks.json"
    path.write_bytes(original)
    artifact = RenderedArtifact(
        "handler",
        Family.HOOKS,
        path,
        StructuredContent(DocumentFormat.JSONC, Selector([Key("hook")]), Scalar("value")),
        schema_requirements=(DocumentSchema(DocumentFormat.JSONC, "version", Scalar(1)),),
    )
    result = sync(
        bundle_of(make_hook()),
        RenderedBundle([artifact]),
        InstallationTarget(tmp_path / "index"),
        acquisition=Acquisition.TAKEOVER,
        replace_modified=True,
    )
    assert result.status is OperationStatus.FAILED
    assert path.read_bytes() == original


def test_missing_schema_is_repaired_on_update_not_status(tmp_path: Path) -> None:
    import re

    from flyrail import (
        DocumentFormat,
        DocumentSchema,
        Family,
        Key,
        RenderedArtifact,
        RenderedBundle,
        Scalar,
        Selector,
        StructuredContent,
    )

    path = tmp_path / "hooks.json"
    artifact = RenderedArtifact(
        "handler",
        Family.HOOKS,
        path,
        StructuredContent(DocumentFormat.JSONC, Selector([Key("hook")]), Scalar("value")),
        schema_requirements=(DocumentSchema(DocumentFormat.JSONC, "version", Scalar(1)),),
    )
    bundle = bundle_of(make_hook())
    rendered = RenderedBundle([artifact])
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
    path.write_text(re.sub(r',?\s*"version"\s*:\s*1', "", path.read_text()))
    before = path.read_bytes()
    assert not inspect_installation(bundle.id, target).is_current
    assert path.read_bytes() == before
    assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
    assert json.loads(path.read_text()) == {"hook": "value", "version": 1}
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert not path.exists()


def schema_artifact(path: Path, value: int = 1, identifier: str = "handler") -> RenderedArtifact:
    from flyrail import DocumentFormat, DocumentSchema, Key, Scalar, Selector, StructuredContent

    return RenderedArtifact(
        identifier,
        Family.HOOKS,
        path,
        StructuredContent(DocumentFormat.JSONC, Selector([Key(identifier)]), Scalar("value")),
        schema_requirements=(DocumentSchema(DocumentFormat.JSONC, "version", Scalar(value)),),
    )


@pytest.mark.parametrize("existing", [False, True])
@pytest.mark.parametrize("committed", [False, True])
def test_schema_publication_failure_recovers_original_baseline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, existing: bool, committed: bool
) -> None:
    path = tmp_path / "hooks.json"
    if existing:
        path.write_bytes(b" \n")
    target = InstallationTarget(tmp_path / "index")
    rendered = RenderedBundle([schema_artifact(path)])
    bundle = bundle_of(make_hook())
    original = tx.publish_receipt

    def crash(journal: tx.Journal) -> None:
        if committed:
            original(journal)
        raise KeyboardInterrupt

    monkeypatch.setattr(tx, "publish_receipt", crash)
    with pytest.raises(KeyboardInterrupt):
        sync(bundle, rendered, target)
    assert inspect_installation(bundle.id, target).pending
    monkeypatch.setattr(tx, "publish_receipt", original)
    result = remove(bundle.id, target)
    assert result.status in {OperationStatus.APPLIED, OperationStatus.UNCHANGED}
    assert (path.read_bytes() if path.exists() else None) == (b" \n" if existing else None)


@pytest.mark.parametrize("kind", ["incompatible", "overlap", "alias"])
def test_schema_requirements_conflict_before_publication(tmp_path: Path, kind: str) -> None:
    from flyrail import DocumentFormat, Key, Scalar, Selector, StructuredContent

    path = tmp_path / "hooks.json"
    first = schema_artifact(path)
    second = (
        schema_artifact(path, value=2, identifier="second")
        if kind == "incompatible"
        else RenderedArtifact(
            "second",
            Family.HOOKS,
            path,
            StructuredContent(DocumentFormat.JSONC, Selector([Key("version")]), Scalar(1)),
        )
        if kind == "overlap"
        else replace(first, id="second", schema_requirements=())
    )
    result = sync(
        bundle_of(make_hook()),
        RenderedBundle([first, second]),
        InstallationTarget(tmp_path / "index"),
    )
    assert result.status is OperationStatus.FAILED
    assert not path.exists()


@pytest.mark.skipif(os.name == "nt", reason="Cursor native shell platform boundary")
def test_cursor_upgrade_retains_first_takeover_baseline(tmp_path: Path) -> None:
    from flyrail import DocumentRequest, StructuredContent, edit_document

    ctx = context(tmp_path, "cursor")
    first = bundle_of(make_hook())
    rendered = render(first, ctx)
    artifact = next(item for item in rendered.artifacts if item.id == "handler")
    assert isinstance(artifact.content, StructuredContent)
    created = edit_document(None, [DocumentRequest.set(artifact.content)])
    assert created.after is not None
    entry = json.loads(created.after)["hooks"]["preToolUse"][0]
    entry["timeout"] = 25
    path = artifact.destination
    path.parent.mkdir()
    original = json.dumps(
        {"version": 1, "hooks": {"preToolUse": [entry]}, "foreign": True}
    ).encode()
    path.write_bytes(original)
    target = InstallationTarget(tmp_path / "index")
    assert (
        sync(first, rendered, target, acquisition=Acquisition.TAKEOVER).status
        is OperationStatus.APPLIED
    )
    second = bundle_of(make_hook(output={"version": 1, "outcome": "block", "reason": "update"}))
    assert sync(second, render(second, ctx), target).status is OperationStatus.APPLIED
    assert remove(first.id, target).status is OperationStatus.APPLIED
    assert path.read_bytes() == original


@pytest.mark.parametrize("stable_alias", ["first", "second"])
def test_aliases_cannot_weaken_revision_dependencies(tmp_path: Path, stable_alias: str) -> None:
    first = schema_artifact(tmp_path / "config.json", identifier="first")
    second = replace(first, id="second")
    entry = RenderedArtifact("entry", Family.HOOKS, tmp_path / "entry", FileContent(b"launcher"))
    rendered = RenderedBundle(
        [first, second, entry],
        [
            Dependency(
                alias,
                "entry",
                DependencyMode.STABLE_REFERENCE
                if alias == stable_alias
                else DependencyMode.REVISION,
            )
            for alias in ["first", "second"]
        ],
    )
    result = sync(bundle_of(make_hook()), rendered, InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.FAILED
    assert not first.destination.exists() and not entry.destination.exists()
