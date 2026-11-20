import hashlib
import json
import tomllib
from pathlib import Path
from typing import Any

import pytest

from flyrail import (
    Agent,
    AssetRef,
    Audience,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    Dependency,
    DirectoryStore,
    EnvRef,
    ErrorCode,
    Family,
    FileContent,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HookRuntime,
    HookShell,
    HttpTransport,
    InstructionArtifact,
    LoadoutDestination,
    LoadoutLocation,
    LoadoutPreview,
    LoadoutStatus,
    McpArtifact,
    NativeArtifact,
    OperationStatus,
    Platform,
    SkillArtifact,
    SupportAsset,
    Surface,
    TargetScope,
    TreeContent,
    _lifecycle,
    _resource_transaction,
    apply_loadout,
    discover_loadout,
    inspect_loadout,
    preview_loadout,
    preview_loadout_removal,
    recover_loadout,
)

FIXTURES = Path(__file__).resolve().parents[1] / "spec/fixtures"
if not FIXTURES.is_dir():
    FIXTURES = Path(__file__).resolve().parents[2] / "spec/fixtures"
VECTORS = json.loads((FIXTURES / "loadout.json").read_text())


@pytest.mark.parametrize("case", VECTORS["discovery"], ids=lambda item: item["name"])
def test_shared_discovery(case: dict[str, Any], tmp_path: Path) -> None:
    root = tmp_path.resolve()
    for name, text in case["files"].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text.replace("{root}", str(root)))
    env = {key: item.replace("{root}", str(root)) for key, item in case.get("env", {}).items()}
    result = discover_loadout(root / "home", env=env)
    assert result.status == case["status"]
    if result.status is LoadoutStatus.INVALID:
        assert result.diagnostic is not None
        assert result.diagnostic.code == case["code"]
    if result.status is LoadoutStatus.AVAILABLE:
        setup = result.setup
        assert setup is not None
        assert setup.profile == case["profile"]
        assert [p.relative_to(root).as_posix() for p in setup.profile_chain] == case["chain"]
        assert [s.name for s in setup.sources] == case["sources"]
        if "source_paths" in case:
            assert [s.path.relative_to(root).as_posix() for s in setup.sources] == case[
                "source_paths"
            ]
        assert [h.agent for h in setup.harnesses] == case["harnesses"]
        if "instruction_path" in case:
            [selector] = setup.selectors
            assert selector.manifest.relative_to(root).as_posix() == case["instruction_path"]
            assert list(selector.key) == case["instruction_key"]


def setup_files(
    tmp_path: Path, manifest: str, *, profile: str = "default"
) -> tuple[Path, Path, Path]:
    root = tmp_path.resolve()
    home = root / "home"
    config = home / ".config/loadout/config.toml"
    config.parent.mkdir(parents=True, exist_ok=True)
    source = root / "loadout"
    source.mkdir(exist_ok=True)
    config.write_text(f"source = '{source}'\nprofile = '{profile}'\n")
    (source / "loadout.toml").write_text(manifest)
    return home, source, config


MANIFEST = """# user manifest
[[source]]
name = "local"
path = "."

[claude]
instructions = ["welcome"] # keep

[codex]
instructions = []
"""


def desired(
    *, identifier: str = "review-tools", version: str = "1", text: str = "Review changes."
) -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity(identifier, version),
        [
            InstructionArtifact("guide", text),
            SkillArtifact(
                "skill",
                "review",
                TreeContent(
                    [
                        BundleEntry(
                            "SKILL.md", b"---\nname: review\ndescription: Review changes\n---\n"
                        ),
                        BundleEntry("assets/input.bin", b"\0\xff"),
                    ]
                ),
            ),
            McpArtifact(
                "server",
                "review-http",
                HttpTransport("https://example.test/mcp", EnvRef("REVIEW_TOKEN")),
            ),
            McpArtifact("local", "review-local", Command(["review", "--stdio"], {"STDIO": "on"})),
        ],
    )


def choose(home: Path, *agents: Agent, shared: bool = False) -> LoadoutDestination:
    result = discover_loadout(home)
    assert result.setup is not None, result.diagnostic
    return result.setup.select("local", agents, accept_shared_scope=shared)


@pytest.mark.integration
def test_loadout_install_repeat_upgrade_and_remove_after_registration_changes(
    tmp_path: Path,
) -> None:
    home, source, registration = setup_files(tmp_path, MANIFEST)
    (source / "instructions").mkdir()
    welcome = source / "instructions/welcome.md"
    welcome.write_text("Welcome.")
    store = DirectoryStore(tmp_path / "state")
    bundle = desired()
    agents = (Agent.CLAUDE, Agent.CODEX)

    first = preview_loadout(bundle, choose(home, *agents), store, platform=Platform.MACOS)
    assert first.applicable, first.lifecycle
    assert (source / "loadout.toml").read_text() == MANIFEST
    assert apply_loadout(first).sync_required
    assert inspect_loadout(bundle.id, first.location, store).is_current
    parsed_manifest = tomllib.loads((source / "loadout.toml").read_text())
    [welcome_ref, selection] = parsed_manifest["claude"]["instructions"]
    assert welcome_ref == "welcome"
    assert selection == parsed_manifest["codex"]["instructions"][0]
    name = selection.removeprefix("local/")
    assert (source / "instructions" / f"{name}.md").read_text() == "Review changes."
    assert parsed_manifest["skills"]["review"] == {"agents": ["claude", "codex"], "render": False}
    assert (source / "skills/review/assets/input.bin").read_bytes() == b"\0\xff"
    assert tomllib.loads((source / "mcp.toml").read_text()) == {
        "review-http": {
            "transport": "http",
            "url": "https://example.test/mcp",
            "auth_env_var": "REVIEW_TOKEN",
        },
        "review-local": {
            "transport": "stdio",
            "command": "review",
            "args": ["--stdio"],
            "env": {"STDIO": "on"},
        },
    }

    second = preview_loadout(bundle, choose(home, *agents), store, platform=Platform.MACOS)
    assert apply_loadout(second).lifecycle.status is OperationStatus.UNCHANGED
    upgrade = preview_loadout(
        desired(version="2", text="Read the diff."),
        choose(home, *agents),
        store,
        platform=Platform.MACOS,
    )
    assert apply_loadout(upgrade).lifecycle.status is OperationStatus.APPLIED
    assert (source / "instructions" / f"{name}.md").read_text() == "Read the diff."

    registration.write_text("source = '~/nowhere'\n")
    location = LoadoutLocation(source, "default", "local", source)
    removal = preview_loadout_removal(bundle.id, location, store)
    assert removal.applicable
    assert apply_loadout(removal).sync_required
    assert (source / "loadout.toml").read_text() == MANIFEST
    assert welcome.read_text() == "Welcome."
    assert sorted(p.name for p in source.iterdir()) == ["instructions", "loadout.toml"]


@pytest.mark.integration
def test_loadout_mcp_scope_and_stale_inputs(tmp_path: Path) -> None:
    home, source, registration = setup_files(tmp_path, MANIFEST)
    store = DirectoryStore(tmp_path / "state")
    bundle = Bundle.from_artifacts(
        BundleIdentity("team", "1"),
        [
            McpArtifact("mcp", "team", Command(["team"])),
        ],
    )

    denied = preview_loadout(bundle, choose(home, Agent.CLAUDE), store, platform=Platform.LINUX)
    assert denied.diagnostic and denied.diagnostic.code == "shared-scope"
    assert denied.impact[0].harnesses == (Agent.CLAUDE, Agent.CODEX)
    assert apply_loadout(denied).lifecycle.status is OperationStatus.FAILED

    allowed = preview_loadout(
        bundle, choose(home, Agent.CLAUDE, shared=True), store, platform=Platform.LINUX
    )
    assert allowed.applicable
    registration.write_text("source = '~/new-source'\n")
    result = apply_loadout(allowed).lifecycle
    assert result.error and result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert sorted(path.name for path in source.iterdir()) == ["loadout.toml"]


@pytest.mark.integration
def test_loadout_inherited_selector_and_shared_manifest_ownership(tmp_path: Path) -> None:
    home, source, _ = setup_files(tmp_path, MANIFEST, profile="work")
    child = source / "work.toml"
    child.write_text("extends = 'default'\n")
    store = DirectoryStore(tmp_path / "state")
    first = Bundle.from_artifacts(
        BundleIdentity("one", "1"), [InstructionArtifact("guide", "One.")]
    )
    second = Bundle.from_artifacts(
        BundleIdentity("two", "1"), [InstructionArtifact("guide", "Two.")]
    )
    first_plan = preview_loadout(first, choose(home, Agent.CLAUDE), store, platform=Platform.MACOS)
    assert first_plan.diagnostic and first_plan.diagnostic.code == "shared-scope"
    assert first_plan.impact[0].path == source / "loadout.toml"

    first_plan = preview_loadout(
        first, choose(home, Agent.CLAUDE, shared=True), store, platform=Platform.MACOS
    )
    assert apply_loadout(first_plan).lifecycle.status is OperationStatus.APPLIED
    second_plan = preview_loadout(
        second, choose(home, Agent.CLAUDE, shared=True), store, platform=Platform.MACOS
    )
    assert apply_loadout(second_plan).lifecycle.status is OperationStatus.APPLIED
    assert child.read_text() == "extends = 'default'\n"
    retirement = preview_loadout_removal(first.id, first_plan.location, store)
    assert apply_loadout(retirement).lifecycle.status is OperationStatus.APPLIED
    assert inspect_loadout(second.id, second_plan.location, store).is_current
    assert len(tomllib.loads((source / "loadout.toml").read_text())["claude"]["instructions"]) == 2


@pytest.mark.parametrize(
    ("manifest", "transport", "code"),
    [
        ("[claude]\nmcp = false\n", Command(["review"]), "disabled-slice"),
        ("use = ['instructions']\n[claude]\n", Command(["review"]), "source-use"),
        ("[claude]\n", Command(["review"], {"TOKEN": EnvRef("REVIEW_TOKEN")}), "mcp-env-reference"),
        ("[claude]\n", Command(["review"], cwd="subdir"), "mcp-cwd"),
    ],
)
def test_loadout_preserves_disabled_slices_and_unsupported_command_semantics(
    tmp_path: Path, manifest: str, transport: Command, code: str
) -> None:
    manifest = "[[source]]\nname = 'local'\npath = '.'\n" + manifest
    home, source, _ = setup_files(tmp_path, manifest)
    bundle = Bundle.from_artifacts(
        BundleIdentity("command", "1"), [McpArtifact("review", "review", transport)]
    )
    plan = preview_loadout(
        bundle,
        choose(home, Agent.CLAUDE),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
    )
    assert plan.diagnostic and plan.diagnostic.code == code
    assert sorted(path.name for path in source.iterdir()) == ["loadout.toml"]
    assert (source / "loadout.toml").read_text() == manifest


@pytest.mark.integration
def test_loadout_checks_later_sources_before_preview_and_application(tmp_path: Path) -> None:
    manifest = MANIFEST.replace("[claude]", "[[source]]\nname='team'\npath='../team'\n[claude]")
    home, source, _ = setup_files(tmp_path, manifest)
    team = source.parent / "team"
    team.mkdir()
    store = DirectoryStore(tmp_path / "state")
    bundle = Bundle.from_artifacts(
        BundleIdentity("command", "1"), [McpArtifact("review", "review", Command(["review"]))]
    )
    setup = discover_loadout(home).setup
    assert setup is not None
    agents = (Agent.CLAUDE, Agent.CODEX)
    assert [item.name for item in setup.eligible_sources(bundle, agents)] == ["local", "team"]
    selected = setup.select("local", agents)
    fresh = preview_loadout(bundle, selected, store, platform=Platform.LINUX)
    assert fresh.applicable
    colliding = "[review]\ntransport = 'stdio'\ncommand = 'team-review'\n"
    (team / "mcp.toml").write_text(colliding)
    stale = apply_loadout(fresh).lifecycle
    assert stale.error and stale.error.code is ErrorCode.CONCURRENT_CHANGE
    conflict = preview_loadout(bundle, selected, store, platform=Platform.LINUX)
    assert conflict.diagnostic and conflict.diagnostic.code == "source-collision"
    assert (team / "mcp.toml").read_text() == colliding
    assert sorted(path.name for path in source.iterdir()) == ["loadout.toml"]


@pytest.mark.integration
def test_loadout_recovers_interrupted_fragment_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home, source, _ = setup_files(tmp_path, MANIFEST)
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"), [InstructionArtifact("guide", "Read the diff.")]
    )
    store = DirectoryStore(tmp_path / "state")

    def plan() -> LoadoutPreview:
        return preview_loadout(bundle, choose(home, Agent.CODEX), store, platform=Platform.LINUX)

    class Interruption(Exception):
        pass

    def interrupt(store: DirectoryStore, journal: _resource_transaction.Journal) -> None:
        if journal.resource.destination == source / "loadout.toml":
            raise Interruption
        _resource_transaction.publish(store, journal)

    proposal = plan()
    with monkeypatch.context() as patch:
        patch.setattr(_lifecycle, "publish", interrupt)
        with pytest.raises(Interruption):
            apply_loadout(proposal)
    assert (source / "loadout.toml").read_text() == MANIFEST
    [fragment] = list((source / "instructions").glob("*.md"))
    assert fragment.read_text() == "Read the diff."
    recovered = recover_loadout(bundle.id, proposal.location, store)
    assert recovered.lifecycle.status is OperationStatus.APPLIED
    assert apply_loadout(plan()).lifecycle.status is OperationStatus.APPLIED
    assert inspect_loadout(bundle.id, proposal.location, store).is_current


@pytest.mark.integration
def test_loadout_preserves_an_edited_owned_fragment(tmp_path: Path) -> None:
    home, source, _ = setup_files(tmp_path, MANIFEST)
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"), [InstructionArtifact("guide", "Read the diff.")]
    )
    store = DirectoryStore(tmp_path / "state")
    first = preview_loadout(bundle, choose(home, Agent.CODEX), store, platform=Platform.LINUX)
    assert apply_loadout(first).sync_required
    [fragment] = list((source / "instructions").glob("*.md"))
    fragment.write_text("Keep my edit.")
    manifest = (source / "loadout.toml").read_bytes()
    removal = apply_loadout(preview_loadout_removal(bundle.id, first.location, store)).lifecycle
    assert removal.status is OperationStatus.FAILED
    assert fragment.read_text() == "Keep my edit."
    assert (source / "loadout.toml").read_bytes() == manifest


SOURCE = "[[source]]\nname = 'local'\npath = '.'\n"


@pytest.mark.parametrize(
    ("tail", "code"),
    [
        ("[unexpected]\n", "unsupported-manifest"),
        ("\n[claude]\nstray = true", "invalid-manifest"),
        ("\n[codex]\nsettings = ['cfg']", "invalid-manifest"),
        ("\n[claude]\nskills = []", "invalid-manifest"),
        ("\n[claude]\ninstructions = 'guide'", "invalid-manifest"),
        ("\n[claude]\npreserve = false", "invalid-manifest"),
        ("\n[claude]\nsubstitute = []", "invalid-manifest"),
        ("\n[claude.substitute]\nfoo = 3", "invalid-manifest"),
        ("\n[claude]\nsettings = 8", "invalid-manifest"),
        ("\n[claude]\npermissions = false\nsettings = 'cfg'", "invalid-manifest"),
        ("\n[pi]\nplugins = 3", "invalid-manifest"),
        ("\n[skills.review]\nagents = ['claude']", "invalid-manifest"),
        ("\n[claude]\n[skills.review]\nagents = ['claude', 'claude']", "invalid-manifest"),
        ("\n[claude]\n[skills.review]\nagents = ['claude']\nrender = 'yes'", "invalid-manifest"),
        ("\n[claude]\n[skills.review]\nagents = ['claude']\nunknown = true", "invalid-manifest"),
        ("\n[claude]\n[skills.'sub/review']\nagents = ['claude']", "invalid-manifest"),
        ("\n[claude]\n[skills.review]\nagents = ['pi']", "invalid-manifest"),
        ("\n[claude]\n[skills.review]\nagents = [3]", "invalid-manifest"),
        ("\n[claude]\n[skills]\nreview = true", "invalid-manifest"),
        ("\n[instructions.claude]\norder = []", "invalid-manifest"),
        ("\n[instructions.claude]\norder = []\noutput = '../escape'", "invalid-manifest"),
        ("\n[instructions.claude]\norder = []\noutput = '/etc/escape'", "invalid-manifest"),
        ("\n[instructions.claude]\norder = []\noutput = '.'", "invalid-manifest"),
        ("\n[instructions.claude]\norder = []\ndestinations = ['']", "invalid-manifest"),
        ("\n[instructions.claude]\norder = []\noutput = 'one'\nprofile = true", "invalid-manifest"),
        ("\n[instructions.copilot]\norder = []\noutput = 'one'", "unsupported-manifest"),
        ("\n[permissions.tools]\nrender = 'unknown'\noutput = 'one'", "unsupported-manifest"),
        (
            "\n[permissions.tools]\nrender = 'claude'\nrules = []\noutput = 'one'",
            "unsupported-manifest",
        ),
        (
            "\n[instructions.claude]\norder=[]\noutput='one'\n[instructions.codex]\norder=[]\noutput='one'",
            "invalid-manifest",
        ),
        ("\n[all]\ninstructions = []", "invalid-manifest"),
        ("use = []\n[claude]", "invalid-source"),
        ("use = ['junk']\n[claude]", "invalid-source"),
        ("use = [1]\n[claude]", "invalid-manifest"),
        ("unknown = true\n[claude]", "unsupported-manifest"),
        ("\n[[source]]\nname='LOCAL'\npath='.'\n[claude]", "invalid-source"),
        ("\n[[source]]\nname='alias'\npath='.'\n[claude]", "invalid-source"),
        ("\n[[source]]\nname=3\npath='.'\n[claude]", "invalid-source"),
        ("\n[[source]]\nname='a/b'\npath='.'\n[claude]", "invalid-source"),
        ("\n[[source]]\nname='missing'\npath='.'\noverrides = []\n[claude]", "invalid-source"),
        ("[source.overrides]\nmcp = ['review']\n[claude]", "invalid-source"),
        ("[source.overrides]\nskills = ['review', 'review']\n[claude]", "invalid-source"),
        ("[source.overrides]\nskills = ['a/b']\n[claude]", "invalid-source"),
        ("[source.overrides]\nmodule-config = ['none/plugins/x.ts']\n[claude]", "invalid-source"),
    ],
)
def test_malformed_effective_declarations(tmp_path: Path, tail: str, code: str) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + tail)
    discovery = discover_loadout(home)
    assert discovery.status is LoadoutStatus.INVALID
    assert discovery.diagnostic is not None
    assert discovery.diagnostic.code == code
    assert discovery.diagnostic.path == source / "loadout.toml"


@pytest.mark.parametrize(
    ("extra", "diagnostic"),
    [
        ("extends = 9", "invalid-profile"),
        ("extends = 'absent'", "invalid-profile"),
        ("extends = 'default'\nremove = 3", "invalid-profile"),
        ("extends = 'default'\nremove = ['claude', 'claude.instructions']", "invalid-profile"),
        ("extends = 'default'\nremove = ['missing']", "invalid-profile"),
        ("extends = 'default'\nremove = ['claude.instructions.nope']", "invalid-profile"),
        ("extends = 'default'\nremove = ['claude.&']", "invalid-profile"),
        ("extends = 'default'\nremove = ['\"\\q\"']", "invalid-profile"),
    ],
)
def test_damaged_profile_overlays(tmp_path: Path, extra: str, diagnostic: str) -> None:
    home, source, _ = setup_files(tmp_path, MANIFEST, profile="work")
    (source / "work.toml").write_text(extra)
    discovery = discover_loadout(home)
    assert discovery.status is LoadoutStatus.INVALID
    assert discovery.diagnostic is not None
    assert discovery.diagnostic.code == diagnostic


@pytest.mark.parametrize(
    "key",
    [
        "source = 7",
        "source = '~/unset'\nunknown = ''",
        "source = '~/unset'\nharnesses = []",
        "source = '~/unset'\nharnesses = ['nope']",
        "source = '~nobody/not-this-home'",
        "source = '~/unset'\nprofile = 5",
        "source = '~/unset'\nharnesses = [5]",
    ],
)
def test_invalid_registration_values(tmp_path: Path, key: str) -> None:
    home, _, registry = setup_files(tmp_path, MANIFEST)
    registry.write_text(key)
    discovered = discover_loadout(home)
    assert discovered.status is LoadoutStatus.INVALID
    assert discovered.diagnostic is not None
    assert discovered.diagnostic.code == "invalid-registry"
    assert discovered.diagnostic.path == registry


@pytest.mark.parametrize("kind", ["directory", "dangling", "oversize", "alias-loop", "escape"])
def test_invalid_observed_manifest_input(tmp_path: Path, kind: str) -> None:
    home, source, _ = setup_files(tmp_path, MANIFEST, profile="work")
    profile = source / "work.toml"
    expected = "invalid-profile"
    if kind == "directory":
        profile.mkdir()
    elif kind == "dangling":
        profile.symlink_to(source / "not-there.toml")
    elif kind == "oversize":
        with profile.open("wb") as file:
            file.truncate(8 * 1024 * 1024 + 1)
    elif kind == "alias-loop":
        profile.write_text("extends = 'broken'")
        (source / "broken.toml").symlink_to("broken.toml")
    else:
        outside = tmp_path / "outside.toml"
        outside.write_text("extends = 'default'")
        profile.symlink_to(outside)
        expected = "escaped-path"
    discovered = discover_loadout(home)
    assert discovered.status is LoadoutStatus.INVALID
    assert discovered.diagnostic is not None
    assert discovered.diagnostic.code == expected


def test_explicit_config_choice_and_source_selection(tmp_path: Path) -> None:
    home, source, config = setup_files(tmp_path, MANIFEST)
    selected = tmp_path / "explicit" / "loadout" / "config.toml"
    selected.parent.mkdir(parents=True)
    selected.write_bytes(config.read_bytes())
    config.write_text("source = [")
    found = discover_loadout(home, config_home=selected.parent.parent, env={"XDG_CONFIG_HOME": "/"})
    assert found.registry == selected
    assert found.setup is not None and found.setup.root == source
    assert found.setup.eligible_sources(desired(), [Agent.DROID]) == ()
    for agents in ([], [Agent.CLAUDE, Agent.CLAUDE]):
        with pytest.raises(ValueError, match="distinct harnesses"):
            found.setup.select("local", agents)
    with pytest.raises(ValueError, match="declared Loadout source"):
        found.setup.select("foreign", [Agent.CLAUDE])
    with pytest.raises(ValueError, match="absolute path"):
        discover_loadout(Path("home"))
    with pytest.raises(ValueError, match="sibling"):
        LoadoutLocation(source, "../scope", "local", source)
    with pytest.raises(ValueError, match="source_name"):
        LoadoutLocation(source, "default", "name/path", source)


def mcp_command(name: str, transport: Command | HttpTransport | None = None) -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity("toolbox", "1"),
        [McpArtifact("server", name, transport or Command(["review"]))],
    )


@pytest.mark.parametrize("case", VECTORS["mcp_previews"], ids=lambda item: item["name"])
def test_shared_mcp_preview_cases(case: dict[str, Any], tmp_path: Path) -> None:
    root = tmp_path.resolve()
    for name, text in case["files"].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text.replace("{root}", str(root)))
    command = Command(
        ["review"], {"TOKEN": EnvRef("REVIEW_TOKEN")} if case.get("env_reference") else {}
    )
    choice = choose(root / "home", *(Agent(agent) for agent in case["harnesses"]))
    plan = preview_loadout(
        mcp_command("review", command),
        choice,
        DirectoryStore(root / "state"),
        platform=Platform.LINUX,
    )
    assert plan.applicable == (case["code"] is None)
    assert (plan.diagnostic.code if plan.diagnostic else None) == case["code"]
    assert (root / "setup/loadout.toml").read_text() == case["files"]["setup/loadout.toml"]


@pytest.mark.parametrize(
    "agent,rule", [(Agent.OPENCODE, "review_team/*"), (Agent.PI, "review__team/*")]
)
@pytest.mark.integration
def test_mcp_rules_can_shadow_a_new_server(tmp_path: Path, agent: Agent, rule: str) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + f"[{agent}]\n")
    store = DirectoryStore(tmp_path / "state")
    bundle = mcp_command("review")
    selected = choose(home, agent)
    plan = preview_loadout(bundle, selected, store, platform=Platform.LINUX)
    assert plan.applicable
    rules = source / "permissions.toml"
    rules.write_text(f"[mcp]\nallow = ['*/allowed', '{rule}']\n")
    stale = apply_loadout(plan).lifecycle
    assert stale.error and stale.error.code is ErrorCode.CONCURRENT_CHANGE
    denied = preview_loadout(bundle, selected, store, platform=Platform.LINUX)
    assert denied.diagnostic and denied.diagnostic.code == "mcp-permission-overlap"
    assert denied.diagnostic.path == rules
    assert sorted(path.name for path in source.iterdir()) == ["loadout.toml", "permissions.toml"]
    rules.write_text("[mcp]\nallow = ['review/*', 'separate/*']\n")
    succeeds = preview_loadout(bundle, selected, store, platform=Platform.LINUX)
    assert apply_loadout(succeeds).sync_required


@pytest.mark.parametrize(
    "table",
    [
        "bad = 1",
        "[bad]\ntransport='pipe'",
        "[bad]\ntransport='http'\nurl=false",
        "[bad]\ntransport='stdio'\ncommand='review'\ncooding='utf8'",
        "[bad]\ntransport='stdio'\ncommand='review'\nheaders={Accept='text/plain'}",
        "[bad]\ntransport='stdio'\ncommand='review'\nargs=1",
        "[bad]\ntransport='stdio'\ncommand='review'\nenv='LOCAL'",
        "[bad]\ntransport='stdio'\ncommand='review'\nenv={TEST=3}",
        "[bad]\ntransport='http'\nurl='https://example.test/mcp'\nheaders='wrong-shaped'",
        "[bad]\ntransport='http'\nurl='https://example.test/mcp'\nheaders={Accept='!exec'}",
        "[bad]\ntransport='http'\nurl='https://example.test/mcp'\nheaders={Accept='${SECRET}'}",
        "[bad]\ntransport='http'\nurl='https://example.test/mcp'\nheaders={Accept='{env:SECRET}'}",
        "[bad]\ntransport='http'\nurl='https://example.test/mcp'\nheaders={Accept=5}",
        "[bad]\ntransport='http'\nurl='https://example.test/mcp'\nauth_env_var=false",
        (
            "[bad]\ntransport='http'\nurl='https://example.test/mcp'\n"
            "auth_env_var='TOKEN'\nheaders={authorization='Bearer direct'}"
        ),
    ],
)
def test_mcp_source_refuses_invalid_existing_data(tmp_path: Path, table: str) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + "[opencode]\n")
    existing = source / "mcp.toml"
    existing.write_text(table)
    store = DirectoryStore(tmp_path / "state")
    plan = preview_loadout(
        mcp_command("review"), choose(home, Agent.OPENCODE), store, platform=Platform.LINUX
    )
    assert plan.diagnostic and plan.diagnostic.code == "invalid-mcp"
    assert plan.diagnostic.path == existing
    assert existing.read_text() == table


@pytest.mark.parametrize(
    "rules",
    ["mcp = []", "[mcp]\nunknown = []", "[mcp]\nallow = 'server/tool'", "[mcp]\nallow = ['bad']"],
)
def test_mcp_source_refuses_invalid_permission_inputs(tmp_path: Path, rules: str) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + "[opencode]\n")
    existing = source / "permissions.toml"
    existing.write_text(rules)
    plan = preview_loadout(
        mcp_command("review"),
        choose(home, Agent.OPENCODE),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
    )
    assert plan.diagnostic and plan.diagnostic.code == "invalid-permissions"
    assert plan.diagnostic.path == existing


@pytest.mark.integration
def test_loadout_same_bundle_cannot_claim_another_profiles_resources(tmp_path: Path) -> None:
    home, source, registration = setup_files(tmp_path, SOURCE + "[claude]\n")
    store = DirectoryStore(tmp_path / "state")
    first = preview_loadout(
        mcp_command("review"), choose(home, Agent.CLAUDE), store, platform=Platform.LINUX
    )
    assert apply_loadout(first).sync_required
    (source / "work.toml").write_text("extends = 'default'")
    registration.write_text(registration.read_text().replace("'default'", "'work'"))
    second = preview_loadout(
        mcp_command("review"), choose(home, Agent.CLAUDE), store, platform=Platform.LINUX
    )
    assert second.diagnostic and second.diagnostic.code == "source-ownership"
    assert apply_loadout(second).lifecycle.error is not None
    assert inspect_loadout("toolbox", first.location, store).is_current
    assert (source / "mcp.toml").is_file()


def test_loadout_declined_preview_reports_all_shared_effects(tmp_path: Path) -> None:
    home, source, _ = setup_files(tmp_path, MANIFEST, profile="work")
    (source / "work.toml").write_text("extends = 'default'\n")
    store = DirectoryStore(tmp_path / "state")
    plan = preview_loadout(desired(), choose(home, Agent.CLAUDE), store, platform=Platform.LINUX)
    assert plan.diagnostic and plan.diagnostic.code == "shared-scope"
    shared = [(part.category, part.harnesses) for part in plan.impact if part.shared]
    assert set(shared) == {
        ("instructions", (Agent.CLAUDE,)),
        ("mcp", (Agent.CLAUDE, Agent.CODEX)),
    }
    assert "Other profiles and projects" in plan.scope_notice
    assert (source / "loadout.toml").read_text() == MANIFEST


HOOKS = (
    SOURCE
    + """[claude]
hooks = []
[codex]
hooks = []
[cursor]
hooks = []
[opencode]
[pi]
"""
)


@pytest.mark.parametrize(
    "agent", [Agent.CLAUDE, Agent.CODEX, Agent.CURSOR, Agent.OPENCODE, Agent.PI]
)
def test_loadout_preview_renders_hook_registrations_and_support_assets(
    tmp_path: Path, agent: Agent
) -> None:
    home, source, _ = setup_files(tmp_path, HOOKS)
    asset = SupportAsset("tool", Family.HOOKS, TreeContent([BundleEntry("review.mjs", b"fixture")]))
    command = Command(["node", AssetRef("tool", "review.mjs")])
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"),
        [
            HookArtifact("first", HookEvent.BEFORE_TOOL, command, outcomes=[HookOutcome.BLOCK]),
            HookArtifact("second", HookEvent.AFTER_TOOL, command),
        ],
        assets=[asset],
        dependencies=[Dependency("first", "tool"), Dependency("second", "tool")],
    )
    runtime = HookRuntime(node=tmp_path / "node", shell=HookShell.POSIX)
    plan = preview_loadout(
        bundle,
        choose(home, agent),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
        hook_runtime=runtime,
    )
    assert plan.applicable, plan.diagnostic or plan.lifecycle.error
    outputs = plan.lifecycle.rendered.artifacts
    assert any(item.destination.is_relative_to(source / ".flyrail-assets") for item in outputs)
    if agent in (Agent.OPENCODE, Agent.PI):
        [module] = [
            item for item in outputs if item.destination.is_relative_to(source / "module-config")
        ]
        assert module.family is Family.HOOKS and module.destination.suffix == ".ts"
        assert ".flyrail-assets" in module.content.data.decode()  # type: ignore[union-attr]
    else:
        [fragment] = [item for item in outputs if item.destination.is_relative_to(source / "hooks")]
        assert isinstance(fragment.content, FileContent)
        parsed = json.loads(fragment.content.data)
        assert sorted(parsed) == ["PostToolUse", "PreToolUse"]
        assert all(value[0]["hooks"][0]["type"] == "command" for value in parsed.values())


@pytest.mark.parametrize(
    ("tail", "agent", "runtime", "code"),
    [
        ("[droid]\nhooks=[]", Agent.DROID, True, "unsupported-hook"),
        ("[claude]\nhooks=[]", Agent.COPILOT, True, "unsupported-harness"),
        ("[claude]\nhooks=[]", Agent.CLAUDE, False, "hook-runtime"),
        ("[claude]\nhooks=false", Agent.CLAUDE, True, "disabled-slice"),
        ("[claude]\nhooks='local/welcome'", Agent.CLAUDE, True, "selector-not-array"),
        ("[all]\nhooks=[]\n[claude]\n[codex]\n", Agent.CLAUDE, True, "hook-shared-selector"),
    ],
)
def test_loadout_rejects_unrepresentable_hook_routes(
    tmp_path: Path, tail: str, agent: Agent, runtime: bool, code: str
) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + tail)
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"),
        [HookArtifact("hook", HookEvent.SESSION_START, Command(["review"]))],
    )
    plan = preview_loadout(
        bundle,
        choose(home, agent),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
        hook_runtime=HookRuntime(node=tmp_path / "node", shell=HookShell.POSIX)
        if runtime
        else None,
    )
    assert plan.diagnostic and plan.diagnostic.code == code
    assert (source / "loadout.toml").read_text() == SOURCE + tail


@pytest.mark.parametrize(
    ("agent", "path", "category"),
    [
        (Agent.OPENCODE, ".config/opencode/plugins/review.mjs", "module-config"),
        (Agent.PI, ".pi/agent/extensions/review.mjs", "module-config"),
        (Agent.CLAUDE, ".claude/plugin/review.js", "native-path"),
    ],
)
def test_loadout_native_file_mappings(
    tmp_path: Path, agent: Agent, path: str, category: str
) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + f"[{agent}]")
    content = FileContent(b"export default {};\n")
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"),
        [
            NativeArtifact(
                "module",
                Family.HOOKS,
                Audience(agent, TargetScope.USER, Surface.CLI),
                path,
                content,
            )
        ],
    )
    plan = preview_loadout(
        bundle,
        choose(home, agent),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
    )
    if category == "module-config":
        assert plan.applicable
        [artifact] = plan.lifecycle.rendered.artifacts
        assert artifact.content == content
        assert artifact.destination.parent.parent == source / "module-config" / str(agent)
    else:
        assert plan.diagnostic and plan.diagnostic.code == category
    assert sorted(path.name for path in source.iterdir()) == ["loadout.toml"]


def test_loadout_unselected_hook_list_cannot_gain_an_implicit_registration(tmp_path: Path) -> None:
    encoded = hashlib.sha256(b"hooks\0review\0claude").hexdigest()
    entry = f"local/flyrail-{encoded}"
    home, source, _ = setup_files(
        tmp_path, SOURCE + f"[claude]\nhooks=[]\n[codex]\nhooks=['{entry}']"
    )
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"),
        [HookArtifact("hook", HookEvent.SESSION_START, Command(["review"]))],
    )
    plan = preview_loadout(
        bundle,
        choose(home, Agent.CLAUDE),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
        hook_runtime=HookRuntime(node=tmp_path / "node", shell=HookShell.POSIX),
    )
    assert plan.diagnostic and plan.diagnostic.code == "selector-shadowed"
    assert plan.diagnostic.path == source / "loadout.toml"


@pytest.mark.integration
def test_loadout_checks_routing_after_pending_index_write(tmp_path: Path) -> None:
    home, source, registration = setup_files(tmp_path, SOURCE + "[claude]")
    original = registration.read_text()

    class RegistrationChangingStore(DirectoryStore):
        def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
            written = super().write(key, value, expected=expected)
            if written and key.startswith("index/"):
                registration.write_text(original + "# changed after preparation\n")
            return written

    store = RegistrationChangingStore(tmp_path / "state")
    selected = choose(home, Agent.CLAUDE)
    plan = preview_loadout(mcp_command("review"), selected, store, platform=Platform.LINUX)
    assert plan.applicable
    result = apply_loadout(plan).lifecycle
    assert result.error and result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert sorted(item.name for item in source.iterdir()) == ["loadout.toml"]


def test_unprofiled_legacy_instruction_requires_shared_scope(tmp_path: Path) -> None:
    shared = "[instructions.claude]\norder=[]\noutput='CLAUDE.md'\n"
    named = "[instructions.codex]\nprofile='work'\norder=[]\noutput='AGENTS.md'\n"
    home, source, _ = setup_files(tmp_path, SOURCE + shared + named, profile="work")
    bundle = Bundle.from_artifacts(
        BundleIdentity("legacy", "1"), [InstructionArtifact("guide", "Read the diff.")]
    )
    preview = preview_loadout(
        bundle,
        choose(home, Agent.CLAUDE),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
    )
    assert preview.diagnostic and preview.diagnostic.code == "shared-scope"
    [impact] = preview.impact
    assert impact.path == source / "loadout.toml" and impact.shared


@pytest.mark.integration
def test_loadout_pending_profile_cannot_remove_another_profiles_claim(tmp_path: Path) -> None:
    home, source, registration = setup_files(tmp_path, SOURCE + "[claude]")
    (source / "team.toml").write_text("extends = 'default'\n")

    class ProfileSwitchingStore(DirectoryStore):
        switched = False

        def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
            changed = super().write(key, value, expected=expected)
            if changed and not self.switched and key.startswith("index/"):
                self.switched = True
                registration.write_text(registration.read_text().replace("'default'", "'team'"))
            return changed

    store = ProfileSwitchingStore(tmp_path / "state")
    bundle = mcp_command("review")
    first = preview_loadout(bundle, choose(home, Agent.CLAUDE), store, platform=Platform.LINUX)
    interrupted = apply_loadout(first).lifecycle
    assert interrupted.error and interrupted.error.code is ErrorCode.CONCURRENT_CHANGE
    assert interrupted.observation.pending
    second = preview_loadout(bundle, choose(home, Agent.CLAUDE), store, platform=Platform.LINUX)
    assert apply_loadout(second).lifecycle.status is OperationStatus.APPLIED
    old_removal = preview_loadout_removal(bundle.id, first.location, store)
    assert old_removal.diagnostic and old_removal.diagnostic.code == "source-ownership"
    assert apply_loadout(old_removal).lifecycle.status is OperationStatus.FAILED
    assert inspect_loadout(bundle.id, second.location, store).is_current
    assert tomllib.loads((source / "mcp.toml").read_text())["review"]["command"] == "review"


@pytest.mark.integration
def test_loadout_keeps_checking_logical_alias_after_publishing_a_manifest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + "[claude]\ninstructions=[]\n", profile="work")
    original_parent = source / "a-parent.toml"
    new_parent = source / "b-parent.toml"
    overlay = "extends = 'default'\n[claude]\ninstructions = []\n"
    original_parent.write_text(overlay)
    new_parent.write_text(overlay)
    alias = source / "parent.toml"
    alias.symlink_to(original_parent.name)
    work = source / "work.toml"
    untouched = "extends = 'parent'\n[codex]\ninstructions = []\n"
    work.write_text(untouched)
    store = DirectoryStore(tmp_path / "state")
    bundle = Bundle.from_artifacts(
        BundleIdentity("review", "1"), [InstructionArtifact("guide", "Read the diff.")]
    )
    selected = choose(home, Agent.CLAUDE, Agent.CODEX, shared=True)
    approved = preview_loadout(bundle, selected, store, platform=Platform.LINUX)
    assert approved.applicable

    def publish(store: DirectoryStore, journal: _resource_transaction.Journal) -> None:
        _resource_transaction.publish(store, journal)
        if journal.resource.destination == original_parent:
            alias.unlink()
            alias.symlink_to(new_parent.name)

    monkeypatch.setattr(_lifecycle, "publish", publish)
    applied = apply_loadout(approved).lifecycle
    assert applied.status is OperationStatus.PARTIAL
    assert applied.error and applied.error.code is ErrorCode.CONCURRENT_CHANGE
    assert tomllib.loads(original_parent.read_text())["claude"]["instructions"]
    assert work.read_text() == untouched


def test_pi_permission_rule_for_another_name_cannot_authorize_a_server_alias(
    tmp_path: Path,
) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + "[pi]\n")
    permissions = source / "permissions.toml"
    permissions.write_text("[mcp]\nallow = ['review_tools/*']\n")
    plan = preview_loadout(
        mcp_command("review-tools"),
        choose(home, Agent.PI),
        DirectoryStore(tmp_path / "state"),
        platform=Platform.LINUX,
    )
    assert plan.diagnostic and plan.diagnostic.code == "mcp-permission-overlap"
    assert plan.diagnostic.path == permissions


@pytest.mark.integration
def test_loadout_retires_own_claims_after_two_interrupted_previews(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home, source, _ = setup_files(tmp_path, SOURCE + "[claude]\ninstructions=[]\n")
    store = DirectoryStore(tmp_path / "state")

    def plan(version: str, text: str) -> LoadoutPreview:
        bundle = Bundle.from_artifacts(
            BundleIdentity("review", version), [InstructionArtifact("guide", text)]
        )
        return preview_loadout(bundle, choose(home, Agent.CLAUDE), store, platform=Platform.LINUX)

    def break_selector(store: DirectoryStore, journal: _resource_transaction.Journal) -> None:
        if journal.resource.destination == source / "loadout.toml":
            raise OSError("fixture interrupted selector publication")
        _resource_transaction.publish(store, journal)

    def break_every_write(_store: DirectoryStore, _journal: _resource_transaction.Journal) -> None:
        raise OSError("fixture interrupted the next revision")

    first = plan("1", "First revision.")
    with monkeypatch.context() as patch:
        patch.setattr(_lifecycle, "publish", break_selector)
        assert apply_loadout(first).lifecycle.status is OperationStatus.PARTIAL
    [fragment] = list((source / "instructions").glob("*.md"))
    assert fragment.read_text() == "First revision."
    second = plan("2", "Second revision.")
    assert second.applicable
    with monkeypatch.context() as patch:
        patch.setattr(_lifecycle, "publish", break_every_write)
        assert apply_loadout(second).lifecycle.status is OperationStatus.FAILED
    removal = preview_loadout_removal("review", first.location, store)
    assert removal.applicable
    assert apply_loadout(removal).lifecycle.status is OperationStatus.APPLIED
    assert not fragment.exists()
    assert (source / "loadout.toml").read_text() == SOURCE + "[claude]\ninstructions=[]\n"
