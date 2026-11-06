"""Compare the Go differential driver with Python's (and Rust's when it builds) for the categories Go implements."""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "swift/scripts"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from check_corpus_limits import check_corpus_limits
from editing_corpus import editing_corpus
from harness import ENV, build_go_driver, build_rust_examples, run

CORPUS = ROOT / "spec/fixtures/differential.tsv"
CATEGORIES_FILE = Path(__file__).with_name("differential_categories.txt")
PYTHON_DRIVER = [sys.executable, "-B", str(ROOT / "python/scripts/differential_driver.py")]
PYTHON_EDITING = [sys.executable, "-B", str(ROOT / "swift/scripts/reference_value_driver.py"), "--edits"]
RUST_DRIVER = [str(ROOT / "rust/target/debug/examples/differential")]


def listed_categories():
    lines = CATEGORIES_FILE.read_text(encoding="utf-8").splitlines()
    return {line.strip() for line in lines if line.strip() and not line.startswith("#")}


def corpus_categories():
    lines = CORPUS.read_text(encoding="utf-8").splitlines()
    return Counter(line.split("\t")[0] for line in lines if line and not line.startswith("#"))


def output_lines(output):
    return re.findall(rb"[^\n]*\n|[^\n]+", output)


def selected_output(output, categories):
    return b"".join(
        line
        for line in output_lines(output)
        if line.split(b"\t")[0].decode() in categories
    )


def records(output):
    result = {}
    for line in output_lines(output):
        category, name, field, raw = line.rstrip(b"\n").decode().split("\t")
        key = category, name, field
        assert key not in result, f"duplicate record {key}"
        result[key] = bytes.fromhex(raw)
    return result


def rust_driver():
    if shutil.which("cargo", path=ENV.get("PATH")) is None:
        assert os.environ.get("CI") != "true", "cargo not found on PATH, but CI requires comparing Go with Rust"
        print("cargo not found on PATH, comparing Go with Python only", file=sys.stderr)
        return None
    build_rust_examples("differential")
    return RUST_DRIVER


def driver_output(driver, work):
    try:
        return run(driver, "--corpus", CORPUS, "--root", work, timeout=300)
    finally:
        shutil.rmtree(work, ignore_errors=True)


def compare(name, expected_output, actual_output):
    expected, actual = records(expected_output), records(actual_output)
    assert actual.keys() == expected.keys(), (name, sorted(actual.keys() ^ expected.keys())[:5])
    mismatches = [(key, expected[key].hex(), actual[key].hex()) for key in expected if expected[key] != actual[key]]
    assert not mismatches, (name, mismatches[:3])
    assert actual_output == expected_output, f"{name}: differential output bytes differ"


def check_editing_corpus(go, root):
    corpus = root / "editing-corpus.json"
    cases = editing_corpus()
    corpus.write_text(json.dumps(cases, ensure_ascii=False, sort_keys=True), encoding="utf-8")
    expected = subprocess.run(
        PYTHON_EDITING + [str(corpus)], capture_output=True, env=ENV | {"PYTHONIOENCODING": "utf-8"},
        cwd=ROOT, timeout=600, check=True,
    ).stdout
    actual = run(go, "edits", "--corpus", corpus, timeout=600)
    expected_lines, actual_lines = output_lines(expected), output_lines(actual)
    assert len(actual_lines) == len(expected_lines), ("editing corpus", len(expected_lines), len(actual_lines))
    mismatches = [line.split(b"\t", 2)[:2] for line, other in zip(expected_lines, actual_lines) if line != other]
    assert not mismatches, ("editing corpus records differ", mismatches[:5])
    assert actual == expected, "editing corpus output bytes differ"
    return len(cases), len(expected_lines)


def main():
    listed = listed_categories()
    available = corpus_categories()
    assert listed, f"{CATEGORIES_FILE.name} lists no categories"
    assert listed <= available.keys(), f"listed categories missing from the corpus: {sorted(listed - available.keys())}"
    go = build_go_driver()
    rust = rust_driver()
    base = ROOT / ".cache/go-differential"
    base.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=base) as temporary:
        root = Path(temporary)
        if sys.platform == "darwin":
            check_corpus_limits(root, go, ENV)
        work = root / "work"
        go_output = driver_output(go + ["differential"], work)
        emitted = {line.split(b"\t")[0].decode() for line in output_lines(go_output)}
        assert emitted == listed, (
            f"Go emitted categories {sorted(emitted)} but {CATEGORIES_FILE.name} lists {sorted(listed)}"
        )
        editing_cases, editing_records = check_editing_corpus(go, root)
        expected = selected_output(driver_output(PYTHON_DRIVER, work), listed)
        compare("python/go", expected, go_output)
        languages = ["Python", "Go"]
        if rust is not None:
            compare("python/rust", expected, selected_output(driver_output(rust, work), listed))
            languages.insert(1, "Rust")
    cases = sum(available[category] for category in listed)
    print(f"Go differential: {len(records(expected))} exact records across {cases} cases in {len(listed)} of "
          f"{len(available)} categories ({', '.join(sorted(listed))}) match {'/'.join(languages)}; "
          f"editing corpus: {editing_records} records across {editing_cases} cases match Python")


if __name__ == "__main__":
    main()
