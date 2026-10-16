#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use flyrail::{DirectoryStore, StateStore, StoreError};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const HOLDER: &str = "FLYRAIL_STORE_LOCK_HOLDER";
const TEST: &str = "a_held_directory_store_lock_excludes_another_process";
const HELD: &str = "store-lock-held";

fn hold(root: &Path) {
    let store = DirectoryStore::new(root).unwrap();
    let _lock = store.lock("resource/held", Duration::ZERO).unwrap();
    println!("{HELD}");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}

fn unique_root() -> PathBuf {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.cache/rust-tests");
    std::fs::create_dir_all(&parent).unwrap();
    parent.join(format!("{}-store-locks", std::process::id()))
}

#[test]
fn a_held_directory_store_lock_excludes_another_process() {
    if let Some(root) = std::env::var_os(HOLDER) {
        hold(Path::new(&root));
        return;
    }
    let root = unique_root();
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(HOLDER, &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(holder.stdout.take().unwrap()).lines();
    assert!(
        output
            .by_ref()
            .any(|line| line.is_ok_and(|line| line.ends_with(HELD))),
        "the holder process must report the lock as held"
    );
    let store = DirectoryStore::new(&root).unwrap();
    let refused = store.lock("resource/held", Duration::ZERO).unwrap_err();
    assert_eq!(
        refused,
        StoreError::Busy {
            name: "resource/held".to_owned()
        }
    );
    drop(holder.stdin.take());
    output.for_each(drop);
    assert!(holder.wait().unwrap().success());
    let lock = store.lock("resource/held", Duration::ZERO).unwrap();
    drop(lock);
    std::fs::remove_dir_all(&root).unwrap();
}
