import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import TypedDict, cast

SEMVER_PARTS = 3


class ReleaseError(Exception):
    pass


class StageConfig(TypedDict):
    files: list[str]
    commands: list[list[str]]
    checks: list[list[str]]
    message: str


class ReleaseConfig(TypedDict):
    branch: str
    initial_version: str
    tag_prefix: str
    paths: list[str]
    checks: list[list[str]]
    tools: list[str]
    stages: list[StageConfig]
    name: str
    repository: str
    workflow: str
    publication: str


class ReleaseState(TypedDict):
    head: str
    refs: dict[str, str]
    latest: str | None
    messages: list[str]
    ahead: str


def _mapping(value: object, context: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ReleaseError(f"{context} must be an object.")
    return cast(dict[str, object], value)


def _keys(value: dict[str, object], expected: set[str], context: str) -> None:
    missing = sorted(expected - value.keys())
    unknown = sorted(value.keys() - expected)
    if missing or unknown:
        details = []
        if missing:
            details.append("missing " + ", ".join(missing))
        if unknown:
            details.append("unknown " + ", ".join(unknown))
        raise ReleaseError(f"{context} has invalid keys: {'; '.join(details)}.")


def _string(value: object, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise ReleaseError(f"{context} must be a non-empty string.")
    return value


def _strings(value: object, context: str) -> list[str]:
    if not isinstance(value, list) or not value:
        raise ReleaseError(f"{context} must be a non-empty string list.")
    return [_string(item, f"{context} item") for item in value]


def _commands(value: object, context: str, *, allow_empty: bool = False) -> list[list[str]]:
    if not isinstance(value, list) or (not value and not allow_empty):
        raise ReleaseError(f"{context} must be a command list.")
    return [_strings(item, f"{context} command") for item in value]


def load_config(path: Path) -> ReleaseConfig:
    raw = _mapping(json.loads(path.read_text()), "release configuration")
    expected = {
        "branch",
        "initial_version",
        "tag_prefix",
        "paths",
        "checks",
        "tools",
        "stages",
        "name",
        "repository",
        "workflow",
        "publication",
    }
    _keys(raw, expected, "release configuration")
    raw_stages = raw["stages"]
    if not isinstance(raw_stages, list) or not raw_stages:
        raise ReleaseError("release configuration stages must be a non-empty list.")
    stages: list[StageConfig] = []
    for index, value in enumerate(raw_stages):
        context = f"release stage {index + 1}"
        stage = _mapping(value, context)
        _keys(stage, {"files", "commands", "checks", "message"}, context)
        stages.append(
            {
                "files": _strings(stage["files"], f"{context} files"),
                "commands": _commands(stage["commands"], f"{context} commands"),
                "checks": _commands(stage["checks"], f"{context} checks", allow_empty=True),
                "message": _string(stage["message"], f"{context} message"),
            }
        )
    config = cast(
        ReleaseConfig,
        {
            key: _string(raw[key], f"release configuration {key}")
            for key in (
                "branch",
                "initial_version",
                "tag_prefix",
                "name",
                "repository",
                "workflow",
                "publication",
            )
        },
    )
    config.update(
        {
            "paths": _strings(raw["paths"], "release configuration paths"),
            "checks": _commands(raw["checks"], "release configuration checks"),
            "tools": _strings(raw["tools"], "release configuration tools"),
            "stages": stages,
        }
    )
    version_tuple(config["initial_version"])
    if not config["repository"].count("/") == 1:
        raise ReleaseError("release configuration repository must be owner/name.")
    return config


def run(root: Path, *args: str, capture: bool = True) -> str:
    environment = os.environ.copy()
    environment.setdefault("UV_TOOL_DIR", str(root / ".cache/uv/tools"))
    environment.setdefault("UV_CACHE_DIR", str(root / ".cache/uv/cache"))
    result = subprocess.run(  # noqa: S603
        args,
        cwd=root,
        env=environment,
        text=True,
        capture_output=capture,
        check=False,
    )
    if result.returncode:
        detail = (result.stderr or result.stdout or "").strip() if capture else ""
        raise ReleaseError(f"{' '.join(args)} failed ({result.returncode}). {detail}")
    return result.stdout.strip() if capture else ""


def version_tuple(value: str) -> tuple[int, int, int]:
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ReleaseError(f"Expected a three-part version, got {value!r}.")
    major, minor, patch = value.split(".")
    return int(major), int(minor), int(patch)


def tag_version(tag: str, prefix: str) -> tuple[int, int, int] | None:
    if not tag.startswith(prefix):
        return None
    try:
        return version_tuple(tag.removeprefix(prefix))
    except ReleaseError:
        return None


def known_tag_version(tag: str, prefix: str) -> tuple[int, int, int]:
    parsed = tag_version(tag, prefix)
    if parsed is None:
        raise ReleaseError(f"Invalid release tag: {tag!r}")
    return parsed


def bumped(base: tuple[int, int, int], kind: str) -> str:
    parts = list(base)
    index = {"major": 0, "minor": 1, "patch": 2}[kind]
    parts[index] += 1
    parts[index + 1 :] = [0] * (SEMVER_PARTS - index - 1)
    return ".".join(map(str, parts))


def suggested(base: tuple[int, int, int], messages: list[str]) -> tuple[str | None, dict[str, int]]:
    counts = {"features": 0, "fixes": 0, "breaking": 0}
    for message in messages:
        header = message.splitlines()[0]
        match = re.match(r"(\w+)(?:\([^\n]*\))?(!)?:", header)
        if match is None:
            continue
        if match[2] or re.search(r"^BREAKING[ -]CHANGE:", message, re.MULTILINE):
            counts["breaking"] += 1
        if match[1] == "feat":
            counts["features"] += 1
        elif match[1] in {"fix", "perf"}:
            counts["fixes"] += 1
    if counts["breaking"]:
        kind = "minor" if base[0] == 0 else "major"
    elif counts["features"]:
        kind = "minor"
    elif counts["fixes"]:
        kind = "patch"
    else:
        return None, counts
    return bumped(base, kind), counts


def remote_refs(root: Path) -> dict[str, str]:
    result = run(root, "git", "ls-remote", "--heads", "--tags", "origin")
    return dict(line.split()[::-1] for line in result.splitlines())


def clean(root: Path) -> None:
    status = run(root, "git", "status", "--porcelain", "--untracked-files=all")
    if status:
        raise ReleaseError("The checkout must be clean before releasing:\n" + status)


def github_repository(origin: str) -> str | None:
    patterns = (
        r"git@github\.com:([^/]+/[^/]+?)(?:\.git)?",
        r"ssh://git@github\.com/([^/]+/[^/]+?)(?:\.git)?",
        r"https://github\.com/([^/]+/[^/]+?)(?:\.git)?",
    )
    for pattern in patterns:
        if match := re.fullmatch(pattern, origin):
            return match[1]
    return None


def inspect(root: Path, config: ReleaseConfig) -> ReleaseState:
    clean(root)
    branch = run(root, "git", "symbolic-ref", "--short", "HEAD")
    if branch != config["branch"]:
        raise ReleaseError(f"Release from {config['branch']}, not {branch}.")
    if run(root, "git", "rev-parse", "--is-shallow-repository") == "true":
        raise ReleaseError("Release calculation requires complete Git history and tags.")
    origin = run(root, "git", "remote", "get-url", "--all", "origin")
    push_origin = run(root, "git", "remote", "get-url", "--push", "--all", "origin")
    if "\n" in origin or origin != push_origin:
        raise ReleaseError("origin must have one matching fetch and push destination.")
    if github_repository(origin) != config["repository"]:
        raise ReleaseError("origin does not match the configured GitHub release repository.")
    refs = remote_refs(root)
    head = run(root, "git", "rev-parse", "HEAD")
    branch_ref = "refs/heads/" + config["branch"]
    remote_head = refs.get(branch_ref)
    if remote_head is None:
        raise ReleaseError(f"origin/{config['branch']} does not exist.")
    try:
        run(root, "git", "merge-base", "--is-ancestor", remote_head, head)
    except ReleaseError as error:
        raise ReleaseError(f"Update this checkout to include origin/{config['branch']}.") from error
    prefix = config["tag_prefix"]
    local_tags = [
        tag
        for tag in run(root, "git", "tag", "--list", prefix + "*").splitlines()
        if tag_version(tag, prefix) is not None
    ]
    remote_tags = [
        ref.removeprefix("refs/tags/")
        for ref in refs
        if ref.startswith("refs/tags/")
        and not ref.endswith("^{}")
        and tag_version(ref.removeprefix("refs/tags/"), prefix) is not None
    ]
    if set(local_tags) != set(remote_tags):
        raise ReleaseError("Local and origin release tags differ; reconcile them before releasing.")
    latest = max(local_tags, key=lambda tag: known_tag_version(tag, prefix), default=None)
    remote_latest = max(remote_tags, key=lambda tag: known_tag_version(tag, prefix), default=None)
    if latest != remote_latest:
        raise ReleaseError("Local and origin release tags differ; reconcile them before releasing.")
    if latest is not None:
        target = run(root, "git", "rev-parse", latest + "^{commit}")
        remote_target = refs.get("refs/tags/" + latest + "^{}", refs["refs/tags/" + latest])
        if target != remote_target:
            raise ReleaseError(f"Local {latest} does not match origin.")
        run(root, "git", "merge-base", "--is-ancestor", latest, head)
    revision_range = latest + "..HEAD" if latest else "HEAD"
    log = run(
        root,
        "git",
        "log",
        "--format=%B%x00",
        revision_range,
        "--",
        *config["paths"],
    )
    messages = [message.strip() for message in log.split("\0") if message.strip()]
    if not messages:
        raise ReleaseError("There are no component commits to release.")
    ahead = run(root, "git", "rev-list", "--count", remote_head + "..HEAD")
    return {
        "head": head,
        "refs": refs,
        "latest": latest,
        "messages": messages,
        "ahead": ahead,
    }


def select_version(value: str, state: ReleaseState, config: ReleaseConfig) -> str:
    prefix = config["tag_prefix"]
    latest = tag_version(state["latest"], prefix) if state["latest"] else None
    base = latest or version_tuple(config["initial_version"])
    candidate = bumped(base, value) if value in {"patch", "minor", "major"} else value
    parsed = version_tuple(candidate)
    if latest is not None and parsed <= latest:
        raise ReleaseError(f"The release must be newer than {state['latest']}.")
    return ".".join(map(str, parsed))


def preview(
    config: ReleaseConfig,
    state: ReleaseState,
    version: str | None,
    counts: dict[str, int],
) -> None:
    tag = config["tag_prefix"] + version if version else "(no automatic bump)"
    print(f"\n{config['name']} release", flush=True)
    print(f"Current:   {state['latest'] or '(first release)'}")
    print(f"Proposed:  {tag}")
    print(
        f"Changes:   {counts['features']} features, {counts['fixes']} fixes, "
        f"{counts['breaking']} breaking"
    )
    print(
        f"Push:      origin/{config['branch']} ({state['ahead']} existing local commits) "
        "and release tag"
    )
    for stage in config["stages"]:
        print("Prepare:   " + ", ".join(stage["files"]))
    print("Publish:   " + config["publication"])


def confirm(
    config: ReleaseConfig,
    state: ReleaseState,
    version: str | None,
    counts: dict[str, int],
) -> str | None:
    while True:
        preview(config, state, version, counts)
        answer = input(
            "Confirm [y], enter a version/patch/minor/major, or cancel [Enter]: "
        ).strip()
        if answer.lower() in {"", "n", "no", "q", "quit"}:
            return None
        if answer.lower() in {"y", "yes"}:
            if version:
                return version
            print("Choose an explicit version or bump first.")
            continue
        try:
            version = select_version(answer, state, config)
        except ReleaseError as error:
            print(error)


def changed_paths(root: Path) -> set[str]:
    paths: set[str] = set()
    for arguments in (
        ("diff", "--name-only", "-z"),
        ("diff", "--cached", "--name-only", "-z"),
        ("ls-files", "--others", "--exclude-standard", "-z"),
    ):
        paths.update(run(root, "git", *arguments).split("\0"))
    return paths - {""}


def prepare(root: Path, config: ReleaseConfig, version: str) -> None:
    for stage in config["stages"]:
        clean(root)
        values = {
            "version": version,
            "revision": run(root, "git", "rev-parse", "HEAD"),
            "python": sys.executable,
        }
        for command in stage["commands"]:
            run(root, *(argument.format(**values) for argument in command), capture=False)
        paths = changed_paths(root)
        unexpected = {
            path
            for path in paths
            if not any(
                path == allowed or path.startswith(allowed + "/") for allowed in stage["files"]
            )
        }
        if unexpected:
            raise ReleaseError(
                "Preparation changed unexpected files: " + ", ".join(sorted(unexpected))
            )
        for command in stage.get("checks", []):
            print("Checking prepared release: " + " ".join(command), flush=True)
            run(root, *command, capture=False)
        paths = changed_paths(root)
        unexpected = {
            path
            for path in paths
            if not any(
                path == allowed or path.startswith(allowed + "/") for allowed in stage["files"]
            )
        }
        if unexpected:
            raise ReleaseError(
                "Prepared checks changed unexpected files: " + ", ".join(sorted(unexpected))
            )
        if paths:
            run(root, "git", "add", "--", *sorted(paths), capture=False)
            run(
                root,
                "git",
                "commit",
                "--only",
                "-m",
                stage["message"].format(**values),
                "--",
                *sorted(paths),
                capture=False,
            )
        clean(root)


def report_publication(root: Path, config: ReleaseConfig, tag: str, revision: str) -> None:
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        runs = json.loads(
            run(
                root,
                "gh",
                "run",
                "list",
                "--repo",
                config["repository"],
                "--workflow",
                config["workflow"],
                "--event",
                "push",
                "--commit",
                revision,
                "--json",
                "databaseId,headBranch,url",
                "--limit",
                "20",
            )
        )
        matches = [item for item in runs if item["headBranch"] == tag]
        if matches:
            workflow_run = matches[0]
            break
        time.sleep(3)
    else:
        raise ReleaseError(
            f"{tag} was pushed, but its release workflow has not appeared. Check GitHub Actions."
        )
    print("Publication: " + workflow_run["url"], flush=True)
    deadline = time.monotonic() + 7200
    while time.monotonic() < deadline:
        result = json.loads(
            run(
                root,
                "gh",
                "run",
                "view",
                str(workflow_run["databaseId"]),
                "--repo",
                config["repository"],
                "--json",
                "status,conclusion",
            )
        )
        if result["status"] == "completed":
            if result["conclusion"] != "success":
                raise ReleaseError(
                    f"Publication finished with {result['conclusion']}: {workflow_run['url']}"
                )
            published = json.loads(
                run(
                    root,
                    "gh",
                    "release",
                    "view",
                    tag,
                    "--repo",
                    config["repository"],
                    "--json",
                    "url,isDraft",
                )
            )
            if published["isDraft"] is not False:
                raise ReleaseError(f"Expected a published release: {published['url']}")
            print("Released: " + published["url"])
            return
        time.sleep(5)
    raise ReleaseError("Publication is still running: " + workflow_run["url"])


def proposal(
    state: ReleaseState, config: ReleaseConfig, override: str | None
) -> tuple[str | None, dict[str, int]]:
    latest = (
        tag_version(state["latest"], config["tag_prefix"]) if state["latest"] is not None else None
    )
    if state["latest"] is not None and latest is None:
        raise ReleaseError(f"Invalid current release tag: {state['latest']!r}")
    base = latest or version_tuple(config["initial_version"])
    version, counts = suggested(base, state["messages"])
    if latest is None:
        version = config["initial_version"]
    if override:
        version = select_version(override, state, config)
    return version, counts


def main() -> None:
    parser = argparse.ArgumentParser(description="Preview, confirm, and publish a Python release.")
    parser.add_argument("version", nargs="?", help="patch, minor, major, or an exact version")
    parser.add_argument(
        "--yes", action="store_true", help="confirm the proposed release without a prompt"
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="inspect and preview; do not check or publish"
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    config = load_config(Path(__file__).with_name("release.json"))
    if not args.yes and not args.dry_run and not sys.stdin.isatty():
        raise ReleaseError(
            "Interactive confirmation needs a terminal; use --dry-run or explicit --yes."
        )
    required_tools = ["git"] if args.dry_run else config["tools"]
    for tool in required_tools:
        if shutil.which(tool) is None:
            raise ReleaseError(f"Required release tool is missing: {tool}")
    state = inspect(root, config)
    version, counts = proposal(state, config, args.version)
    if args.dry_run:
        preview(config, state, version, counts)
        for command in config["checks"]:
            print("Check:     " + " ".join(command))
        print("Dry run: checks and publication were not run.")
        return
    run(root, "gh", "repo", "view", config["repository"], "--json", "nameWithOwner")
    if args.yes and version is None:
        raise ReleaseError("No automatic release is due. Supply an explicit version or bump.")
    for command in config["checks"]:
        print("Checking: " + " ".join(command), flush=True)
        run(root, *command, capture=False)
    clean(root)
    if run(root, "git", "rev-parse", "HEAD") != state["head"]:
        raise ReleaseError("HEAD changed while checking the release.")
    if args.yes:
        preview(config, state, version, counts)
    else:
        version = confirm(config, state, version, counts)
    if version is None:
        print("Release cancelled.")
        return
    if inspect(root, config) != state:
        raise ReleaseError("The checkout or origin changed during review. Run release again.")
    prepare(root, config, version)
    tag = config["tag_prefix"] + version
    run(root, "git", "tag", "-a", "-m", f"Release {tag}", tag, capture=False)
    run(
        root,
        "git",
        "push",
        "--atomic",
        "--no-follow-tags",
        "origin",
        "HEAD:refs/heads/" + config["branch"],
        "refs/tags/" + tag,
        capture=False,
    )
    report_publication(root, config, tag, run(root, "git", "rev-parse", "HEAD"))


if __name__ == "__main__":
    try:
        main()
    except (ReleaseError, OSError, EOFError, KeyboardInterrupt, json.JSONDecodeError) as error:
        print(f"Release stopped: {error}", file=sys.stderr)
        sys.exit(1)
