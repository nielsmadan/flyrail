#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use flyrail::{
    DirectoryStore, ResourceAuthority, ResourceKind, ResourceRef, StateStore, admit, decode, encode,
};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const WORKER: &str = "flyrail-admission-worker";
const WAIT: Duration = Duration::from_secs(30);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == WORKER) {
        std::process::exit(run_worker(&args));
    }
    tree_first_ordering();
    child_first_ordering();
    simultaneous_ordering();
    case_alias_ordering();
    println!("admission race orderings: 4 scenarios passed");
}

fn run_worker(args: &[String]) -> i32 {
    let (Some(state), Some(destination), Some(kind), Some(barrier)) =
        (args.get(2), args.get(3), args.get(4), args.get(5))
    else {
        eprintln!("worker requires state, destination, kind and barrier arguments");
        return 64;
    };
    let kind = match ResourceKind::from_str(kind) {
        Ok(kind) => kind,
        Err(error) => {
            eprintln!("worker kind: {error}");
            return 64;
        }
    };
    let authority = match ResourceAuthority::new(destination) {
        Ok(authority) => authority,
        Err(error) => {
            eprintln!("worker authority: {error}");
            return 1;
        }
    };
    let reference = match ResourceRef::new(authority.destination().to_path_buf(), kind) {
        Ok(reference) => reference,
        Err(error) => {
            eprintln!("worker reference: {error}");
            return 1;
        }
    };
    let store = match DirectoryStore::new(state) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("worker store: {error}");
            return 2;
        }
    };
    let name = format!(
        "resource/{}",
        support::destination_key(reference.destination())
    );
    let _lock = match store.lock(&name, Duration::from_secs(10)) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("worker lock: {error}");
            return 2;
        }
    };
    let key = support::resource_key(reference.destination(), "authority");
    if let Err(error) = store.write(&key, &encode(&reference), None) {
        eprintln!("worker reserve: {error}");
        return 2;
    }
    println!("reserved");
    let barrier = Path::new(barrier);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !barrier.exists() {
        if Instant::now() > deadline {
            eprintln!("worker barrier timed out");
            return 3;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    match admit(&store, &reference) {
        Ok(()) => println!("admitted"),
        Err(error) => println!("{}", error.code()),
    }
    0
}

fn unique_root(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = format!(
        "{}-{}-{label}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.cache/rust-tests")
        .join(unique);
    std::fs::create_dir_all(&root).expect("race root must be creatable");
    std::fs::canonicalize(&root).expect("race root must resolve")
}

fn authority_for(destination: &Path) -> ResourceAuthority {
    ResourceAuthority::new(destination).expect("authority")
}

fn state_of(root: &Path) -> PathBuf {
    root.join("state")
}

fn recorded(root: &Path, destination: &Path) -> Option<Vec<u8>> {
    let canonical = authority_for(destination).destination().to_path_buf();
    DirectoryStore::new(state_of(root))
        .expect("store")
        .read(&support::resource_key(&canonical, "authority"))
        .expect("authority read")
}

fn wait_for_fence(root: &Path, destination: &Path) {
    let deadline = Instant::now() + WAIT;
    while recorded(root, destination).is_none() {
        assert!(
            Instant::now() <= deadline,
            "authority record never appeared for {}",
            destination.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn spawn_worker(root: &Path, label: &str, destination: &Path, kind: &str) -> Child {
    let barrier = root.join(format!("barrier-{label}"));
    Command::new(std::env::current_exe().expect("test binary"))
        .arg(WORKER)
        .arg(state_of(root))
        .arg(destination)
        .arg(kind)
        .arg(&barrier)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("worker {label} failed to spawn: {error}"))
}

fn release(root: &Path, label: &str) {
    let barrier = root.join(format!("barrier-{label}"));
    std::fs::write(&barrier, b"validate\n").unwrap_or_else(|error| {
        panic!("barrier {label} failed to appear: {error}");
    });
}

fn outcome_of(child: Child, label: &str) -> String {
    let output = child.wait_with_output().expect("worker output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "worker {label} failed with {}: {}",
        output.status,
        stderr
    );
    stdout.lines().last().unwrap_or_default().to_owned()
}

fn recorded_header(root: &Path, destination: &Path) -> ResourceRef {
    let bytes = recorded(root, destination).expect("authority record must persist");
    decode(&bytes).expect("authority record must decode")
}

fn reference_for(destination: &Path, kind: &str) -> ResourceRef {
    let authority = authority_for(destination);
    ResourceRef::new(
        authority.destination().to_path_buf(),
        ResourceKind::from_str(kind).expect("kind"),
    )
    .expect("resource")
}

fn assert_fences_retained(root: &Path, tree: &Path, child: &Path, kind: &str) {
    assert_eq!(
        recorded_header(root, tree),
        reference_for(tree, kind),
        "tree identity must be recorded"
    );
    assert_eq!(
        recorded_header(root, child),
        reference_for(child, "document"),
        "child identity must be recorded even after its refusal"
    );
}

fn tree_first_ordering() {
    let root = unique_root("tree-first");
    let tree = root.join("config");
    std::fs::create_dir(&tree).expect("tree destination");
    let tree_worker = spawn_worker(&root, "tree", &tree, "tree");
    wait_for_fence(&root, &tree);
    release(&root, "tree");
    assert_eq!(outcome_of(tree_worker, "tree"), "admitted");
    let child = tree.join("child");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&root, &child);
    release(&root, "child");
    assert_eq!(outcome_of(child_worker, "child"), "conflict");
    assert_fences_retained(&root, &tree, &child, "tree");
    let _ = std::fs::remove_dir_all(&root);
}

fn child_first_ordering() {
    let root = unique_root("child-first");
    let tree = root.join("config");
    std::fs::create_dir(&tree).expect("tree destination");
    let child = tree.join("child");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&root, &child);
    release(&root, "child");
    assert_eq!(outcome_of(child_worker, "child"), "admitted");
    let tree_worker = spawn_worker(&root, "tree", &tree, "tree");
    wait_for_fence(&root, &tree);
    release(&root, "tree");
    assert_eq!(outcome_of(tree_worker, "tree"), "conflict");
    assert_fences_retained(&root, &tree, &child, "tree");
    let _ = std::fs::remove_dir_all(&root);
}

fn simultaneous_ordering() {
    let root = unique_root("simultaneous");
    let tree = root.join("config");
    std::fs::create_dir(&tree).expect("tree destination");
    let child = tree.join("child");
    let tree_worker = spawn_worker(&root, "tree", &tree, "tree");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&root, &tree);
    wait_for_fence(&root, &child);
    release(&root, "tree");
    release(&root, "child");
    assert_eq!(outcome_of(tree_worker, "tree"), "conflict");
    assert_eq!(outcome_of(child_worker, "child"), "conflict");
    assert_fences_retained(&root, &tree, &child, "tree");
    let _ = std::fs::remove_dir_all(&root);
}

fn case_alias_ordering() {
    let root = unique_root("case-alias");
    let exact = root.join("config");
    std::fs::create_dir(&exact).expect("tree destination");
    let probe = root.join("case-probe");
    std::fs::create_dir(&probe).expect("case probe");
    let sensitive = std::fs::create_dir(root.join("CASE-PROBE")).is_ok();
    if sensitive {
        std::fs::remove_dir(root.join("CASE-PROBE")).expect("probe cleanup");
    }
    std::fs::remove_dir(&probe).expect("probe cleanup");
    if sensitive {
        println!("skipping: host is case sensitive");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    let aliased_tree = root.join("CONFIG");
    let child = exact.join("child");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&root, &child);
    release(&root, "child");
    assert_eq!(outcome_of(child_worker, "child"), "admitted");
    let tree_worker = spawn_worker(&root, "tree", &aliased_tree, "tree");
    wait_for_fence(&root, &exact);
    release(&root, "tree");
    assert_eq!(outcome_of(tree_worker, "tree"), "conflict");
    assert_eq!(
        recorded_header(&root, &exact),
        reference_for(&exact, "tree"),
        "the aliased spelling must record the canonical identity"
    );
    assert_eq!(
        recorded_header(&root, &child),
        reference_for(&child, "document"),
        "child identity must be recorded"
    );
    let _ = std::fs::remove_dir_all(&root);
}
