#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn python() -> Option<String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let driver = root.join("python/scripts/interop_driver.py");
    let result = Command::new("python3")
        .args([
            "-c",
            "import runpy, sys; sys.path.insert(0, sys.argv[1]); runpy.run_path(sys.argv[2], run_name='interop_driver')",
        ])
        .arg(root.join("python/src"))
        .arg(&driver)
        .output();
    match result {
        Ok(output) if output.status.success() && driver.is_file() => Some(String::from("python3")),
        Ok(output) => {
            assert!(
                std::env::var_os("CI").is_none(),
                "python3 and flyrail are required for interop in CI: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!(
                "skipping interop: python3 or flyrail import unavailable ({})",
                String::from_utf8_lossy(&output.stderr)
            );
            None
        }
        Err(error) => {
            assert!(
                std::env::var_os("CI").is_none(),
                "python3 is required for interop in CI: {error}"
            );
            eprintln!("skipping interop: python3 unavailable ({error})");
            None
        }
    }
}

fn feed(mut command: std::process::Child, input: &[u8]) -> io::Result<Output> {
    let mut stdin = command
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "child stdin was not captured"))?;
    stdin.write_all(input)?;
    drop(stdin);
    command.wait_with_output()
}

fn run_python(python: &str, driver: &Path, input: &[u8]) -> Output {
    let mut command = Command::new(python);
    command
        .arg(driver)
        .arg("encode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("python driver must spawn");
    feed(child, input).expect("python driver must finish")
}

fn run_rust(binary: &Path, type_name: &str, input: &[u8]) -> Output {
    let mut command = Command::new(binary);
    command
        .args(["encode", "--type", type_name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("Rust driver must spawn");
    feed(child, input).expect("Rust driver must finish")
}

fn hex_bytes(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("fixture hex is UTF-8");
            u8::from_str_radix(text, 16).expect("fixture bytes are hexadecimal")
        })
        .collect()
}

fn build_example(root: &Path) -> PathBuf {
    let binary = root.join("rust/target/debug/examples/interop");
    let status = Command::new("cargo")
        .args([
            "build",
            "--example",
            "interop",
            "--features",
            "interop-test-seams",
        ])
        .current_dir(root.join("rust"))
        .env("CARGO_HOME", root.join(".cache/cargo-home"))
        .status()
        .expect("cargo must spawn");
    assert!(status.success(), "interop example build failed");
    binary
}

fn vectors(python: &str, root: &Path) -> Option<Vec<(String, String, Vec<u8>)>> {
    if !root.join("spec/fixtures/configurations.json").is_file() {
        eprintln!("skipping interop: shared fixtures are unavailable");
        return None;
    }
    let output = Command::new(python)
        .arg(root.join("python/tests/interop_vectors.py"))
        .output()
        .expect("fixture vector helper must spawn");
    assert!(output.status.success(), "fixture helper failed: {output:?}");
    Some(
        String::from_utf8(output.stdout)
            .expect("fixture helper output is UTF-8")
            .lines()
            .map(|line| {
                let mut parts = line.split('\t');
                let name = parts.next().expect("fixture vector name");
                let type_name = parts.next().expect("fixture vector type");
                let data = parts.next().expect("fixture vector bytes");
                (name.to_owned(), type_name.to_owned(), hex_bytes(data))
            })
            .collect(),
    )
}

#[test]
fn python_and_rust_codecs_are_byte_identical() {
    let Some(python) = python() else {
        return;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let driver = root.join("python/scripts/interop_driver.py");
    let binary = build_example(&root);
    let Some(fixtures) = vectors(&python, &root) else {
        return;
    };
    for (name, type_name, fixture) in fixtures {
        let case = format!("{name} ({type_name})");
        let from_python = run_python(&python, &driver, &fixture);
        assert!(
            from_python.status.success(),
            "Python rejected {case}: {}",
            String::from_utf8_lossy(&from_python.stderr)
        );
        assert_eq!(from_python.stdout, fixture, "Python re-encoded {case}");

        let from_rust = run_rust(&binary, &type_name, &fixture);
        assert!(
            from_rust.status.success(),
            "Rust rejected {case}: {}",
            String::from_utf8_lossy(&from_rust.stderr)
        );
        assert_eq!(from_rust.stdout, fixture, "Rust re-encoded {case}");

        let python_after_rust = run_python(&python, &driver, &from_rust.stdout);
        assert_eq!(
            python_after_rust.stdout,
            fixture,
            "Python re-encoded Rust output for {case}: {}",
            String::from_utf8_lossy(&python_after_rust.stderr)
        );
        let rust_after_python = run_rust(&binary, &type_name, &from_python.stdout);
        assert_eq!(
            rust_after_python.stdout,
            fixture,
            "Rust re-encoded Python output for {case}: {}",
            String::from_utf8_lossy(&rust_after_python.stderr)
        );
    }
}

#[test]
fn python_and_rust_drivers_reject_an_unknown_state_record() {
    let Some(python) = python() else {
        return;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let driver = root.join("python/scripts/interop_driver.py");
    let binary = build_example(&root);
    let rejected = b"{\"fields\":{},\"type\":\"Nope\"}\n";

    let from_python = run_python(&python, &driver, rejected);
    assert_eq!(from_python.status.code(), Some(1));
    assert!(
        from_python.stdout.is_empty(),
        "python stdout: {}",
        String::from_utf8_lossy(&from_python.stdout)
    );
    assert!(
        String::from_utf8_lossy(&from_python.stderr).contains("unknown state record"),
        "python stderr: {}",
        String::from_utf8_lossy(&from_python.stderr)
    );

    let from_rust = run_rust(&binary, "Receipt", rejected);
    assert_eq!(from_rust.status.code(), Some(1));
    assert!(
        from_rust.stdout.is_empty(),
        "rust stdout: {}",
        String::from_utf8_lossy(&from_rust.stdout)
    );
    assert!(
        String::from_utf8_lossy(&from_rust.stderr).contains("decode Receipt: unknown state record"),
        "rust stderr: {}",
        String::from_utf8_lossy(&from_rust.stderr)
    );
}
