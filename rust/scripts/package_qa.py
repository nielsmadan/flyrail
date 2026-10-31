from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import time
from pathlib import Path


RUST = Path(__file__).resolve().parents[1]
REPO = RUST.parent
CARGO_HOME = REPO / ".cache" / "cargo-home"


def run(arguments: list[str], *, cwd: Path = RUST) -> subprocess.CompletedProcess[bytes]:
    result = subprocess.run(
        arguments,
        cwd=cwd,
        env={**os.environ, "CARGO_HOME": str(CARGO_HOME)},
        capture_output=True,
        check=False,
        timeout=300,
    )
    if result.returncode:
        raise RuntimeError(
            f"{' '.join(arguments)} failed ({result.returncode})\n"
            f"{result.stdout.decode(errors='replace')}\n"
            f"{result.stderr.decode(errors='replace')}"
        )
    return result


def extract(archive: Path, destination: Path) -> Path:
    with tarfile.open(archive) as package:
        package.extractall(destination, filter="data")
    return next(destination.iterdir())


def checksum_directory(root: Path, package_checksum: str) -> None:
    files = {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file() and path.name != ".cargo-checksum.json"
    }
    (root / ".cargo-checksum.json").write_text(
        json.dumps({"files": files, "package": package_checksum}, separators=(",", ":"))
    )


def cargo_config(root: Path, vendor: Path) -> Path:
    config = root / "cargo-config.toml"
    config.write_text(
        '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
        '[source.vendored-sources]\n'
        f'directory = "{vendor}"\n'
    )
    return config


def main() -> None:
    qa = REPO / ".cache" / "tmp" / f"rust-package-qa-{time.time_ns()}"
    target = qa / "target"
    extracted = qa / "extracted"
    vendor = qa / "vendor"
    qa.mkdir(parents=True)
    try:
        common = [
            "cargo",
            "package",
            "--allow-dirty",
            "--no-verify",
            "--offline",
            "--target-dir",
            str(target),
        ]
        run([*common, "-p", "flyrail"])
        root_archive = target / "package" / "flyrail-0.1.0.crate"
        package_checksum = hashlib.sha256(root_archive.read_bytes()).hexdigest()
        with tarfile.open(root_archive) as package:
            license_member = package.extractfile("flyrail-0.1.0/LICENSE")
            assert license_member is not None
            assert license_member.read() == (RUST / "LICENSE").read_bytes()
        run(["cargo", "vendor", "--offline", str(vendor)])
        root_vendor = extract(root_archive, qa / "root-package")
        root_destination = vendor / root_vendor.name
        shutil.move(root_vendor, root_destination)
        checksum_directory(root_destination, package_checksum)
        config = cargo_config(qa, vendor)

        for package in ("flyrail-example-notes", "flyrail-example-checksum"):
            run([*common, "--config", str(config), "-p", package])

        notes_archive = target / "package" / "flyrail-example-notes-0.4.0.crate"
        checksum_archive = target / "package" / "flyrail-example-checksum-2.1.0.crate"
        notes = extract(notes_archive, extracted / "notes")
        checksum = extract(checksum_archive, extracted / "checksum")

        for manifest in (notes / "Cargo.toml", checksum / "Cargo.toml"):
            run(
                [
                    "cargo",
                    "build",
                    "--offline",
                    "--config",
                    str(config),
                    "--manifest-path",
                    str(manifest),
                    "--target-dir",
                    str(target / "extracted-build"),
                ],
                cwd=qa,
            )

        binaries = target / "extracted-build" / "debug"
        notes_bin = binaries / "flyrail-notes"
        checksum_bin = binaries / "flyrail-checksum"
        manifest = notes / "bundle/flyrail.json"
        words = run([str(notes_bin), "words", str(manifest)], cwd=qa)
        assert words.stdout.strip() == str(len(manifest.read_text().split())).encode()

        notes_target = qa / "notes-target"
        notes_state = qa / "notes-state"
        run(
            [
                str(notes_bin),
                "ai",
                "install",
                "--source",
                str(notes / "bundle"),
                "--target",
                str(notes_target),
                "--state",
                str(notes_state),
            ],
            cwd=qa,
        )
        shutil.rmtree(notes / "bundle")
        run(
            [
                str(notes_bin),
                "ai",
                "uninstall",
                "--bundle-id",
                "example-notes",
                "--target",
                str(notes_target),
                "--state",
                str(notes_state),
            ],
            cwd=qa,
        )

        checksum_target = qa / "checksum-target"
        checksum_state = ["--state", str(qa / "checksum-state")]
        run(
            [str(checksum_bin), "ai", "install", "--target", str(checksum_target)]
            + checksum_state,
            cwd=qa,
        )
        sample = checksum_target / "checksum-review/assets/sample.bin"
        script = checksum_target / "checksum-review/scripts/verify_checksum.py"
        assert sample.read_bytes() == b"\x00\x01\x02\n\r\x7f\x80\xfe\xffFLYRAIL\x00"
        assert script.read_bytes() == (
            checksum / "resources/flyrail/skills/checksum-review/scripts/verify_checksum.py"
        ).read_bytes()
        if os.name != "nt":
            assert script.stat().st_mode & 0o111
        shutil.rmtree(checksum / "resources")
        run(
            [str(checksum_bin), "ai", "uninstall", "--target", str(checksum_target)]
            + checksum_state,
            cwd=qa,
        )
        print("Extracted package QA passed for flyrail and both example crate archives.")
    finally:
        shutil.rmtree(qa, ignore_errors=True)
        assert not qa.exists()


if __name__ == "__main__":
    main()
