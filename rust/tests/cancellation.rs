#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use flyrail::{
    Acquisition, Artifact, Bundle, BundleEntry, BundleIdentity, Cancellation, Cancelled, Content,
    DirectoryStore, ErrorCode, Family, FileContent, FileMode, InstallationTarget, MemoryStore,
    OperationStatus, RenderedArtifact, RenderedBundle, SkillArtifact, StateStore, StoreError,
    StoreLock, TreeContent, apply_preview, cancellable, install, install_with_timeout, preview,
    recover_installation, sync,
};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);
const CANCEL_AFTER: Duration = Duration::from_millis(100);
const PROMPT: Duration = Duration::from_secs(1);

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

fn tree(data: &[u8]) -> TreeContent {
    TreeContent::new([BundleEntry::file("SKILL.md", data.to_vec()).expect("file")]).expect("tree")
}

fn skills() -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("team", "1").expect("identity"),
        [Artifact::Skill(
            SkillArtifact::new("skill", "review", tree(b"skill")).expect("skill"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn pair_bundle() -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("team", "1").expect("identity"),
        [
            Artifact::Skill(SkillArtifact::new("first", "first", tree(b"first")).expect("first")),
            Artifact::Skill(
                SkillArtifact::new("second", "second", tree(b"second")).expect("second"),
            ),
        ],
        [],
        [],
    )
    .expect("bundle")
}

fn file(identifier: &str, path: &Path, data: &[u8]) -> RenderedArtifact {
    RenderedArtifact::new(
        identifier,
        Family::Skills,
        path.to_path_buf(),
        Content::File(FileContent::new(data, FileMode::Readable)),
        None,
        false,
        Vec::new(),
    )
    .expect("artifact")
}

fn context_digest(store: &Arc<dyn StateStore>, root: &Path) -> String {
    InstallationTarget::new(
        Arc::clone(store),
        [(String::from("destination"), root.display().to_string())],
        [],
    )
    .expect("installation target")
    .context_digest()
}

struct Holder {
    release: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl Holder {
    fn hold(store: Arc<dyn StateStore>, name: String) -> Self {
        let (held, ready) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            let _lock = store.lock(&name, Duration::ZERO).expect("held lock");
            held.send(()).expect("signal held");
            let _ = released.recv();
        });
        ready.recv().expect("lock held");
        Self { release, thread }
    }

    fn release(self) {
        self.release.send(()).expect("release");
        self.thread.join().expect("holder thread");
    }
}

fn cancel_later(token: &Cancellation) -> JoinHandle<()> {
    let token = token.clone();
    thread::spawn(move || {
        thread::sleep(CANCEL_AFTER);
        token.cancel();
    })
}

struct Observed {
    inner: MemoryStore,
    locks: AtomicUsize,
    cancel_on_receipt: Option<Cancellation>,
}

impl Observed {
    fn new(cancel_on_receipt: Option<Cancellation>) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(),
            locks: AtomicUsize::new(0),
            cancel_on_receipt,
        })
    }
}

impl StateStore for Observed {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.read(key)
    }

    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        if key.ends_with("/receipt")
            && let Some(token) = &self.cancel_on_receipt
        {
            token.cancel();
        }
        self.inner.write(key, value, expected)
    }

    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
        self.inner.delete(key, expected)
    }

    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.inner.keys(prefix)
    }

    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
        self.locks.fetch_add(1, Ordering::SeqCst);
        self.inner.lock(name, timeout)
    }
}

#[test]
fn a_cancelled_lock_wait_on_a_later_resource_returns_before_any_commit() {
    let root = temp_root("cancel-memory-resource");
    let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
    let first = root.join("a.txt");
    let second = root.join("b.txt");
    let bundle = pair_bundle();
    let rendered = RenderedBundle::new(
        [
            file("first", &first, b"first"),
            file("second", &second, b"second"),
        ],
        [],
        [],
    )
    .expect("rendered");
    let target = InstallationTarget::without_context(Arc::clone(&store)).expect("target");
    let holder = Holder::hold(
        Arc::clone(&store),
        format!("resource/{}", support::destination_key(&second)),
    );
    let token = Cancellation::new();
    let canceller = cancel_later(&token);
    let started = Instant::now();

    let outcome = cancellable(&token, || {
        sync(
            &bundle,
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            WAIT,
        )
    });

    let elapsed = started.elapsed();
    canceller.join().expect("canceller");
    holder.release();
    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    assert!(elapsed < PROMPT, "cancelled after {elapsed:?}");
    let index = format!("index/team/{}", target.context_digest());
    assert_eq!(store.read(&index).expect("index read"), None);
    let records = store.keys("resource/").expect("resource keys");
    assert_eq!(records, [support::resource_key(&first, "authority")]);
    assert_eq!(
        [
            first.try_exists().expect("first"),
            second.try_exists().expect("second")
        ],
        [false, false]
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_cancelled_directory_store_index_wait_leaves_no_index() {
    let root = temp_root("cancel-directory-index");
    let state = root.join("state");
    let store: Arc<dyn StateStore> = Arc::new(DirectoryStore::new(&state).expect("store"));
    let other: Arc<dyn StateStore> = Arc::new(DirectoryStore::new(&state).expect("other store"));
    let destination = root.join("skill.md");
    fs::write(&destination, b"original").expect("destination");
    let bundle = skills();
    let rendered =
        RenderedBundle::new([file("skill", &destination, b"desired")], [], []).expect("rendered");
    let target = InstallationTarget::new(
        Arc::clone(&store),
        [(String::from("project"), String::from("one"))],
        [],
    )
    .expect("target");
    let index = format!("index/team/{}", target.context_digest());
    let holder = Holder::hold(other, index.clone());
    let token = Cancellation::new();
    let canceller = cancel_later(&token);
    let started = Instant::now();

    let outcome = cancellable(&token, || {
        sync(
            &bundle,
            &rendered,
            &target,
            Acquisition::Takeover,
            false,
            WAIT,
        )
    });

    let elapsed = started.elapsed();
    canceller.join().expect("canceller");
    holder.release();
    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    assert!(elapsed < PROMPT, "cancelled after {elapsed:?}");
    assert_eq!(store.read(&index).expect("index read"), None);
    assert_eq!(fs::read(&destination).expect("destination"), b"original");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_pre_cancelled_token_makes_no_lock_attempt() {
    let root = temp_root("cancel-pre-cancelled");
    let observed = Observed::new(None);
    let store: Arc<dyn StateStore> = observed.clone();
    let token = Cancellation::new();
    token.cancel();

    let outcome = cancellable(&token, || {
        install(&skills(), [root.join("one"), root.join("two")], &store)
    });

    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    assert_eq!(observed.locks.load(Ordering::SeqCst), 0);
    assert_eq!(store.keys("").expect("keys"), Vec::<String>::new());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_unchanged_target_leaves_later_targets_cancellable() {
    let root = temp_root("cancel-unchanged-first");
    let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
    let current = root.join("one");
    let waiting = root.join("two");
    let bundle = skills();
    let installed = install(&bundle, [current.clone()], &store).expect("first install");
    assert_eq!(installed[0].status(), OperationStatus::Applied);
    let index = format!("index/team/{}", context_digest(&store, &waiting));
    let holder = Holder::hold(Arc::clone(&store), index.clone());
    let token = Cancellation::new();
    let canceller = cancel_later(&token);
    let started = Instant::now();

    let outcome = cancellable(&token, || {
        install_with_timeout(&bundle, [current.clone(), waiting.clone()], &store, WAIT)
    });

    let elapsed = started.elapsed();
    canceller.join().expect("canceller");
    holder.release();
    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    assert!(elapsed < PROMPT, "cancelled after {elapsed:?}");
    assert_eq!(store.read(&index).expect("index read"), None);
    assert_eq!(
        fs::read(current.join("review/SKILL.md")).expect("current skill"),
        b"skill"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cancellation_after_the_first_durable_write_returns_the_real_results() {
    let root = temp_root("cancel-after-commit");
    let token = Cancellation::new();
    let store: Arc<dyn StateStore> = Observed::new(Some(token.clone()));
    let first = root.join("one");
    let second = root.join("two");
    let index = format!("index/team/{}", context_digest(&store, &second));
    let holder = Holder::hold(Arc::clone(&store), index);
    let timeout = Duration::from_millis(250);
    let started = Instant::now();

    let outcome = cancellable(&token, || {
        install_with_timeout(&skills(), [first.clone(), second.clone()], &store, timeout)
    });

    let elapsed = started.elapsed();
    holder.release();
    assert!(token.is_cancelled());
    let results = outcome
        .expect("committed scope returns its results")
        .expect("install");
    assert_eq!(
        results
            .iter()
            .map(flyrail::TargetResult::status)
            .collect::<Vec<_>>(),
        [OperationStatus::Applied, OperationStatus::Failed]
    );
    assert_eq!(
        results[1].error().map(flyrail::TargetError::code),
        Some(ErrorCode::Busy)
    );
    assert!(elapsed >= timeout, "returned after {elapsed:?}");
    assert_eq!(
        fs::read(first.join("review/SKILL.md")).expect("first skill"),
        b"skill"
    );
    let _ = fs::remove_dir_all(root);
}

struct Probe {
    inner: MemoryStore,
    crash_on: Option<&'static str>,
    crash_armed: AtomicBool,
    cancel_after_resource_lock: Mutex<Option<Cancellation>>,
    cancel_on_index_write: Option<Cancellation>,
    fail_preparation: AtomicBool,
}

impl Probe {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            crash_on: None,
            crash_armed: AtomicBool::new(false),
            cancel_after_resource_lock: Mutex::new(None),
            cancel_on_index_write: None,
            fail_preparation: AtomicBool::new(false),
        }
    }
}

impl StateStore for Probe {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.read(key)
    }

    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        if let Some(token) = &self.cancel_on_index_write
            && key.starts_with("index/")
        {
            token.cancel();
            self.fail_preparation.store(true, Ordering::SeqCst);
        }
        if key.ends_with("/preparation") && self.fail_preparation.load(Ordering::SeqCst) {
            return Err(StoreError::Io {
                message: String::from("injected preparation failure"),
                path: None,
                kind: std::io::ErrorKind::Other,
                errno: None,
            });
        }
        let written = self.inner.write(key, value, expected)?;
        if let Some(suffix) = self.crash_on
            && key.ends_with(suffix)
            && self.crash_armed.swap(false, Ordering::SeqCst)
        {
            resume_unwind(Box::new("interrupted"));
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
        let lock = self.inner.lock(name, timeout)?;
        if name.starts_with("resource/")
            && let Some(token) = self
                .cancel_after_resource_lock
                .lock()
                .expect("token slot")
                .as_ref()
        {
            token.cancel();
        }
        Ok(lock)
    }
}

fn single_target(store: &Arc<dyn StateStore>, root: &Path) -> InstallationTarget {
    InstallationTarget::new(
        Arc::clone(store),
        [(String::from("destination"), root.display().to_string())],
        [],
    )
    .expect("installation target")
}

fn crashed_install(suffix: &'static str, label: &str) -> (PathBuf, Arc<Probe>, PathBuf) {
    let root = temp_root(label);
    let probe = Arc::new(Probe {
        crash_on: Some(suffix),
        ..Probe::new()
    });
    let store: Arc<dyn StateStore> = probe.clone();
    let crashed = root.join("crashed");
    probe.crash_armed.store(true, Ordering::SeqCst);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        install(&skills(), [crashed.clone()], &store)
    }));
    assert!(outcome.is_err(), "{suffix}: install was not interrupted");
    assert!(
        store
            .keys("")
            .expect("keys")
            .iter()
            .any(|key| key.ends_with("/preparation")),
        "{suffix}: no preparation seeded"
    );
    (root, probe, crashed)
}

#[test]
fn recovery_marks_the_scope_committed_for_every_transaction_stage() {
    for (suffix, stage) in [
        ("/preparation", "abandon"),
        ("/journal", "rollback"),
        ("/receipt", "cleanup"),
    ] {
        let (root, probe, crashed) = crashed_install(suffix, &format!("recover-{stage}"));
        let store: Arc<dyn StateStore> = probe;
        let target = single_target(&store, &crashed);
        let token = Cancellation::new();
        let other = root.join("other");

        let outcome = cancellable(&token, || {
            let recovered = recover_installation("team", &target, Duration::ZERO)
                .expect("recover installation");
            token.cancel();
            (recovered, install(&skills(), [other.clone()], &store))
        });

        let (recovered, installed) = outcome.expect("scope stays committed");
        assert_eq!(recovered.status(), OperationStatus::Applied, "{stage}");
        assert_eq!(
            installed.expect("install")[0].status(),
            OperationStatus::Applied,
            "{stage}"
        );
        assert!(
            store
                .keys("")
                .expect("keys")
                .iter()
                .all(|key| !key.ends_with("/preparation")),
            "{stage}: preparation remains"
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn a_cancel_before_the_first_preparation_write_still_commits_the_scope() {
    let root = temp_root("cancel-pending-index");
    let token = Cancellation::new();
    let probe = Arc::new(Probe {
        cancel_on_index_write: Some(token.clone()),
        ..Probe::new()
    });
    let store: Arc<dyn StateStore> = probe;
    let first = root.join("one");
    let second = root.join("two");
    let index = format!("index/team/{}", context_digest(&store, &second));
    let holder = Holder::hold(Arc::clone(&store), index);

    let outcome = cancellable(&token, || {
        install_with_timeout(&skills(), [first, second], &store, Duration::ZERO)
    });

    holder.release();
    let results = outcome
        .expect("committed scope returns its results")
        .expect("install");
    assert_eq!(
        results
            .iter()
            .map(flyrail::TargetResult::status)
            .collect::<Vec<_>>(),
        [OperationStatus::Failed, OperationStatus::Failed]
    );
    assert_eq!(
        results[1].error().map(flyrail::TargetError::code),
        Some(ErrorCode::Busy)
    );
    let _ = fs::remove_dir_all(root);
}

fn cancelled_after_locks() -> (Cancellation, Arc<Probe>) {
    let token = Cancellation::new();
    let probe = Arc::new(Probe::new());
    *probe.cancel_after_resource_lock.lock().expect("token slot") = Some(token.clone());
    (token, probe)
}

#[test]
fn a_cancel_after_the_last_lock_vetoes_synchronize() {
    let root = temp_root("veto-sync");
    let (token, probe) = cancelled_after_locks();
    let store: Arc<dyn StateStore> = probe;
    let path = root.join("a.txt");
    let target = InstallationTarget::without_context(Arc::clone(&store)).expect("target");
    let rendered = RenderedBundle::new([file("first", &path, b"first")], [], []).expect("rendered");

    let outcome = cancellable(&token, || {
        sync(
            &pair_bundle(),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
    });

    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    let index = format!("index/team/{}", target.context_digest());
    assert_eq!(store.read(&index).expect("index read"), None);
    assert!(!path.try_exists().expect("destination"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_cancel_after_the_last_lock_vetoes_apply() {
    let root = temp_root("veto-apply");
    let (token, probe) = cancelled_after_locks();
    let store: Arc<dyn StateStore> = probe;
    let path = root.join("a.txt");
    let target = InstallationTarget::without_context(Arc::clone(&store)).expect("target");
    let rendered = RenderedBundle::new([file("first", &path, b"first")], [], []).expect("rendered");
    let proposal = preview(
        &pair_bundle(),
        &rendered,
        &target,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");

    let outcome = cancellable(&token, || apply_preview(&proposal, Duration::ZERO));

    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    let index = format!("index/team/{}", target.context_digest());
    assert_eq!(store.read(&index).expect("index read"), None);
    assert!(!path.try_exists().expect("destination"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_cancel_after_the_last_lock_vetoes_recovery() {
    let (root, probe, crashed) = crashed_install("/journal", "veto-recover");
    let token = Cancellation::new();
    *probe.cancel_after_resource_lock.lock().expect("token slot") = Some(token.clone());
    let store: Arc<dyn StateStore> = probe;
    let target = single_target(&store, &crashed);

    let outcome = cancellable(&token, || {
        recover_installation("team", &target, Duration::ZERO)
    });

    assert_eq!(outcome.map(|_| ()), Err(Cancelled));
    let keys = store.keys("").expect("keys");
    assert!(keys.iter().any(|key| key.ends_with("/preparation")));
    assert!(keys.iter().any(|key| key.ends_with("/journal")));
    let _ = fs::remove_dir_all(root);
}
