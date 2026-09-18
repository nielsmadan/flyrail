import email.parser
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
import zipfile
from pathlib import Path

from check import REPOSITORY, ROOT, tool_environment, uv_executable

EXCLUDED = {".git", ".cache", ".venv", "__pycache__", "build", "dist", ".DS_Store"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def run(command: list[str], cwd: Path, environment: dict[str, str]) -> None:
    print(f"+ {' '.join(command)}", flush=True)
    subprocess.run(command, cwd=cwd, env=environment, check=True)  # noqa: S603


def distributable(path: Path) -> bool:
    return (
        not EXCLUDED.intersection(path.parts)
        and not any(part.endswith(".egg-info") for part in path.parts)
        and not path.name.endswith((".pyc", ".pyo", ".pyd", ".log"))
    )


def verify_portability(members: dict[str, bytes]) -> None:
    home_path = re.compile(rb"(?:/Users/|/home/|[A-Za-z]:[\\/]Users[\\/])[\w.-]+[\\/]")
    checkout = REPOSITORY.as_posix().encode()
    for name, data in members.items():
        require(distributable(Path(name)), f"local artifact in distribution: {name}")
        normalized = data.replace(b"\\\\", b"\\").replace(b"\\", b"/")
        require(
            not home_path.search(normalized) and checkout not in normalized,
            f"machine-specific path in distribution: {name}",
        )


def expected_files(project: Path, selected: list[str]) -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for name in selected:
        root = project / name.lstrip("/")
        for path in root.rglob("*") if root.is_dir() else [root]:
            relative = path.relative_to(project)
            if path.is_file() and distributable(relative):
                files[relative.as_posix()] = path.read_bytes()
    return files


def verify_artifacts(project: Path, wheel: Path, sdist: Path) -> None:
    config = tomllib.loads((project / "pyproject.toml").read_text(encoding="utf-8"))
    package = config["tool"]["hatch"]["build"]["targets"]["wheel"]["packages"][0]
    expected_wheel = {
        str(Path(name).relative_to("src")).replace(os.sep, "/"): data
        for name, data in expected_files(project, [package]).items()
    }
    with zipfile.ZipFile(wheel) as archive:
        members = {
            name: archive.read(name) for name in archive.namelist() if not name.endswith("/")
        }
    payload = {name: data for name, data in members.items() if ".dist-info/" not in name}
    verify_portability(members)
    require(payload == expected_wheel, f"wheel content differs from package sources: {wheel}")
    metadata_name = next(name for name in members if name.endswith(".dist-info/METADATA"))
    metadata = email.parser.BytesParser().parsebytes(members[metadata_name])
    require(metadata["Name"] == config["project"]["name"], "distribution name differs")
    require(metadata["Version"] == config["project"]["version"], "application version differs")
    require(metadata["Requires-Python"] == ">=3.11", "Python requirement differs")
    require(
        metadata.get_all("Requires-Dist", []) == config["project"]["dependencies"],
        "published runtime dependencies differ",
    )
    with tarfile.open(sdist) as archive:
        contents: dict[str, bytes] = {}
        for member in archive.getmembers():
            require(member.isfile() or member.isdir(), f"unexpected archive member: {member.name}")
            if member.isfile():
                stream = archive.extractfile(member)
                require(stream is not None, f"unreadable archive member: {member.name}")
                if stream is not None:
                    with stream:
                        contents[member.name.split("/", 1)[1]] = stream.read()
    verify_portability(contents)
    require("PKG-INFO" in contents, "sdist metadata missing")
    del contents["PKG-INFO"]
    expected_sdist = expected_files(
        project, config["tool"]["hatch"]["build"]["targets"]["sdist"]["include"]
    )
    if project == ROOT:
        for name in (
            "bundles.json",
            "configurations.json",
            "edits.json",
            "translations.json",
            "hooks.json",
        ):
            expected_sdist[f"spec/fixtures/{name}"] = (
                REPOSITORY / "spec/fixtures" / name
            ).read_bytes()
    changed = sorted(
        name
        for name in contents.keys() & expected_sdist.keys()
        if contents[name] != expected_sdist[name]
    )
    require(
        contents == expected_sdist,
        f"sdist differs: {sdist}; missing={sorted(expected_sdist.keys() - contents.keys())}; "
        f"extra={sorted(contents.keys() - expected_sdist.keys())}; "
        f"changed={changed}",
    )
    print(f"Verified complete wheel and sdist: {project.name}", flush=True)


def dependency_wheels(work: Path) -> list[Path]:
    lock = tomllib.loads((ROOT / "uv.lock").read_text(encoding="utf-8"))
    config = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    wheels: list[Path] = []
    for requirement in config["project"]["dependencies"]:
        name, version = requirement.split("==")
        package = next(
            item for item in lock["package"] if item["name"] == name and item["version"] == version
        )
        (wheel,) = package["wheels"]
        url = wheel["url"]
        require(
            url.startswith("https://files.pythonhosted.org/"), "dependency wheel origin differs"
        )
        with urllib.request.urlopen(url, timeout=60) as response:  # noqa: S310
            data = response.read()
        require(
            "sha256:" + hashlib.sha256(data).hexdigest() == wheel["hash"],
            "dependency wheel digest differs from lock",
        )
        destination = work / url.rsplit("/", 1)[-1]
        destination.write_bytes(data)
        with zipfile.ZipFile(destination) as archive:
            metadata_name = next(
                item for item in archive.namelist() if item.endswith(".dist-info/METADATA")
            )
            metadata = email.parser.BytesParser().parsebytes(archive.read(metadata_name))
        require(
            metadata["Name"] == name and metadata["Version"] == version,
            "dependency metadata differs",
        )
        require(
            not metadata.get_all("Requires-Dist", []),
            "dependency has an unverified runtime dependency",
        )
        wheels.append(destination)
    return wheels


def main() -> int:
    environment = tool_environment()
    environment.pop("PYTHONPATH", None)
    environment.pop("MYPYPATH", None)
    environment["PYTHONNOUSERSITE"] = "1"
    uv = uv_executable()
    require(uv is not None, "uv is required")
    if uv is None:
        return 1
    workspace = ROOT / ".cache/check-package"
    workspace.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="run-", dir=workspace)).resolve()
    consumer = work / "consumer"
    consumer.mkdir()
    build_environment = {**environment, "PATH": ""}
    wheels: list[Path] = []
    for index, project in enumerate(
        [ROOT, ROOT / "examples/filesystem", ROOT / "examples/packaged"]
    ):
        output = work / f"build-{index}"
        build = [uv, "--no-config", "build", "--no-build-isolation", "--python", sys.executable]
        run(
            [*build, "--sdist", "--wheel", "--out-dir", str(output), str(project)],
            consumer,
            build_environment,
        )
        (wheel,) = output.glob("*.whl")
        (sdist,) = output.glob("*.tar.gz")
        verify_artifacts(project, wheel, sdist)
        rebuilt = output / "rebuilt"
        run([*build, "--wheel", "--out-dir", str(rebuilt), str(sdist)], consumer, build_environment)
        (rebuilt_wheel,) = rebuilt.glob("*.whl")
        require(
            wheel.read_bytes() == rebuilt_wheel.read_bytes(), f"sdist rebuild differs: {project}"
        )
        wheels.append(rebuilt_wheel)
    venv = work / "venv"
    run([uv, "--no-config", "venv", "--python", sys.executable, str(venv)], consumer, environment)
    python = venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    run(
        [
            uv,
            "--no-config",
            "pip",
            "install",
            "--python",
            str(python),
            "--no-index",
            "--link-mode",
            "copy",
            *map(str, wheels),
            *map(str, dependency_wheels(work)),
        ],
        consumer,
        environment,
    )
    shutil.copytree(ROOT / "examples/filesystem/bundle", consumer / "notes-bundle")
    for name in (
        "package_probe.py",
        "typed_consumer.py",
        "hook_package_probe.py",
        "configuration_package_probe.py",
    ):
        shutil.copyfile(ROOT / "scripts" / name, consumer / name)
    run(
        [
            sys.executable,
            "-m",
            "mypy",
            "--strict",
            "--no-incremental",
            "--python-executable",
            str(python),
            "--cache-dir",
            str(work / "mypy"),
            "typed_consumer.py",
        ],
        consumer,
        environment,
    )
    run([str(python), "-I", "typed_consumer.py"], consumer, environment)
    run([str(python), "-I", "hook_package_probe.py"], consumer, environment)
    run([str(python), "-I", "configuration_package_probe.py"], consumer, environment)
    run([str(python), "-I", "package_probe.py", str(wheels[2])], consumer, environment)
    print(f"Package QA passed. Built artifacts and isolated installation: {work}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
