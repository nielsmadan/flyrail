#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(root: &Path) -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = root.join(".cache/tmp").join(format!(
            "differential-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn python(root: &Path) -> PathBuf {
    if std::env::var("FLYRAIL_SKIP_DIFFERENTIAL").as_deref() == Ok("1") {
        eprintln!("differential parity explicitly disabled by FLYRAIL_SKIP_DIFFERENTIAL");
        return PathBuf::new();
    }
    let candidate = std::env::var_os("FLYRAIL_DIFFERENTIAL_PYTHON")
        .map_or_else(|| root.join("python/.venv/bin/python"), PathBuf::from);
    let output = Command::new(&candidate)
        .args(["-c", "import flyrail, tomlkit"])
        .current_dir(root.join("python"))
        .output()
        .expect("differential Python preflight must start");
    assert!(
        output.status.success(),
        "differential Python preflight failed for {}:\n{}",
        candidate.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    candidate
}

fn build_driver(root: &Path) -> PathBuf {
    let binary = root.join("rust/target/debug/examples/differential");
    let status = Command::new("cargo")
        .args(["build", "--example", "differential"])
        .current_dir(root.join("rust"))
        .env("CARGO_HOME", root.join(".cache/cargo-home"))
        .status()
        .expect("cargo must spawn");
    assert!(status.success(), "differential example build failed");
    binary
}

fn run(command: &mut Command, label: &str) -> Output {
    let output = command.output().expect("differential command must spawn");
    assert!(
        output.status.success(),
        "{label} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn records(output: &[u8], label: &str) -> BTreeMap<(String, String, String), Vec<u8>> {
    let text = std::str::from_utf8(output).expect("differential output must be UTF-8");
    let mut records = BTreeMap::new();
    for line in text.lines() {
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 4, "{label}: invalid record: {line}");
        let key = (
            columns[0].to_owned(),
            columns[1].to_owned(),
            columns[2].to_owned(),
        );
        let value = hex_bytes(columns[3]);
        assert!(
            records.insert(key.clone(), value).is_none(),
            "{label}: duplicate record key: {key:?}"
        );
    }
    records
}

fn hex_bytes(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2), "odd-length output hex");
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("output hex is UTF-8");
            u8::from_str_radix(text, 16).expect("output bytes are hexadecimal")
        })
        .collect()
}

fn case_counts(records: &BTreeMap<(String, String, String), Vec<u8>>) -> BTreeMap<&str, usize> {
    ["value", "bundle", "render", "edit", "lifecycle"]
        .into_iter()
        .map(|category| {
            let cases = records
                .keys()
                .filter(|(actual, _, _)| actual == category)
                .map(|(_, case, _)| case)
                .collect::<BTreeSet<_>>();
            (category, cases.len())
        })
        .collect()
}

fn record_key(category: &str, case: &str, field: impl Into<String>) -> (String, String, String) {
    (category.to_owned(), case.to_owned(), field.into())
}

fn lifecycle_destination(case_root: &Path, kind: &str) -> PathBuf {
    case_root.join(match kind {
        "file" | "alias" => "owned",
        "casefold" => "Straße",
        "toml" => "config.toml",
        "json" => "config.json",
        _ => "AGENTS.md",
    })
}

fn expected_record_keys(
    rows: &[Vec<&str>],
    work: &Path,
) -> Result<BTreeSet<(String, String, String)>, String> {
    let work = fs::canonicalize(work).map_err(|error| error.to_string())?;
    let mut expected = BTreeSet::new();
    for row in rows {
        let category = row[0];
        let case = row[1];
        match category {
            "value" => {
                expected.insert(record_key(category, case, "semantic"));
            }
            "bundle" => {
                for field in ["digest", "canonical"] {
                    expected.insert(record_key(category, case, field));
                }
            }
            "render" => {
                for field in ["digest", "canonical", "context", "claim-0", "claim-1"] {
                    expected.insert(record_key(category, case, field));
                }
            }
            "edit" => {
                let count = row[4].parse::<usize>().expect("edit count must be valid");
                assert_eq!(row.len(), 5 + count * 3, "{case}: invalid edit row");
                for (index, operation) in row[5..].as_chunks::<3>().0.iter().enumerate() {
                    let step = index + 1;
                    for suffix in ["bytes", "status", "provenance", "ownership-count"] {
                        expected.insert(record_key(category, case, format!("{step}-{suffix}")));
                    }
                    let owns_selection = match operation[0] {
                        "set" => true,
                        "retire" => false,
                        other => return Err(format!("{case}: invalid edit operation {other}")),
                    };
                    if owns_selection {
                        expected.insert(record_key(category, case, format!("{step}-ownership-0")));
                    }
                }
            }
            "lifecycle" => {
                let case_root = work.join(case);
                let authority =
                    flyrail::ResourceAuthority::new(lifecycle_destination(&case_root, row[2]))
                        .expect("lifecycle destination must be valid");
                let state_root = authority
                    .state_root()
                    .strip_prefix(&case_root)
                    .expect("lifecycle state root must belong to case root")
                    .to_string_lossy();
                for phase in [
                    "1-create",
                    "2-unchanged",
                    "3-update",
                    "4-recover",
                    "6-remove",
                ] {
                    for field in [
                        format!("{phase}-result"),
                        format!("{phase}-state-$ROOT/{state_root}/receipt.json"),
                        format!("{phase}-state-$ROOT/index/index.json"),
                        format!("{phase}-payloads"),
                    ] {
                        expected.insert(record_key(category, case, field));
                    }
                }
                expected.insert(record_key(category, case, "5-inspect"));
            }
            other => return Err(format!("unknown differential category {other}")),
        }
    }
    Ok(expected)
}

#[test]
fn generated_python_and_rust_corpus_is_byte_identical() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let python = python(&root);
    if python.as_os_str().is_empty() {
        return;
    }
    let corpus = root.join("spec/fixtures/differential.tsv");
    let corpus_text = fs::read_to_string(&corpus).expect("corpus must be readable");
    let corpus_rows = corpus_text
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let generator = root.join("python/scripts/generate_differential_corpus.py");
    let python_driver = root.join("python/scripts/differential_driver.py");
    let rust_driver = build_driver(&root);
    let temporary = TempDir::new(&root).expect("temporary root must be created");
    let work = temporary.0.join("work");

    run(
        Command::new(&python).arg(&generator).arg("--check"),
        "corpus freshness check",
    );
    let python_output = run(
        Command::new(&python)
            .arg(&python_driver)
            .args(["--corpus", corpus.to_str().expect("corpus path is UTF-8")])
            .args(["--root", work.to_str().expect("work path is UTF-8")]),
        "Python differential driver",
    );
    fs::remove_dir_all(&work).expect("Python differential root must be removable");
    let rust_output = run(
        Command::new(&rust_driver)
            .args(["--corpus", corpus.to_str().expect("corpus path is UTF-8")])
            .args(["--root", work.to_str().expect("work path is UTF-8")]),
        "Rust differential driver",
    );

    let python_records = records(&python_output.stdout, "Python output");
    let rust_records = records(&rust_output.stdout, "Rust output");
    let expected_keys =
        expected_record_keys(&corpus_rows, &work).expect("expected keys must be derivable");
    let python_keys = python_records.keys().cloned().collect::<BTreeSet<_>>();
    let rust_keys = rust_records.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(rust_output.stdout, python_output.stdout);
    assert_eq!(python_keys, expected_keys);
    assert_eq!(rust_keys, expected_keys);
    assert_eq!(python_records.len(), 437);
    assert_eq!(
        case_counts(&python_records),
        BTreeMap::from([
            ("bundle", 12),
            ("edit", 8),
            ("lifecycle", 9),
            ("render", 8),
            ("value", 28),
        ])
    );
    assert_eq!(rust_records, python_records);
}

#[test]
fn record_parser_rejects_duplicate_keys_and_preserves_distinct_keys() {
    let duplicate = b"value\tcase\tfield\t00\nvalue\tcase\tfield\t01\n";
    let failure = std::panic::catch_unwind(|| records(duplicate, "duplicate"));
    assert!(failure.is_err());

    let distinct = records(
        b"value\tcase\tfirst\t00\nvalue\tcase\tsecond\t01\n",
        "distinct",
    );
    assert_eq!(distinct.len(), 2);
    assert_eq!(
        distinct.get(&(
            String::from("value"),
            String::from("case"),
            String::from("first")
        )),
        Some(&vec![0])
    );
}

#[test]
fn corpus_covers_structures_orders_and_operation_shapes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let corpus =
        fs::read_to_string(root.join("spec/fixtures/differential.tsv")).expect("corpus must exist");
    let rows = corpus
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .collect::<Vec<_>>();

    let bundle_rows = rows.iter().filter(|row| row[0] == "bundle");
    let schemas = bundle_rows
        .clone()
        .map(|row| row[2])
        .collect::<BTreeSet<_>>();
    let structured_bundles = bundle_rows
        .filter(|row| {
            if row[2] != "2" {
                return false;
            }
            let manifest: serde_json::Value =
                serde_json::from_slice(&hex_bytes(row[3])).expect("manifest must be JSON");
            let artifacts = manifest["artifacts"]
                .as_array()
                .expect("schema-2 artifacts must be an array");
            let assets = manifest["assets"]
                .as_array()
                .expect("schema-2 assets must be an array");
            let dependencies = manifest["dependencies"]
                .as_array()
                .expect("schema-2 dependencies must be an array");
            assert!(
                !artifacts.is_empty(),
                "schema-2 artifacts must not be empty"
            );
            assert!(!assets.is_empty(), "schema-2 assets must not be empty");
            assert!(
                !dependencies.is_empty(),
                "schema-2 dependencies must not be empty"
            );
            let artifact_ids = artifacts
                .iter()
                .map(|item| item["id"].as_str().expect("artifact id must be a string"))
                .collect::<BTreeSet<_>>();
            let asset_ids = assets
                .iter()
                .map(|item| item["id"].as_str().expect("asset id must be a string"))
                .collect::<BTreeSet<_>>();
            assert!(dependencies.iter().all(|item| {
                item["dependent"]
                    .as_str()
                    .is_some_and(|id| artifact_ids.contains(id))
                    && item["required"]
                        .as_str()
                        .is_some_and(|id| asset_ids.contains(id))
            }));
            true
        })
        .count();
    assert_eq!(schemas, BTreeSet::from(["1", "2"]));
    assert!(structured_bundles >= 4);

    let render_rows = rows.iter().filter(|row| row[0] == "render");
    assert_eq!(
        render_rows
            .clone()
            .map(|row| row[5])
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["file-first", "section-first"])
    );
    assert_eq!(
        render_rows.map(|row| row[6]).collect::<BTreeSet<_>>(),
        BTreeSet::from(["alpha-first", "zeta-first"])
    );

    let edit_rows = rows.iter().filter(|row| row[0] == "edit");
    let shapes = edit_rows
        .clone()
        .map(|row| {
            row[5..]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|operation| operation[0])
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    let acquisitions = edit_rows
        .flat_map(|row| row[5..].as_chunks::<3>().0)
        .filter(|operation| operation[0] == "set")
        .map(|operation| operation[2])
        .collect::<BTreeSet<_>>();
    assert!(shapes.len() >= 3);
    assert!(
        shapes
            .iter()
            .any(|shape| shape.iter().filter(|op| **op == "retire").count() > 1)
    );
    assert_eq!(acquisitions, BTreeSet::from(["conflict", "takeover"]));
}
