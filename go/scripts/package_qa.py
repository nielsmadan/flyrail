"""Check the Go module as a consumer sees it: a copy of its files outside the repository."""

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODULE = "github.com/nielsmadan/flyrail/go"
RUNTIME = ROOT / "python/src/flyrail/runtime"
SAMPLE = b"\x00\x01\x02\n\r\x7f\x80\xfe\xffFLYRAIL\x00"

CONSUMER_MAIN = r'''package main

import (
	"context"
	"embed"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

//go:embed all:bundle
var embedded embed.FS

const lockTimeout = 5 * time.Second

type report struct {
	Status   string            `json:"status"`
	State    string            `json:"state"`
	Current  bool              `json:"current"`
	Entries  []string          `json:"entries"`
	Runtime  map[string]string `json:"runtime"`
	BundleID string            `json:"bundle_id"`
}

func main() {
	if err := run(context.Background(), os.Args[1], os.Args[2]); err != nil {
		fmt.Fprintln(os.Stderr, "consumer:", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, action, root string) error {
	st, err := store.NewDirectoryStore(ctx, filepath.Join(root, "state"))
	if err != nil {
		return err
	}
	target, err := flyrail.DirectoryTarget(filepath.Join(root, "skills"))
	if err != nil {
		return err
	}
	targets := []flyrail.Target{target}
	out := report{Entries: []string{}, Runtime: map[string]string{}}
	switch action {
	case "install":
		source, err := fs.Sub(embedded, "bundle")
		if err != nil {
			return err
		}
		bundle, err := flyrail.LoadBundleFS(source)
		if err != nil {
			return err
		}
		results, err := flyrail.Install(ctx, bundle, targets, st, flyrail.LockOptions{LockTimeout: lockTimeout})
		if err != nil {
			return err
		}
		if err := results[0].Failure(); err != nil {
			return err
		}
		inspected, err := flyrail.Inspect(ctx, bundle, targets, st)
		if err != nil {
			return err
		}
		observation := inspected[0].Observation
		out.Status, out.State, out.Current, out.BundleID = string(results[0].Status), string(observation.State), observation.IsCurrent(), bundle.ID()
		if observation.Installed != nil {
			for _, entry := range observation.Installed.Entries {
				out.Entries = append(out.Entries, entry.Path)
			}
		}
		runtime, err := flyrail.NewHookRuntime("", "")
		if err != nil {
			return err
		}
		for _, resource := range runtime.Resources() {
			out.Runtime[resource.Path] = hex.EncodeToString(resource.Data)
		}
	case "uninstall":
		results, err := flyrail.Uninstall(ctx, "package-probe", targets, st, flyrail.SkillOptions{LockTimeout: lockTimeout})
		if err != nil {
			return err
		}
		if err := results[0].Failure(); err != nil {
			return err
		}
		out.Status, out.State = string(results[0].Status), string(results[0].Observation.State)
	default:
		return fmt.Errorf("unknown action %q", action)
	}
	return json.NewEncoder(os.Stdout).Encode(out)
}
'''

CONSUMER_MANIFEST = '{"schema_version": 1, "id": "package-probe", "version": "1", "skills": [{"name": "probe", "path": "skills/probe"}]}\n'
CONSUMER_SKILL = "---\nname: probe\ndescription: Probe the packaged Go module.\n---\nReport that the probe ran.\n"


def go_environment(offline=True):
    cache = ROOT / ".cache/go"
    environment = {key: value for key, value in os.environ.items() if not key.startswith("FLYRAIL_")}
    environment.setdefault("GOTOOLCHAIN", "go1.27.3+auto")
    environment.setdefault("GOPATH", str(cache))
    environment.setdefault("GOMODCACHE", str(cache / "mod"))
    environment.setdefault("GOCACHE", str(cache / "build"))
    return environment | {"GOFLAGS": "-modcacherw", "GOWORK": "off"} | ({"GOPROXY": "off"} if offline else {})


def run(arguments, cwd, environment=None, timeout=900):
    result = subprocess.run(arguments, cwd=cwd, env=environment or go_environment(),
                            capture_output=True, text=True, timeout=timeout, check=False)
    if result.returncode:
        raise RuntimeError(f"{arguments}: exit {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def module_files():
    tracked = run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "go"], ROOT, os.environ.copy())
    files = sorted({Path(name) for name in tracked.split("\0") if name and (ROOT / name).exists()})
    assert all((ROOT / path).is_file() and not (ROOT / path).is_symlink() for path in files)
    assert Path("go/go.mod") in files and Path("go/go.sum") in files
    return files


def copy_module(files, destination):
    for path in files:
        (destination / path).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / path, destination / path)
    return destination / "go"


def check_runtime(module):
    embedded = sorted((module / "internal/hookruntime").glob("*.mts"))
    assert [path.name for path in embedded] == ["bridge.mts", "process.mts", "runner.mts"], embedded
    for path in embedded:
        assert path.read_bytes() == (RUNTIME / path.name).read_bytes(), f"{path.name} differs from Python's runtime"


def check_consumer(qa, module):
    consumer = qa / "consumer"
    (consumer / "bundle/skills/probe").mkdir(parents=True)
    (consumer / "bundle/flyrail.json").write_text(CONSUMER_MANIFEST)
    (consumer / "bundle/skills/probe/SKILL.md").write_text(CONSUMER_SKILL)
    (consumer / "main.go").write_text(CONSUMER_MAIN)
    (consumer / "go.mod").write_text(
        f"module example.com/flyrail-consumer\n\ngo 1.27\n\nrequire {MODULE} v0.0.0\n\n"
        f"replace {MODULE} => {module}\n"
    )
    shutil.copy2(module / "go.sum", consumer / "go.sum")
    run(["go", "mod", "tidy"], consumer)
    binary = qa / "bin/consumer"
    run(["go", "build", "-o", str(binary), "."], consumer, go_environment() | {"CGO_ENABLED": "0"})
    target = qa / "consumer-target"
    installed = json.loads(run([str(binary), "install", str(target)], qa))
    assert installed["status"] == "applied" and installed["state"] == "installed" and installed["current"], installed
    assert installed["entries"] == ["probe", "probe/SKILL.md"], installed
    assert installed["runtime"] == {path.name: path.read_bytes().hex() for path in RUNTIME.glob("*.mts")}, \
        "the consumer's embedded hook runtime differs from Python's"
    shutil.rmtree(consumer / "bundle")
    removed = json.loads(run([str(binary), "uninstall", str(target)], qa))
    assert removed["status"] == "applied", removed
    assert not (target / "skills").exists()


def check_examples(qa, module):
    binaries = qa / "bin"
    for name in ("flyrail-notes", "flyrail-checksum"):
        run(["go", "build", "-o", str(binaries / name), f"./examples/{name}"], module,
            go_environment() | {"CGO_ENABLED": "0"})
    notes = str(binaries / "flyrail-notes")
    manifest = module / "examples/flyrail-notes/bundle/flyrail.json"
    assert run([notes, "words", str(manifest)], qa).strip() == str(len(manifest.read_text().split()))
    source = qa / "notes-source"
    shutil.copytree(module / "examples/flyrail-notes/bundle", source)
    targets = ["--target", str(qa / "notes-target"), "--state", str(qa / "notes-state")]
    installed = json.loads(run([notes, "ai", "install", "--source", str(source), *targets], qa))
    assert installed["results"][0]["status"] == "applied", installed
    assert (qa / "notes-target/notes-summary/SKILL.md").is_file()
    shutil.rmtree(source)
    removed = json.loads(run([notes, "ai", "uninstall", "--bundle-id", "example-notes", *targets], qa))
    assert removed["results"][0]["status"] == "applied" and removed["bundle_version"] is None, removed
    assert not (qa / "notes-target").exists()

    checksum = str(binaries / "flyrail-checksum")
    target = qa / "checksum-target"
    targets = ["--target", str(target), "--state", str(qa / "checksum-state")]
    installed = json.loads(run([checksum, "ai", "install", *targets], qa))
    assert installed["results"][0]["status"] == "applied", installed
    resources = module / "examples/flyrail-checksum/resources/flyrail/skills/checksum-review"
    script = target / "checksum-review/scripts/verify_checksum.py"
    assert (target / "checksum-review/assets/sample.bin").read_bytes() == SAMPLE
    assert script.read_bytes() == (resources / "scripts/verify_checksum.py").read_bytes()
    assert script.stat().st_mode & 0o111
    shutil.rmtree(module / "examples/flyrail-checksum/resources")
    removed = json.loads(run([checksum, "ai", "uninstall", *targets], qa))
    assert removed["results"][0]["status"] == "applied", removed
    assert not target.exists()


def main():
    files = module_files()
    qa = Path(tempfile.mkdtemp(prefix="flyrail-go-package-")).resolve()
    try:
        assert not qa.is_relative_to(ROOT), "package QA must run outside the repository"
        module = copy_module(files, qa / "copy")
        check_runtime(module)
        run(["go", "mod", "download"], module, go_environment(offline=False))
        run(["go", "vet", "./..."], module)
        run(["go", "build", "./..."], module)
        run(["go", "test", "-short", "-count=1", "./..."], module)
        check_consumer(qa, module)
        check_examples(qa, module)
    finally:
        shutil.rmtree(qa, ignore_errors=True)
    assert not qa.exists(), "package QA temporary files were retained"
    print(f"Go package QA: {len(files)} module files copied outside the repository; vet, build and short tests "
          "passed there; an external consumer module installed, inspected and removed an embedded bundle "
          "source-free; both example CLIs built without cgo and uninstalled after their sources were deleted")


if __name__ == "__main__":
    main()
