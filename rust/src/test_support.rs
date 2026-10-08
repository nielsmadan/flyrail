#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::json::Json;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures")
}

pub fn fixture(name: &str) -> Option<Json> {
    let text = std::fs::read_to_string(fixture_root().join(name)).ok()?;
    Some(Json::parse(&text).expect("shared fixtures must be valid JSON"))
}

pub fn temp_dir(tag: &str) -> PathBuf {
    let unique = format!(
        "{}-{}-{tag}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let path = std::env::temp_dir().join(format!("longshot-rust-implementation/{unique}"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&path)
        .expect("temp dir must be creatable");
    std::fs::canonicalize(&path).expect("temp dir must resolve")
}

pub struct TempRoot(PathBuf);

impl std::ops::Deref for TempRoot {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn checkout_temp_dir(tag: &str) -> TempRoot {
    let unique = format!(
        "{}-{}-{tag}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.cache/rust-tests")
        .join(unique);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&path)
        .expect("checkout temp dir must be creatable");
    TempRoot(std::fs::canonicalize(&path).expect("checkout temp dir must resolve"))
}

pub fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir must be creatable");
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options.open(path).expect("test file must open");
    std::io::Write::write_all(&mut file, bytes).expect("test file must be writable");
}

pub fn write_str(path: &Path, text: &str) {
    write(path, text.as_bytes());
}

pub fn from_hex(text: &str) -> Vec<u8> {
    let digits: Vec<char> = text.chars().collect();
    digits
        .chunks(2)
        .map(|pair| {
            let byte: String = pair.iter().collect();
            u8::from_str_radix(&byte, 16).expect("fixture hex must be valid")
        })
        .collect()
}

pub const DEFAULT_MANIFEST: &str = concat!(
    r#"{"schema_version":1,"id":"team","version":"1","#,
    r#""skills":[{"name":"review","path":"skills/review"}]}"#
);

pub fn write_bundle(root: &Path, manifest: &str) -> PathBuf {
    write_str(&root.join("flyrail.json"), manifest);
    let skill = root.join("skills/review");
    std::fs::create_dir_all(&skill).expect("skill dir must be creatable");
    write(
        &skill.join("SKILL.md"),
        b"---\r\ninvalid yaml: [\r\n---\r\n\xff\x00",
    );
    skill
}

pub fn case_sensitive(root: &Path) -> bool {
    let probe = root.join("case-probe");
    std::fs::create_dir_all(&probe).expect("probe dir must be creatable");
    let shouted = root.join("CASE-PROBE");
    let sensitive = std::fs::create_dir(&shouted).is_ok();
    if sensitive {
        std::fs::remove_dir(&shouted).expect("probe dir must be removable");
    }
    std::fs::remove_dir(&probe).expect("probe dir must be removable");
    sensitive
}
