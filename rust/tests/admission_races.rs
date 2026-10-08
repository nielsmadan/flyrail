#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use flyrail::{ResourceAuthority, ResourceKind, ResourceRef, admit, decode, reserve};
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
    let (Some(destination), Some(kind), Some(barrier)) = (args.get(2), args.get(3), args.get(4))
    else {
        eprintln!("worker requires destination, kind and barrier arguments");
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
    let reference = match ResourceRef::new(
        authority.destination().to_path_buf(),
        kind,
        authority.state_root().to_path_buf(),
    ) {
        Ok(reference) => reference,
        Err(error) => {
            eprintln!("worker reference: {error}");
            return 1;
        }
    };
    if let Err(error) = reserve(&reference) {
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
    match admit(&reference) {
        Ok(()) => {
            println!("admitted");
            0
        }
        Err(error) => {
            println!("{}", error.code());
            0
        }
    }
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

fn fence_of(destination: &Path) -> PathBuf {
    authority_for(destination).lock_path()
}

fn wait_for_fence(destination: &Path) {
    let fence = fence_of(destination);
    let deadline = Instant::now() + WAIT;
    while !fence.is_file() {
        assert!(
            Instant::now() <= deadline,
            "admission fence never appeared at {}",
            fence.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn spawn_worker(root: &Path, label: &str, destination: &Path, kind: &str) -> Child {
    let barrier = root.join(format!("barrier-{label}"));
    Command::new(std::env::current_exe().expect("test binary"))
        .arg(WORKER)
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

fn recorded_header(destination: &Path) -> ResourceRef {
    let state_root = authority_for(destination).state_root().to_path_buf();
    let bytes = std::fs::read(state_root.join("authority.json"))
        .unwrap_or_else(|error| panic!("authority.json must be readable: {error}"));
    decode(&bytes).expect("authority.json must decode")
}

fn reference_for(destination: &Path, kind: &str) -> ResourceRef {
    let authority = authority_for(destination);
    ResourceRef::new(
        authority.destination().to_path_buf(),
        ResourceKind::from_str(kind).expect("kind"),
        authority.state_root().to_path_buf(),
    )
    .expect("resource")
}

fn assert_fences_retained(tree: &Path, child: &Path, kind: &str) {
    assert!(fence_of(tree).is_file(), "tree fence must persist");
    assert!(fence_of(child).is_file(), "child fence must persist");
    assert_eq!(
        recorded_header(tree),
        reference_for(tree, kind),
        "tree identity must be recorded"
    );
    assert_eq!(
        recorded_header(child),
        reference_for(child, "document"),
        "child identity must be recorded even after its refusal"
    );
}

fn tree_first_ordering() {
    let root = unique_root("tree-first");
    let tree = root.join("config");
    std::fs::create_dir(&tree).expect("tree destination");
    let tree_worker = spawn_worker(&root, "tree", &tree, "tree");
    wait_for_fence(&tree);
    release(&root, "tree");
    assert_eq!(outcome_of(tree_worker, "tree"), "admitted");
    let child = tree.join("child");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&child);
    release(&root, "child");
    assert_eq!(outcome_of(child_worker, "child"), "conflict");
    assert_fences_retained(&tree, &child, "tree");
    let _ = std::fs::remove_dir_all(&root);
}

fn child_first_ordering() {
    let root = unique_root("child-first");
    let tree = root.join("config");
    std::fs::create_dir(&tree).expect("tree destination");
    let child = tree.join("child");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&child);
    release(&root, "child");
    assert_eq!(outcome_of(child_worker, "child"), "admitted");
    let tree_worker = spawn_worker(&root, "tree", &tree, "tree");
    wait_for_fence(&tree);
    release(&root, "tree");
    assert_eq!(outcome_of(tree_worker, "tree"), "conflict");
    assert_fences_retained(&tree, &child, "tree");
    let _ = std::fs::remove_dir_all(&root);
}

fn simultaneous_ordering() {
    let root = unique_root("simultaneous");
    let tree = root.join("config");
    std::fs::create_dir(&tree).expect("tree destination");
    let child = tree.join("child");
    let tree_worker = spawn_worker(&root, "tree", &tree, "tree");
    let child_worker = spawn_worker(&root, "child", &child, "document");
    wait_for_fence(&tree);
    wait_for_fence(&child);
    release(&root, "tree");
    release(&root, "child");
    assert_eq!(outcome_of(tree_worker, "tree"), "conflict");
    assert_eq!(outcome_of(child_worker, "child"), "conflict");
    assert_fences_retained(&tree, &child, "tree");
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
    wait_for_fence(&child);
    release(&root, "child");
    assert_eq!(outcome_of(child_worker, "child"), "admitted");
    let tree_worker = spawn_worker(&root, "tree", &aliased_tree, "tree");
    wait_for_fence(&exact);
    release(&root, "tree");
    assert_eq!(outcome_of(tree_worker, "tree"), "conflict");
    assert!(
        fence_of(&exact).is_file(),
        "aliased tree fence must persist"
    );
    assert_eq!(
        recorded_header(&exact),
        reference_for(&exact, "tree"),
        "the aliased spelling must record the canonical identity"
    );
    assert!(fence_of(&child).is_file(), "child fence must persist");
    assert_eq!(
        recorded_header(&child),
        reference_for(&child, "document"),
        "child identity must be recorded"
    );
    let _ = std::fs::remove_dir_all(&root);
}
