#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use flyrail::{
    Acquisition, Artifact, Bundle, BundleIdentity, Content, DirectoryStore, ErrorCode, Family,
    InstallationResult, InstallationTarget, InstructionArtifact, MemoryStore, OperationStatus,
    Preparation, Receipt, RenderedArtifact, RenderedBundle, SectionContent, StateStore, StoreError,
    StoreLock, decode, recover_installation, sync,
};
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use support::resource_key;

const REPAIRED: &[u8] =
    b"# mine\n\n<!-- flyrail:guide:start -->\none\n<!-- flyrail:guide:end -->\n";

fn temp_root(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.cache/rust-tests")
        .join(format!(
            "{}-{}-{label}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temporary root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
    fs::canonicalize(root).expect("canonical root")
}

fn bundle(version: &str) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("team", version).expect("identity"),
        [Artifact::Instruction(
            InstructionArtifact::new("guide", "text").expect("instruction"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn rendered(path: &Path, text: &str) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path.to_path_buf(),
            Content::Section(SectionContent::new("guide", text, None).expect("section")),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact")],
        [],
        [],
    )
    .expect("rendered")
}

fn target_of(store: Arc<dyn StateStore>) -> InstallationTarget {
    InstallationTarget::without_context(store).expect("target")
}

fn synced(
    version: &str,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    replace_modified: bool,
) -> InstallationResult {
    sync(
        &bundle(version),
        rendered,
        target,
        Acquisition::Conflict,
        replace_modified,
        Duration::ZERO,
    )
    .expect("sync")
}

fn receipt(store: &dyn StateStore, destination: &Path) -> Receipt {
    decode(
        &store
            .read(&resource_key(destination, "receipt"))
            .expect("receipt read")
            .expect("receipt"),
    )
    .expect("receipt record")
}

#[test]
fn a_drift_repair_commits_a_fresh_transaction_and_a_noop_keeps_the_receipt() {
    let root = temp_root("recovery-fresh-transaction");
    let path = root.join("AGENTS.md");
    fs::write(&path, b"# mine\n").expect("foreign");
    let store: Arc<dyn StateStore> =
        Arc::new(DirectoryStore::new(root.join("index")).expect("store"));
    let target = target_of(Arc::clone(&store));
    assert_eq!(
        synced("1", &rendered(&path, "one"), &target, false).status(),
        OperationStatus::Applied
    );
    let installed = String::from_utf8(fs::read(&path).expect("installed")).expect("text");
    fs::write(&path, installed.replace("one", "EDITED")).expect("drift");
    let drifted = receipt(store.as_ref(), &path);

    let repaired = synced("1", &rendered(&path, "one"), &target, true);

    assert_eq!(repaired.status(), OperationStatus::Applied);
    assert_eq!(repaired.resources()[0].status(), OperationStatus::Applied);
    assert_eq!(fs::read(&path).expect("repaired"), REPAIRED);
    let current = receipt(store.as_ref(), &path);
    assert_ne!(current.transaction_id(), drifted.transaction_id());
    assert_eq!(current.resource(), drifted.resource());
    assert_eq!(current.claims(), drifted.claims());
    assert_eq!(current.provenance(), drifted.provenance());
    assert_eq!(current.created_directories(), drifted.created_directories());
    assert_eq!(current.schema_version(), drifted.schema_version());
    assert_eq!(
        synced("1", &rendered(&path, "one"), &target, false).status(),
        OperationStatus::Unchanged
    );
    assert_eq!(receipt(store.as_ref(), &path), current);
}

struct CrashAfterJournal {
    inner: Box<dyn StateStore>,
    armed: AtomicBool,
}

impl StateStore for CrashAfterJournal {
    fn location(&self) -> Option<&Path> {
        self.inner.location()
    }
    fn staging_directory(&self) -> Option<PathBuf> {
        self.inner.staging_directory()
    }
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.read(key)
    }
    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        let written = self.inner.write(key, value, expected)?;
        if key.ends_with("/journal") && self.armed.swap(false, Ordering::SeqCst) {
            resume_unwind(Box::new("interrupted before publication"));
        }
        Ok(written)
    }
    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
        self.inner.delete(key, expected)
    }
    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.inner.keys(prefix)
    }
    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
        self.inner.lock(name, timeout)
    }
}

#[test]
fn recovery_refuses_the_previous_revision_rewritten_in_place_after_preparation() {
    for location in ["store", "beside"] {
        let root = temp_root("recovery-rewritten-in-place");
        let project = root.join("project");
        fs::create_dir(&project).expect("project");
        let destination = project.join("AGENTS.md");
        fs::write(&destination, b"# Foreign\n").expect("foreign");
        let inner: Box<dyn StateStore> = if location == "store" {
            Box::new(DirectoryStore::new(root.join("state")).expect("store"))
        } else {
            Box::new(MemoryStore::new())
        };
        let store = Arc::new(CrashAfterJournal {
            inner,
            armed: AtomicBool::new(false),
        });
        let target = target_of(Arc::clone(&store) as Arc<dyn StateStore>);
        assert_eq!(
            synced("1", &rendered(&destination, "one"), &target, false).status(),
            OperationStatus::Applied
        );
        let original = fs::read(&destination).expect("original");
        let inode = fs::metadata(&destination).expect("metadata").ino();
        store.armed.store(true, Ordering::SeqCst);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            synced("2", &rendered(&destination, "two"), &target, false)
        }));
        assert!(
            outcome.is_err(),
            "{location}: publication was not interrupted"
        );
        let preparation: Preparation = decode(
            &store
                .read(&resource_key(&destination, "preparation"))
                .expect("preparation read")
                .expect("preparation"),
        )
        .expect("preparation record");
        let directory = preparation.directory().to_path_buf();
        let mut stream = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&destination)
            .expect("rewrite in place");
        stream.write_all(&original).expect("original bytes");
        let later = fs::symlink_metadata(&directory)
            .and_then(|metadata| metadata.modified())
            .expect("boundary")
            .checked_add(Duration::from_secs(60))
            .expect("later time");
        stream
            .set_times(fs::FileTimes::new().set_modified(later))
            .expect("later modification time");
        drop(stream);
        assert_eq!(fs::metadata(&destination).expect("metadata").ino(), inode);

        let result = recover_installation("team", &target, Duration::ZERO).expect("recover");

        assert_eq!(result.status(), OperationStatus::Incomplete, "{location}");
        let error = result.error().expect("recovery error");
        assert_eq!(error.code(), ErrorCode::RecoveryNeeded);
        assert_eq!(
            error.message(),
            "destination revision changed after preparation"
        );
        assert_eq!(error.path(), Some(destination.as_path()));
        assert_eq!(fs::read(&destination).expect("retained"), original);
        assert!(directory.is_dir(), "{location}");
        assert!(
            store
                .read(&resource_key(&destination, "journal"))
                .expect("journal read")
                .is_some()
        );
    }
}
