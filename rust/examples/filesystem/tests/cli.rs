#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl Deref for TempRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        if self.0.exists() {
            fs::remove_dir_all(&self.0).expect("remove test root");
        }
    }
}

#[test]
fn temp_root_cleanup_runs_on_drop() {
    let path = {
        let root = root("drop");
        let path = root.to_path_buf();
        assert!(path.exists());
        path
    };
    assert!(!path.exists());
}

#[test]
fn value_options_reject_following_options_without_writes() {
    let root = root("missing-values");
    for option in ["--source", "--bundle-id", "--target", "--state"] {
        let output = run_os(&[
            OsString::from("ai"),
            OsString::from(if option == "--bundle-id" {
                "uninstall"
            } else {
                "install"
            }),
            OsString::from(option),
            OsString::from("--replace-modified"),
        ]);
        assert_eq!(output.status.code(), Some(2), "{option}: {output:?}");
        assert!(output.stdout.is_empty(), "{option}");
        assert!(!output.stderr.is_empty(), "{option}");
    }
    assert_eq!(fs::read_dir(&*root).expect("root empty").count(), 0);
}

fn root(name: &str) -> TempRoot {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(".cache/tmp")
        .join(format!("flyrail-notes-{name}-{nonce}"));
    fs::create_dir_all(&root).expect("create temp root");
    TempRoot(fs::canonicalize(root).expect("canonical temp root"))
}

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flyrail-notes"))
        .args(arguments)
        .output()
        .expect("run notes")
}

fn run_os(arguments: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flyrail-notes"))
        .args(arguments)
        .output()
        .expect("run notes")
}

fn report(arguments: &[&str], code: i32) -> Value {
    let output = run(arguments);
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    serde_json::from_slice(&output.stdout).expect("JSON report")
}

fn source() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/bundle")
}

#[test]
fn version_words_and_usage_contract() {
    let root = root("host");
    let notes = root.join("notes.txt");
    fs::write(&notes, "one two\nthree\n").expect("write notes");
    let output = run(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "flyrail-notes 0.4.0"
    );
    let output = run_os(&[OsString::from("words"), notes.into_os_string()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");
    let output = run_os(&[
        OsString::from("words"),
        root.join("missing").into_os_string(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_ne!(output.stderr, [] as [u8; 0]);
    assert_eq!(run(&["ai", "install"]).status.code(), Some(2));
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_work_and_non_utf8_text_is_rejected_without_panic() {
    use std::os::unix::ffi::OsStringExt as _;

    let root = root("non-utf8");
    let file = root.join(OsString::from_vec(b"notes-\xff.txt".to_vec()));
    if let Err(error) = fs::write(&file, "one two") {
        if error.kind() == std::io::ErrorKind::PermissionDenied && std::env::var_os("CI").is_none()
        {
            eprintln!("SKIP: active sandbox rejects non-UTF-8 filesystem names");
            return;
        }
        panic!("write non-UTF-8 path: {error}");
    }
    let output = run_os(&[OsString::from("words"), file.into_os_string()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"2\n");

    let missing = root.join(OsString::from_vec(b"missing-\xff".to_vec()));
    let output = run_os(&[OsString::from("words"), missing.into_os_string()]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_ne!(output.stderr, [] as [u8; 0]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));

    let output = run_os(&[
        OsString::from("ai"),
        OsString::from("uninstall"),
        OsString::from("--bundle-id"),
        OsString::from_vec(b"bad-\xff".to_vec()),
        OsString::from("--target"),
        root.as_os_str().to_owned(),
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn lifecycle_supports_repeat_targets_modifications_and_source_free_removal() {
    let root = root("lifecycle");
    let copied = root.join("source");
    copy_tree(Path::new(source()), &copied);
    let first = root.join("first");
    let second = root.join("second");
    let state = root.join("state");
    let targets = [
        "--target",
        first.to_str().expect("UTF-8"),
        "--target",
        second.to_str().expect("UTF-8"),
        "--state",
        state.to_str().expect("UTF-8"),
    ];
    let mut content = vec!["ai", "status", "--source", copied.to_str().expect("UTF-8")];
    content.extend(targets);
    let status = report(&content, 1);
    assert_eq!(status["results"][0]["observation"]["state"], "absent");
    assert_eq!(status["results"][0]["target"]["scope"], "directory");
    content[1] = "install";
    let installed = report(&content, 0);
    assert_eq!(installed["results"][0]["status"], "applied");
    assert_eq!(installed["results"][1]["status"], "applied");
    assert_eq!(report(&content, 0)["results"][0]["status"], "unchanged");
    let competing = root.join("competing");
    copy_tree(Path::new(source()), &competing);
    let manifest = competing.join("flyrail.json");
    let manifest_text = fs::read_to_string(&manifest)
        .expect("read competing manifest")
        .replace("\"example-notes\"", "\"competing-notes\"");
    fs::write(manifest, manifest_text).expect("write competing manifest");
    let before = fs::read(first.join("notes-summary/SKILL.md")).expect("installed skill");
    let conflict_status = report(
        &[
            "ai",
            "status",
            "--source",
            competing.to_str().expect("UTF-8"),
            "--target",
            first.to_str().expect("UTF-8"),
            "--state",
            state.to_str().expect("UTF-8"),
        ],
        1,
    );
    assert_eq!(
        conflict_status["results"][0]["observation"]["conflicts"],
        serde_json::json!([{"path": first.to_str().expect("UTF-8 target"), "owner": null}])
    );
    assert_eq!(
        fs::read(first.join("notes-summary/SKILL.md")).expect("skill unchanged"),
        before
    );
    let conflict = report(
        &[
            "ai",
            "install",
            "--source",
            competing.to_str().expect("UTF-8"),
            "--target",
            first.to_str().expect("UTF-8"),
            "--state",
            state.to_str().expect("UTF-8"),
        ],
        1,
    );
    assert_eq!(
        conflict["results"][0]["error"],
        serde_json::json!({
            "code": "conflict",
            "errno": Value::Null,
            "message": "claim overlaps recorded ownership",
            "path": first.to_str().expect("UTF-8 target")
        })
    );
    assert_eq!(
        conflict["results"][0]["observation"]["conflicts"],
        serde_json::json!([])
    );
    assert_eq!(
        conflict["results"][0]["recovery_paths"],
        serde_json::json!([])
    );
    fs::write(first.join("notes-summary/SKILL.md"), "local edit").expect("edit installed skill");
    content[1] = "update";
    let modified = report(&content, 1);
    assert_eq!(modified["results"][0]["error"]["code"], "modified");
    assert_eq!(
        modified["results"][0]["observation"]["modifications"][0],
        serde_json::json!({
            "bundle_id": "example-notes",
            "kind": "content_changed",
            "path": "notes-summary"
        })
    );
    assert_eq!(
        modified["results"][0]["recovery_paths"],
        serde_json::json!([])
    );
    content.push("--replace-modified");
    assert_eq!(report(&content, 0)["results"][0]["status"], "applied");
    fs::remove_dir_all(&copied).expect("remove source");
    let mut uninstall = vec!["ai", "uninstall", "--bundle-id", "example-notes"];
    uninstall.extend(targets);
    let removed = report(&uninstall, 0);
    assert_eq!(removed["bundle_version"], Value::Null);
    assert_eq!(removed["results"][0]["status"], "applied");
    assert!(!first.exists());
    assert!(!second.exists());
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied source");
    for entry in fs::read_dir(source).expect("read source") {
        let entry = entry.expect("source entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy source file");
        }
    }
}
