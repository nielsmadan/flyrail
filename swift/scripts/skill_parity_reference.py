import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python/src"))

from flyrail.bundle import Bundle
from flyrail.inspection import inspect, render_skills
from flyrail.lifecycle import install
from flyrail.stores import DirectoryStore
from flyrail.targets import Agent, Target


def main():
    language, operation, source, destination, state = sys.argv[1:]
    if language == "rust":
        rust_reference(operation, source, destination, state)
        return
    store = DirectoryStore(Path(state))
    bundle = Bundle.from_directory(Path(source))
    target = Target.directory(Path(destination))
    rendered = render_skills(bundle, target)
    droid = render_skills(bundle, Target.project(Agent.DROID, Path(destination)))
    status = install(bundle, (target,), store=store)[0].status.value if operation == "install" else None
    observed = inspect(bundle, (target,), store=store)[0].observation
    print(json.dumps({
        "status": status,
        "current": observed.is_current,
        "content_matches": observed.content_matches,
        "ids": [artifact.id for artifact in rendered.artifacts],
        "render_digest": rendered.content_digest,
        "droid_ids": [notice.artifact_id for notice in droid.notices],
    }, sort_keys=True))


def rust_reference(operation, source, destination, state):
    root = Path(__file__).resolve().parents[2]
    crate = root / ".cache/swift-skill-parity"
    crate.mkdir(parents=True, exist_ok=True)
    manifest = (
        '[package]\nname = "swift-skill-parity"\nversion = "0.0.0"\nedition = "2024"\n'
        '[workspace]\n[dependencies]\n'
        f'flyrail = {{ path = {json.dumps(str(root / "rust"))} }}\n'
        '[[bin]]\nname = "skill-parity"\n'
        f'path = {json.dumps(str(Path(__file__).with_suffix(".rs")))}\n'
    )
    path = crate / "Cargo.toml"
    if not path.exists() or path.read_text() != manifest:
        path.write_text(manifest)
    process = subprocess.run(
        ["cargo", "run", "--quiet", "--offline", "--manifest-path", str(path), "--",
         operation, source, destination, state],
        env={**os.environ, "CARGO_HOME": str(root / ".cache/cargo-home")},
        text=True, capture_output=True, check=True,
    )
    print(process.stdout, end="")


if __name__ == "__main__":
    main()
