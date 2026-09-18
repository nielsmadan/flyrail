import json
from pathlib import Path

from flyrail import Bundle


def make_bundle(
    root: Path,
    identifier: str = "team",
    version: str = "1",
    names: tuple[str, ...] = ("review",),
    data: bytes = b"original",
    executable: bool = False,
) -> Bundle:
    root.mkdir(parents=True)
    specs: list[dict[str, object]] = []
    for name in names:
        skill = root / name
        skill.mkdir()
        (skill / "SKILL.md").write_bytes(data)
        (skill / "empty").mkdir()
        (skill / "run").write_bytes(b"run")
        specs.append({"name": name, "path": name, "executables": ["run"] if executable else []})
    (root / "flyrail.json").write_text(
        json.dumps({"schema_version": 1, "id": identifier, "version": version, "skills": specs}),
        encoding="utf-8",
    )
    return Bundle.from_directory(root)


def snapshot(root: Path) -> dict[str, tuple[int, int, bytes | None]]:
    return {
        path.relative_to(root).as_posix(): (
            path.stat().st_mode,
            path.stat().st_mtime_ns,
            path.read_bytes() if path.is_file() else None,
        )
        for path in root.rglob("*")
    }
