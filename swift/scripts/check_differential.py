import shutil
import sys
import tempfile
from collections import Counter
from pathlib import Path

from check_corpus_limits import check_corpus_limits
from harness import ENV, PYTHON, ROOT, RUST, SWIFT, build_rust_examples, run, use_python_paths

CATEGORIES = {"value", "bundle", "render", "edit", "lifecycle"}
BOUNDARY_FIELDS = {
    "desired-generation", "stale-preview", "partial-inspect",
    "source-free-recover-result", "partial-sync-result", "source-free-remove-result",
}


INDEX_KEY = "index/team/37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570"
LIFECYCLE_PHASES = ["1-create", "2-unchanged", "3-update", "4-recover", "6-remove"]
LIFECYCLE_FILES = {"file": "owned", "alias": "owned", "casefold": "Straße", "toml": "config.toml",
                   "json": "config.json"}


def required_keys(rows):
    keys = set()
    for category, name, *columns in rows:
        fields = []
        if category == "value":
            fields = ["semantic"]
        elif category == "bundle":
            fields = ["digest", "canonical"]
        elif category == "render":
            fields = ["digest", "canonical", "context", "claim-0", "claim-1"]
        elif category == "edit":
            count = int(columns[2])
            operations = columns[3:]
            assert len(operations) == count * 3
            for step in range(count):
                fields += [f"{step + 1}-{field}" for field in
                           ["bytes", "status", "provenance", "ownership-count"]]
                if operations[step * 3] == "set":
                    fields.append(f"{step + 1}-ownership-0")
                else:
                    assert operations[step * 3] == "retire"
        elif category == "lifecycle":
            filename = LIFECYCLE_FILES.get(columns[0], "AGENTS.md")
            for phase in LIFECYCLE_PHASES:
                fields += [f"{phase}-{field}" for field in [
                    "result", "payloads", f"state-{INDEX_KEY}", f"state-resource/$ROOT/{filename}/authority",
                ]]
            fields.append("5-inspect")
        else:
            raise AssertionError(category)
        keys.update((category, name, field) for field in fields)
    return keys


def state_key(key):
    category, _, field = key
    return category == "lifecycle" and any(field.startswith(f"{phase}-state-") for phase in LIFECYCLE_PHASES)


def records(output):
    result = {}
    for line in output.decode().splitlines():
        category, name, field, raw = line.split("\t")
        key = category, name, field
        assert key not in result, key
        result[key] = bytes.fromhex(raw)
    return result


def boundary_line(line):
    return line.startswith(b"lifecycle\tpublic-boundaries\t")


def split_boundaries(output):
    lines = output.splitlines(keepends=True)
    return (b"".join(line for line in lines if not boundary_line(line)),
            b"".join(line for line in lines if boundary_line(line)))


def check_boundaries(swift, work):
    reference = run([sys.executable, "-B", str(ROOT / "swift/scripts/lifecycle_reference.py"), work])
    expected = records(reference)
    assert {field for _, _, field in expected} >= BOUNDARY_FIELDS, expected.keys()
    actual = records(swift)
    assert actual.keys() == expected.keys(), actual.keys() ^ expected.keys()
    assert actual == expected, [
        (key, expected[key], actual[key]) for key in expected if expected[key] != actual[key]
    ][:3]
    assert swift == reference, "public-boundaries record ordering differs"
    return len(expected)


def check_state_vectors(root):
    from interop_vectors import fixture_records, prepared

    vectors = fixture_records()
    tags = Counter(tag for _, _, tag in vectors)
    assert tags.keys() == {"Receipt", "Journal", "Preparation"} and all(tags.values()), tags
    for name, raw, tag in vectors:
        vector = prepared(name, raw, root)
        for driver in [PYTHON, RUST, SWIFT]:
            encoded = run(driver, "encode", *(["--type", tag] if driver == RUST else []), input=vector)
            assert encoded == vector, (driver, name)
    return len(vectors)


def main():
    use_python_paths("python/tests")
    build_rust_examples("differential", "interop")
    corpus = ROOT / "spec/fixtures/differential.tsv"
    rows = [line.split("\t") for line in corpus.read_text().splitlines()
            if line and not line.startswith("#")]
    categories = Counter(row[0] for row in rows)
    assert categories.keys() == CATEGORIES, categories
    run([sys.executable, "-B", str(ROOT / "python/scripts/generate_differential_corpus.py")], "--check")
    base = ROOT / ".cache/swift-differential"
    base.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=base) as temporary:
        root = Path(temporary)
        check_corpus_limits(root, SWIFT, ENV)
        work = root / "work"
        drivers = [
            [sys.executable, "-B", str(ROOT / "python/scripts/differential_driver.py")],
            [str(ROOT / "rust/target/debug/examples/differential")],
            SWIFT + ["differential"],
        ]
        outputs = []
        for driver in drivers:
            output = run(driver, "--corpus", corpus, "--root", work, timeout=300)
            if driver[0] == SWIFT[0]:
                output, boundaries = split_boundaries(output)
                shutil.rmtree(work)
                boundary_count = check_boundaries(boundaries, work.resolve())
            outputs.append(output)
            shutil.rmtree(work)
        expected = records(outputs[0])
        required = required_keys(rows)
        assert expected.keys() >= required, required - expected.keys()
        assert all(map(state_key, expected.keys() - required)), expected.keys() - required
        for output in outputs[1:]:
            actual = records(output)
            assert actual.keys() == expected.keys(), actual.keys() ^ expected.keys()
            assert actual == expected, [
                (key, expected[key], actual[key]) for key in expected if expected[key] != actual[key]
            ][:3]
        assert outputs[0] == outputs[1] == outputs[2], "differential record ordering differs"
        vector_count = check_state_vectors(root)
    print(f"Differential: {len(expected)} exact records across {len(rows)} cases/{len(categories)} categories/"
          f"3 languages; {boundary_count} Swift public-boundary records match the Python reference; "
          f"{vector_count} state vectors encode identically in all 3 languages and cross-decode in all "
          "6 directions passed")


if __name__ == "__main__":
    main()
