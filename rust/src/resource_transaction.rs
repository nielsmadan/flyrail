use crate::observation::{failure, io_failure, managed_metadata, managed_spelling};
use crate::observations::{ErrorCode, TargetError};
use crate::resource_io::{
    ancestors, cleanup_revision, destroy, error_detail, move_revision, observe, require,
    require_unchanged_since, resource_ancestors, validate_kind, write_revision,
};
use crate::resource_models::{
    Ancestor, Index, Preparation, Receipt, ResourceKind, ResourceRef, Revision,
};
use crate::resource_plan::ResourcePlan;
use crate::security::ensure_private;
use crate::state::{
    delete_record, destination_key, hierarchy_conflicts, read_record,
    recognized_transaction_directory, require_index, resource_key, store_lock, write_record,
};
use crate::stores::{StateStore, StoreLock};
use crate::values::ValueError;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
#[cfg(feature = "interop-test-seams")]
use std::{
    sync::{Mutex, OnceLock},
    thread,
    time::Instant,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Journal {
    resource: ResourceRef,
    previous: Option<Receipt>,
    receipt: Receipt,
    before: Revision,
    staged: Revision,
    published: Revision,
    backup: Revision,
    ancestors: Vec<Ancestor>,
    schema_version: i64,
}

fn resource_lock_name(destination: &Path) -> String {
    format!("resource/{}", destination_key(destination))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) struct ResourceLockGuard<'a> {
    reference: ResourceRef,
    _lock: StoreLock<'a>,
}

impl<'a> ResourceLockGuard<'a> {
    pub(crate) fn acquire(
        store: &'a dyn StateStore,
        reference: &ResourceRef,
        timeout: Duration,
    ) -> Result<Self, TargetError> {
        let lock = store_lock(store, &resource_lock_name(reference.destination()), timeout)?;
        Ok(Self {
            reference: reference.clone(),
            _lock: lock,
        })
    }

    pub(crate) const fn reference(&self) -> &ResourceRef {
        &self.reference
    }
}

#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub struct InteropResourceLockGuard<'a> {
    _guard: ResourceLockGuard<'a>,
}

#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub fn interop_resource_lock<'a>(
    store: &'a dyn StateStore,
    reference: &ResourceRef,
    timeout: Duration,
) -> Result<InteropResourceLockGuard<'a>, TargetError> {
    ResourceLockGuard::acquire(store, reference, timeout)
        .map(|guard| InteropResourceLockGuard { _guard: guard })
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn lock_resources<'a>(
    store: &'a dyn StateStore,
    references: &[ResourceRef],
    timeout: Duration,
) -> Result<Vec<ResourceLockGuard<'a>>, TargetError> {
    let mut ordered = references.to_vec();
    crate::resource_plan::destination_order(&mut ordered);
    ordered
        .iter()
        .map(|reference| ResourceLockGuard::acquire(store, reference, timeout))
        .collect()
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) struct TransactionLocks<'a> {
    store: &'a dyn StateStore,
    index_key: String,
    resources: Vec<ResourceLockGuard<'a>>,
    _index: StoreLock<'a>,
    created: Vec<Ancestor>,
}

fn create_parents(path: &Path, created: &mut Vec<Ancestor>) -> Result<(), TargetError> {
    let mut parents = path.ancestors().skip(1).collect::<Vec<_>>();
    parents.reverse();
    for parent in parents {
        if parent.exists() {
            continue;
        }
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {
                if let Some(item) = ancestors(&parent.join("child"))?
                    .into_iter()
                    .find(|item| item.path() == parent)
                {
                    created.push(item);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_failure(&error, parent)),
        }
    }
    Ok(())
}

fn create_store_parents(
    store: &dyn StateStore,
    created: &mut Vec<Ancestor>,
) -> Result<(), TargetError> {
    if let Some(location) = store.location() {
        create_parents(
            &crate::authority::physical_location(location).map_err(error_detail)?,
            created,
        )?;
    }
    if let Some(staging) = store.staging_directory() {
        create_parents(
            &crate::authority::physical_location(&staging)
                .map_err(error_detail)?
                .join("transaction"),
            created,
        )?;
    }
    Ok(())
}

impl<'a> TransactionLocks<'a> {
    #[allow(dead_code)]
    pub(crate) fn acquire(
        store: &'a dyn StateStore,
        index_key: &str,
        references: &[ResourceRef],
        timeout: Duration,
    ) -> Result<Self, TargetError> {
        Self::acquire_after_index_validation(store, index_key, references, timeout, || Ok(()))
    }

    pub(crate) fn acquire_after_index_validation(
        store: &'a dyn StateStore,
        index_key: &str,
        references: &[ResourceRef],
        timeout: Duration,
        validate: impl FnOnce() -> Result<(), TargetError>,
    ) -> Result<Self, TargetError> {
        let mut created = Vec::new();
        create_store_parents(store, &mut created)?;
        let index = store_lock(store, index_key, timeout)?;
        validate()?;
        let mut ordered = references.to_vec();
        crate::resource_plan::destination_order(&mut ordered);
        let mut resources = Vec::with_capacity(ordered.len());
        for reference in &ordered {
            validate_kind(reference)?;
            resources.push(ResourceLockGuard::acquire(store, reference, timeout)?);
            create_parents(reference.destination(), &mut created)?;
            crate::resource_io::admit(store, reference)?;
        }
        Ok(Self {
            store,
            index_key: index_key.to_owned(),
            resources,
            _index: index,
            created,
        })
    }

    pub(crate) fn resources(&self) -> &[ResourceLockGuard<'a>] {
        &self.resources
    }

    pub(crate) fn created(&self) -> &[Ancestor] {
        &self.created
    }

    fn failure(&self, code: ErrorCode, message: &str) -> TargetError {
        TargetError::new(
            code,
            message,
            self.store.location().map(Path::to_path_buf),
            None,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) struct TransactionOutcome {
    completed: Vec<ResourceRef>,
    unchanged: Vec<ResourceRef>,
    failed: Option<ResourceRef>,
    error: Option<TargetError>,
}

#[allow(dead_code)]
impl TransactionOutcome {
    pub(crate) fn completed(&self) -> &[ResourceRef] {
        &self.completed
    }

    pub(crate) fn unchanged(&self) -> &[ResourceRef] {
        &self.unchanged
    }

    pub(crate) const fn failed(&self) -> Option<&ResourceRef> {
        self.failed.as_ref()
    }

    pub(crate) const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }
}

#[allow(
    clippy::redundant_pub_crate,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub(crate) fn execute_transaction(
    locks: &TransactionLocks<'_>,
    plans: &[ResourcePlan],
    previous: Option<&Index>,
    pending: &Index,
    completed: &Index,
) -> Result<TransactionOutcome, TargetError> {
    let store = locks.store;
    let expected_completion = Index::new(
        pending.bundle_id().to_owned(),
        pending.context_digest().to_owned(),
        pending.pending().cloned(),
        None,
        None,
        Vec::new(),
        2,
    )
    .map_err(|error| locks.failure(ErrorCode::InvalidState, error.message()))?;
    if completed != &expected_completion {
        return Err(locks.failure(
            ErrorCode::InvalidState,
            "installation index completion disagrees with preparation",
        ));
    }
    if plans.len() != locks.resources().len()
        || plans.iter().any(|plan| {
            !locks
                .resources()
                .iter()
                .any(|guard| guard.reference() == plan.resource())
        })
    {
        return Err(locks.failure(ErrorCode::ConcurrentChange, "resource locks disagree"));
    }
    if let Some(error) = plans.iter().find_map(ResourcePlan::error) {
        return Ok(TransactionOutcome {
            completed: Vec::new(),
            unchanged: Vec::new(),
            failed: None,
            error: Some(error.clone()),
        });
    }
    if plans.iter().any(|plan| !plan.recovery_paths().is_empty()) {
        return Ok(TransactionOutcome {
            completed: Vec::new(),
            unchanged: Vec::new(),
            failed: None,
            error: Some(TargetError::new(
                ErrorCode::RecoveryNeeded,
                "preview requires recovery and re-observation",
                None,
                None,
            )),
        });
    }
    write_record(store, &locks.index_key, pending, previous)?;
    let mut applied = Vec::new();
    let mut unchanged = Vec::new();
    for plan in plans {
        if plan.previous() == Some(plan.receipt()) && plan.before() == plan.after() {
            unchanged.push(plan.resource().clone());
            continue;
        }
        let mut journal = None;
        let mut guard = None;
        let committed = std::cell::Cell::new(false);
        let result = (|| {
            inject("resource-precondition")?;
            inject("index-require")?;
            require_index(store, &locks.index_key, Some(pending))?;
            inject("guard-lookup")?;
            let acquired = locks
                .resources()
                .iter()
                .find(|guard| guard.reference() == plan.resource())
                .ok_or_else(|| {
                    failure(
                        ErrorCode::ConcurrentChange,
                        "resource lock disagrees",
                        plan.resource().destination(),
                    )
                })?;
            guard = Some(acquired);
            let prepared = prepare(store, plan, acquired)?;
            journal = Some(prepared.clone());
            require_index(store, &locks.index_key, Some(pending))?;
            publish_with(store, &prepared, || committed.set(true))?;
            cleanup(store, &prepared)
        })();
        if let Err(error) = result {
            if !committed.get() {
                committed.set(journal.as_ref().is_some_and(|journal| {
                    current_receipt(store, journal)
                        .is_ok_and(|current| current.as_ref() == Some(journal.receipt()))
                }));
            }
            let recovery_error = guard.and_then(|guard| recover(store, guard).err());
            let final_error = if let Some(recovery) = recovery_error {
                TargetError::new(
                    error.code(),
                    format!("{}; recovery: {}", error.message(), recovery.message()),
                    error.path().map(PathBuf::from),
                    error.errno(),
                )
            } else {
                error
            };
            return Ok(TransactionOutcome {
                completed: if committed.get() {
                    applied
                        .into_iter()
                        .chain(std::iter::once(plan.resource().clone()))
                        .collect()
                } else {
                    applied
                },
                unchanged,
                failed: Some(plan.resource().clone()),
                error: Some(final_error),
            });
        }
        applied.push(plan.resource().clone());
    }
    if let Err(error) = inject("index-completion")
        .and_then(|()| write_record(store, &locks.index_key, completed, Some(pending)))
    {
        return Ok(TransactionOutcome {
            completed: applied,
            unchanged,
            failed: None,
            error: Some(error),
        });
    }
    Ok(TransactionOutcome {
        completed: applied,
        unchanged,
        failed: None,
        error: None,
    })
}

fn paths(directory: &Path) -> (PathBuf, PathBuf) {
    (
        directory.join("staging").join("resource"),
        directory.join("backup").join("resource"),
    )
}

fn journal_key(reference: &ResourceRef) -> String {
    resource_key(reference.destination(), "journal")
}

fn receipt_key(reference: &ResourceRef) -> String {
    resource_key(reference.destination(), "receipt")
}

fn preparation_key(reference: &ResourceRef) -> String {
    resource_key(reference.destination(), "preparation")
}

fn store_failure(store: &dyn StateStore, code: ErrorCode, message: &str) -> TargetError {
    TargetError::new(code, message, store.location().map(Path::to_path_buf), None)
}

fn invalid(error: &ValueError, path: &Path) -> TargetError {
    failure(ErrorCode::InvalidState, error.message(), path)
}

fn matching(
    store: &dyn StateStore,
    preparation: Option<Preparation>,
    journal: &Journal,
) -> Result<Preparation, TargetError> {
    match preparation {
        Some(preparation)
            if preparation.resource() == journal.resource()
                && preparation.transaction_id() == journal.receipt().transaction_id() =>
        {
            Ok(preparation)
        }
        _ => Err(store_failure(
            store,
            ErrorCode::InvalidState,
            "transaction record has no matching preparation",
        )),
    }
}

fn known(store: &dyn StateStore, preparation: &Preparation) -> bool {
    recognized_transaction_directory(
        store,
        preparation.resource().destination(),
        preparation.transaction_id(),
        preparation.directory(),
    )
}

fn unrecognized_directory(store: &dyn StateStore) -> TargetError {
    store_failure(
        store,
        ErrorCode::RecoveryNeeded,
        "transaction record names an unrecognized directory",
    )
}

fn recognized(
    store: &dyn StateStore,
    preparation: Option<Preparation>,
) -> Result<Option<Preparation>, TargetError> {
    if let Some(selected) = &preparation
        && !known(store, selected)
    {
        return Err(unrecognized_directory(store));
    }
    Ok(preparation)
}

fn absent(directory: &Path) -> bool {
    fs::symlink_metadata(directory).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
}

fn read_preparation(
    store: &dyn StateStore,
    reference: &ResourceRef,
) -> Result<Option<Preparation>, TargetError> {
    read_record(store, &preparation_key(reference), "Preparation")
}

fn records(
    store: &dyn StateStore,
    reference: &ResourceRef,
) -> Result<(Option<Preparation>, Option<Journal>), TargetError> {
    let preparation = read_preparation(store, reference)?;
    if let Some(selected) = &preparation
        && !known(store, selected)
        && !absent(selected.directory())
    {
        return Err(unrecognized_directory(store));
    }
    let journal: Option<Journal> = read_record(store, &journal_key(reference), "Journal")?;
    if let Some(journal) = &journal {
        matching(store, recognized(store, preparation.clone())?, journal)?;
    }
    Ok((preparation, journal))
}

fn preparation_for(store: &dyn StateStore, journal: &Journal) -> Result<Preparation, TargetError> {
    let preparation = read_preparation(store, journal.resource())?;
    matching(store, recognized(store, preparation)?, journal)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn transaction_directory(
    store: &dyn StateStore,
    journal: &Journal,
) -> Result<PathBuf, TargetError> {
    preparation_for(store, journal).map(|preparation| preparation.directory().to_path_buf())
}

fn path_exists(path: &Path) -> Result<bool, TargetError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_failure(&error, path)),
    }
}

fn children(directory: &Path) -> Result<Vec<PathBuf>, TargetError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_failure(&error, directory)),
    };
    let mut result = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| io_failure(&error, directory))
        })
        .collect::<Result<Vec<_>, _>>()?;
    result.sort();
    Ok(result)
}

fn layout(directory: &Path) -> Result<(), TargetError> {
    managed_spelling(directory)?;
    for child in children(directory)? {
        let known = child
            .file_name()
            .is_some_and(|name| name == "staging" || name == "backup");
        if !known {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "unknown transaction data",
                &child,
            ));
        }
        let metadata = fs::symlink_metadata(&child).map_err(|error| io_failure(&error, &child))?;
        managed_metadata(&metadata, &child)?;
        if !metadata.is_dir() {
            return Err(failure(
                ErrorCode::UnsafePath,
                "expected a directory",
                &child,
            ));
        }
        if children(&child)?
            .iter()
            .any(|entry| entry.file_name().is_none_or(|name| name != "resource"))
        {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "unknown transaction data",
                &child,
            ));
        }
    }
    Ok(())
}

fn transaction_ancestors(
    destination: &Path,
    directory: &Path,
) -> Result<Vec<Ancestor>, TargetError> {
    let (staged, backup) = paths(directory);
    resource_ancestors(destination, &[&staged, &backup])
}

fn validate_ancestors(
    store: &dyn StateStore,
    journal: &Journal,
    directory: &Path,
) -> Result<(), TargetError> {
    let reference = journal.resource();
    let current = transaction_ancestors(reference.destination(), directory)?;
    let retained = journal
        .ancestors()
        .iter()
        .filter(|item| current.contains(item) || !item.path().starts_with(directory))
        .cloned()
        .collect::<Vec<_>>();
    if current != retained {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            reference.destination(),
        ));
    }
    for path in [
        directory.to_path_buf(),
        directory.join("staging"),
        directory.join("backup"),
    ] {
        if path_exists(&path)? {
            ensure_private(&path)?;
        }
    }
    let authority = reference.authority_checked().map_err(error_detail)?;
    let conflicts = hierarchy_conflicts(
        store,
        &authority,
        reference.kind() != ResourceKind::Document,
    )?;
    if let Some(conflict) = conflicts.first() {
        return Err(failure(
            ErrorCode::Conflict,
            "resource hierarchy reservation changed",
            conflict,
        ));
    }
    Ok(())
}

fn current_receipt(
    store: &dyn StateStore,
    journal: &Journal,
) -> Result<Option<Receipt>, TargetError> {
    read_record(store, &receipt_key(journal.resource()), "Receipt")
}

fn require_journal(
    store: &dyn StateStore,
    journal: &Journal,
    directory: &Path,
) -> Result<(), TargetError> {
    let recorded: Option<Journal> =
        read_record(store, &journal_key(journal.resource()), "Journal")?;
    if recorded.as_ref() != Some(journal) {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "transaction record changed",
            directory,
        ));
    }
    Ok(())
}

fn remove_directories(directory: &Path, ignore_failures: bool) -> Result<(), TargetError> {
    for path in [
        directory.join("staging"),
        directory.join("backup"),
        directory.to_path_buf(),
    ] {
        match fs::remove_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound || ignore_failures => {}
            Err(error) => return Err(io_failure(&error, &path)),
        }
    }
    Ok(())
}

fn release(store: &dyn StateStore, preparation: &Preparation) {
    if matches!(store.read(&journal_key(preparation.resource())), Ok(None)) {
        let _ = delete_record(store, &preparation_key(preparation.resource()), preparation);
    }
}

fn discard(directory: &Path, revisions: &[Revision]) {
    let staged = paths(directory).0;
    let destroyed = observe(&staged).and_then(|current| {
        if !current.nodes().is_empty() && revisions.contains(&current) {
            destroy(&staged, &current)
        } else {
            Ok(())
        }
    });
    if destroyed.is_ok() {
        let _ = remove_directories(directory, true);
    }
}

fn touch(directory: &Path) -> Result<(), TargetError> {
    let now = rustix::fs::Timespec {
        tv_sec: 0,
        tv_nsec: rustix::fs::UTIME_NOW,
    };
    rustix::fs::utimensat(
        rustix::fs::CWD,
        directory,
        &rustix::fs::Timestamps {
            last_access: now,
            last_modification: now,
        },
        rustix::fs::AtFlags::empty(),
    )
    .map_err(|errno| io_failure(&io::Error::from(errno), directory))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn pending_recovery(
    store: &dyn StateStore,
    reference: &ResourceRef,
) -> Result<Vec<PathBuf>, TargetError> {
    let (preparation, _) = records(store, reference)?;
    Ok(preparation
        .map(|preparation| preparation.directory().to_path_buf())
        .into_iter()
        .collect())
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Option<(&'static str, usize)>> =
        const { std::cell::RefCell::new(None) };
    static CRASH: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(feature = "interop-test-seams")]
#[derive(Clone)]
enum InteropTestSeam {
    Barrier {
        boundary: String,
        ready: PathBuf,
        release: PathBuf,
        timeout: Duration,
    },
    Fault {
        boundary: String,
    },
}

#[cfg(feature = "interop-test-seams")]
fn interop_test_seam() -> &'static Mutex<Option<InteropTestSeam>> {
    static SEAM: OnceLock<Mutex<Option<InteropTestSeam>>> = OnceLock::new();
    SEAM.get_or_init(|| Mutex::new(None))
}

#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub fn set_interop_test_barrier(
    boundary: String,
    ready: PathBuf,
    release: PathBuf,
    timeout: Duration,
) -> Result<(), String> {
    let mut seam = interop_test_seam()
        .lock()
        .map_err(|_| String::from("interop test seam lock is poisoned"))?;
    *seam = Some(InteropTestSeam::Barrier {
        boundary,
        ready,
        release,
        timeout,
    });
    drop(seam);
    Ok(())
}

#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub fn set_interop_test_fault(boundary: String) -> Result<(), String> {
    let mut seam = interop_test_seam()
        .lock()
        .map_err(|_| String::from("interop test seam lock is poisoned"))?;
    *seam = Some(InteropTestSeam::Fault { boundary });
    drop(seam);
    Ok(())
}

#[cfg(test)]
#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn set_fault(boundary: Option<&'static str>) {
    FAULT.with(|fault| *fault.borrow_mut() = boundary.map(|value| (value, 0)));
}

#[cfg(test)]
#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn set_fault_after(boundary: &'static str, successful_calls: usize) {
    FAULT.with(|fault| *fault.borrow_mut() = Some((boundary, successful_calls)));
}

#[cfg(test)]
#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn set_crash(boundary: Option<&'static str>) {
    CRASH.with(|crash| crash.set(boundary));
}

#[cfg(any(test, feature = "interop-test-seams"))]
fn inject(boundary: &'static str) -> Result<(), TargetError> {
    #[cfg(test)]
    crate::seam::fire(&crate::seam::Seam::Boundary(boundary));
    #[cfg(test)]
    if CRASH.with(|crash| crash.get() == Some(boundary)) {
        CRASH.with(|crash| crash.set(None));
        std::panic::resume_unwind(Box::new(format!("interrupted at {boundary}")));
    }
    #[cfg(test)]
    if FAULT.with(|fault| {
        let mut fault = fault.borrow_mut();
        let Some((selected, remaining)) = fault.as_mut() else {
            return false;
        };
        if *selected != boundary {
            return false;
        }
        if *remaining > 0 {
            *remaining -= 1;
            return false;
        }
        true
    }) {
        return Err(TargetError::new(
            ErrorCode::IoError,
            format!("injected {boundary} fault"),
            None,
            Some(libc::EIO),
        ));
    }
    #[cfg(feature = "interop-test-seams")]
    {
        let selected = interop_test_seam()
            .lock()
            .map_err(|_| {
                TargetError::new(
                    ErrorCode::IoError,
                    "interop test seam lock is poisoned",
                    None,
                    Some(libc::EIO),
                )
            })?
            .clone();
        match selected {
            Some(InteropTestSeam::Fault { boundary: selected }) if selected == boundary => {
                return Err(TargetError::new(
                    ErrorCode::IoError,
                    format!("injected {boundary} fault"),
                    None,
                    Some(libc::EIO),
                ));
            }
            Some(InteropTestSeam::Barrier {
                boundary: selected,
                ready,
                release,
                timeout,
            }) if selected == boundary => {
                fs::write(&ready, b"ready\n").map_err(|error| io_failure(&error, &ready))?;
                let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
                    failure(
                        ErrorCode::InvalidState,
                        "interop barrier timeout is too large",
                        &ready,
                    )
                })?;
                while !release.exists() {
                    if Instant::now() >= deadline {
                        return Err(failure(
                            ErrorCode::IoError,
                            "interop barrier timed out",
                            &release,
                        ));
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(not(any(test, feature = "interop-test-seams")))]
#[allow(clippy::unnecessary_wraps)]
const fn inject(_boundary: &'static str) -> Result<(), TargetError> {
    Ok(())
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepare(
    store: &dyn StateStore,
    plan: &ResourcePlan,
    guard: &ResourceLockGuard<'_>,
) -> Result<Journal, TargetError> {
    let reference = plan.resource();
    let destination = reference.destination();
    if guard.reference() != reference {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource lock disagrees",
            destination,
        ));
    }
    let directory = plan.transaction_directory();
    if !recognized_transaction_directory(
        store,
        destination,
        plan.receipt().transaction_id(),
        directory,
    ) {
        return Err(TargetError::new(
            ErrorCode::InvalidState,
            "planned transaction directory is not recognized",
            None,
            None,
        ));
    }
    if resource_ancestors(destination, &[&directory.join("staging")])? != plan.ancestors() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            destination,
        ));
    }
    require(destination, plan.before())?;
    if plan.require_existing() && plan.before().nodes().is_empty() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "required existing resource disappeared",
            destination,
        ));
    }
    let receipt: Option<Receipt> = read_record(store, &receipt_key(reference), "Receipt")?;
    if receipt.as_ref() != plan.previous() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "receipt changed during preparation",
            destination,
        ));
    }
    if let (Some(retained), _) = records(store, reference)? {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "transaction record retained",
            retained.directory(),
        ));
    }
    let preparation = Preparation::new(
        reference.clone(),
        plan.receipt().transaction_id().to_owned(),
        directory.to_path_buf(),
    )
    .map_err(|error| invalid(&error, destination))?;
    write_record(store, &preparation_key(reference), &preparation, None)?;
    inject("preparation")?;
    match fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            release(store, &preparation);
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "unrecognized transaction data retained",
                directory,
            ));
        }
        Err(error) => {
            release(store, &preparation);
            return Err(io_failure(&error, directory));
        }
    }
    let mut staged = Vec::new();
    match stage(store, plan, directory, &mut staged) {
        Ok(journal) => {
            inject("prepare")?;
            Ok(journal)
        }
        Err(error) => {
            discard(directory, &staged);
            if !path_exists(directory).unwrap_or(true) {
                release(store, &preparation);
            }
            Err(error)
        }
    }
}

fn stage(
    store: &dyn StateStore,
    plan: &ResourcePlan,
    directory: &Path,
    staged: &mut Vec<Revision>,
) -> Result<Journal, TargetError> {
    let storage_ancestors = storage(plan, directory)?;
    inject("directory")?;
    if plan.after() != plan.before() {
        let written = write_revision(&paths(directory).0, plan.after())?;
        inject("staged")?;
        staged.push(written);
    }
    let revision = staged
        .first()
        .cloned()
        .unwrap_or_else(|| plan.before().clone());
    let journal = journal_for(store, plan, directory, storage_ancestors, revision)?;
    *staged = vec![journal.staged().clone(), journal.published().clone()];
    touch(directory)?;
    inject("journal")?;
    write_record(store, &journal_key(plan.resource()), &journal, None)?;
    Ok(journal)
}

fn storage(plan: &ResourcePlan, directory: &Path) -> Result<Vec<Ancestor>, TargetError> {
    let destination = plan.resource().destination();
    ensure_private(directory)?;
    let created = [
        directory.to_path_buf(),
        directory.join("staging"),
        directory.join("backup"),
    ];
    let mut storage = Vec::new();
    for path in created.iter().skip(1) {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| io_failure(&error, path))?;
        let expected = observe(path)?;
        ensure_private(path)?;
        storage.push((path, expected));
    }
    let storage_ancestors = transaction_ancestors(destination, directory)?;
    for (path, expected) in &storage {
        require(path, expected)?;
    }
    let retained = storage_ancestors
        .iter()
        .filter(|item| !created.iter().any(|path| path == item.path()))
        .cloned()
        .collect::<Vec<_>>();
    if retained != plan.ancestors() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            destination,
        ));
    }
    Ok(storage_ancestors)
}

fn journal_for(
    store: &dyn StateStore,
    plan: &ResourcePlan,
    directory: &Path,
    storage_ancestors: Vec<Ancestor>,
    staged: Revision,
) -> Result<Journal, TargetError> {
    let reference = plan.resource();
    let destination = reference.destination();
    let receipt = plan
        .receipt()
        .with_created_directories(
            plan.receipt()
                .created_directories()
                .iter()
                .filter_map(|marker| {
                    staged
                        .nodes()
                        .iter()
                        .find(|node| node.path() == marker.path())
                        .cloned()
                })
                .collect(),
        )
        .map_err(|error| invalid(&error, destination))?;
    let journal = Journal::new(
        reference.clone(),
        plan.previous().cloned(),
        receipt,
        plan.before().clone(),
        staged.clone(),
        staged,
        plan.before().clone(),
        storage_ancestors,
        2,
    )
    .map_err(|error| invalid(&error, destination))?;
    validate_ancestors(store, &journal, directory)?;
    require(destination, plan.before())?;
    Ok(journal)
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn publish_receipt(
    store: &dyn StateStore,
    journal: &Journal,
) -> Result<(), TargetError> {
    publish_receipt_with(store, journal, || {})
}

fn publish_receipt_with(
    store: &dyn StateStore,
    journal: &Journal,
    committed: impl FnOnce(),
) -> Result<(), TargetError> {
    inject("publish-receipt")?;
    let directory = transaction_directory(store, journal)?;
    require_journal(store, journal, &directory)?;
    layout(&directory)?;
    validate_ancestors(store, journal, &directory)?;
    let (staged, backup) = paths(&directory);
    let empty = Revision::empty();
    require(&staged, &empty)?;
    require(journal.resource().destination(), journal.published())?;
    require(
        &backup,
        if journal.before() == journal.staged() {
            &empty
        } else {
            journal.backup()
        },
    )?;
    if current_receipt(store, journal)?.as_ref() != journal.previous() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "receipt changed before commit",
            journal.resource().destination(),
        ));
    }
    inject("receipt")?;
    write_record(
        store,
        &receipt_key(journal.resource()),
        journal.receipt(),
        journal.previous(),
    )?;
    committed();
    inject("receipt-reread")?;
    inject("receipt-committed")
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn publish(store: &dyn StateStore, journal: &Journal) -> Result<(), TargetError> {
    publish_with(store, journal, || {})
}

fn publish_with(
    store: &dyn StateStore,
    journal: &Journal,
    committed: impl FnOnce(),
) -> Result<(), TargetError> {
    let reference = journal.resource();
    let destination = reference.destination();
    let directory = transaction_directory(store, journal)?;
    let (staged, backup) = paths(&directory);
    require_journal(store, journal, &directory)?;
    validate_ancestors(store, journal, &directory)?;
    require(destination, journal.before())?;
    if journal.before() != journal.staged() {
        require(&staged, journal.staged())?;
        if !journal.before().nodes().is_empty() {
            inject("backup-move")?;
            move_revision(destination, &backup, journal.backup())?;
            inject("backup")?;
            inject("rename")?;
            require(&backup, journal.backup())?;
        }
        if !journal.staged().nodes().is_empty() {
            inject("staging")?;
            inject("rename")?;
            move_revision(&staged, destination, journal.staged())?;
        }
        require(destination, journal.published())?;
    }
    inject("publish")?;
    validate_ancestors(store, journal, &directory)?;
    require(destination, journal.published())?;
    publish_receipt_with(store, journal, committed)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn cleanup(store: &dyn StateStore, journal: &Journal) -> Result<(), TargetError> {
    let preparation = preparation_for(store, journal)?;
    let directory = preparation.directory();
    require_journal(store, journal, directory)?;
    layout(directory)?;
    validate_ancestors(store, journal, directory)?;
    let (staged, backup) = paths(directory);
    for (path, expected) in [
        (&staged, [journal.staged(), journal.published()]),
        (&backup, [journal.backup(), journal.before()]),
    ] {
        let current = observe(path)?;
        if !current.nodes().is_empty() {
            if !expected
                .iter()
                .any(|value| cleanup_revision(&current, value))
            {
                return Err(failure(
                    ErrorCode::RecoveryNeeded,
                    "unexpected cleanup revision",
                    path,
                ));
            }
            destroy(path, &current)?;
        }
    }
    remove_directories(directory, false)?;
    inject("cleanup")?;
    delete_record(store, &journal_key(journal.resource()), journal)?;
    delete_record(store, &preparation_key(journal.resource()), &preparation)
}

fn preparation_time(directory: &Path) -> Result<SystemTime, TargetError> {
    fs::symlink_metadata(directory)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| io_failure(&error, directory))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn rollback(store: &dyn StateStore, journal: &Journal) -> Result<(), TargetError> {
    let reference = journal.resource();
    let destination = reference.destination();
    let directory = transaction_directory(store, journal)?;
    let (staged_path, backup_path) = paths(&directory);
    layout(&directory)?;
    validate_ancestors(store, journal, &directory)?;
    let actual = observe(destination)?;
    let staged = observe(&staged_path)?;
    let backup = observe(&backup_path)?;
    // Only cleanup, after this check passed, removes the directory or moves its time.
    if !actual.nodes().is_empty()
        && (actual == *journal.before()
            || actual == *journal.staged()
            || actual == *journal.published())
        && path_exists(&directory)?
    {
        require_unchanged_since(destination, &actual, preparation_time(&directory)?)?;
    }
    if actual == *journal.before() && backup.nodes().is_empty() {
        inject("rollback")?;
        return cleanup(store, journal);
    }
    if !staged.nodes().is_empty() && staged != *journal.staged() && staged != *journal.published() {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "unexpected staged revision",
            &staged_path,
        ));
    }
    if !backup.nodes().is_empty() && backup != *journal.before() && backup != *journal.backup() {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "unexpected backup revision",
            &backup_path,
        ));
    }
    if journal.before() == journal.staged() {
        require(destination, journal.before())?;
    } else if !backup.nodes().is_empty() {
        if !actual.nodes().is_empty() {
            if (actual != *journal.published() && actual != *journal.staged())
                || !staged.nodes().is_empty()
            {
                return Err(failure(
                    ErrorCode::RecoveryNeeded,
                    "unrecognized published revision",
                    destination,
                ));
            }
            move_revision(destination, &staged_path, &actual)?;
        }
        move_revision(&backup_path, destination, &backup)?;
        require(destination, journal.before())?;
    } else if actual == *journal.before() {
    } else if journal.before().nodes().is_empty()
        && actual == *journal.published()
        && staged.nodes().is_empty()
    {
        move_revision(destination, &staged_path, &actual)?;
    } else {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "cannot restore complete previous revision",
            destination,
        ));
    }
    inject("rollback")?;
    cleanup(store, journal)
}

fn abandon(store: &dyn StateStore, preparation: &Preparation) -> Result<(), TargetError> {
    let directory = preparation.directory();
    let (staged, backup) = paths(directory);
    if path_exists(directory)? {
        if !known(store, preparation) {
            return Err(unrecognized_directory(store));
        }
        let unrecognized = || {
            failure(
                ErrorCode::RecoveryNeeded,
                "unrecognized transaction data retained",
                directory,
            )
        };
        let checked = (|| {
            layout(directory)?;
            for path in [
                directory.to_path_buf(),
                directory.join("staging"),
                directory.join("backup"),
            ] {
                if path_exists(&path)? {
                    ensure_private(&path)?;
                }
            }
            Ok::<_, TargetError>((observe(&backup)?, observe(&staged)?))
        })();
        let (retained, current) = match checked {
            Ok(value) => value,
            Err(error) if error.errno().is_none_or(|errno| errno == libc::ENOTSUP) => {
                return Err(unrecognized());
            }
            Err(error) => return Err(error),
        };
        if !retained.nodes().is_empty() {
            return Err(unrecognized());
        }
        destroy(&staged, &current)?;
        remove_directories(directory, false)?;
    }
    delete_record(store, &preparation_key(preparation.resource()), preparation)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn recover(
    store: &dyn StateStore,
    guard: &ResourceLockGuard<'_>,
) -> Result<(), TargetError> {
    let reference = guard.reference();
    let (preparation, journal) = records(store, reference)?;
    let Some(preparation) = preparation else {
        return Ok(());
    };
    if preparation.resource() != reference {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "transaction record disagrees",
            preparation.directory(),
        ));
    }
    let Some(journal) = journal else {
        return abandon(store, &preparation);
    };
    if journal.previous() == Some(journal.receipt()) {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "transaction record repeats the previous receipt",
            preparation.directory(),
        ));
    }
    let current = current_receipt(store, &journal)?;
    if current.as_ref() == Some(journal.receipt()) {
        cleanup(store, &journal)
    } else if current.as_ref() == journal.previous() {
        rollback(store, &journal)
    } else {
        Err(failure(
            ErrorCode::RecoveryNeeded,
            "receipt disagrees with transaction",
            reference.destination(),
        ))
    }
}

impl Journal {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        resource: ResourceRef,
        previous: Option<Receipt>,
        receipt: Receipt,
        before: Revision,
        staged: Revision,
        published: Revision,
        backup: Revision,
        ancestors: Vec<Ancestor>,
        schema_version: i64,
    ) -> Result<Self, ValueError> {
        if receipt.resource() != &resource
            || previous
                .as_ref()
                .is_some_and(|receipt| receipt.resource() != &resource)
        {
            return Err(ValueError::new("journal receipt resources disagree"));
        }
        if schema_version != 2 {
            return Err(ValueError::new("journal schema must be 2"));
        }
        if before.content_digest() != backup.content_digest()
            || staged.content_digest() != published.content_digest()
        {
            return Err(ValueError::new(
                "journal private revisions must retain complete content",
            ));
        }
        Ok(Self {
            resource,
            previous,
            receipt,
            before,
            staged,
            published,
            backup,
            ancestors,
            schema_version,
        })
    }

    pub(crate) const fn resource(&self) -> &ResourceRef {
        &self.resource
    }

    pub(crate) const fn previous(&self) -> Option<&Receipt> {
        self.previous.as_ref()
    }

    pub(crate) const fn receipt(&self) -> &Receipt {
        &self.receipt
    }

    pub(crate) const fn before(&self) -> &Revision {
        &self.before
    }

    pub(crate) const fn staged(&self) -> &Revision {
        &self.staged
    }

    pub(crate) const fn published(&self) -> &Revision {
        &self.published
    }

    pub(crate) const fn backup(&self) -> &Revision {
        &self.backup
    }

    pub(crate) fn ancestors(&self) -> &[Ancestor] {
        &self.ancestors
    }

    pub(crate) const fn schema_version(&self) -> i64 {
        self.schema_version
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::authority::ResourceAuthority;
    use crate::editors::DocumentProvenance;
    use crate::resource_io::admit;
    use crate::resource_models::{Count, Node, ResourceKind};
    use crate::stores::MemoryStore;
    use crate::test_support::checkout_temp_dir;
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
    use std::path::Path;

    fn index_key() -> String {
        crate::state::index_key("team", &"a".repeat(64))
    }

    fn reference(destination: &Path) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::Document,
        )
        .expect("reference")
    }

    fn container_reference(destination: &Path) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::SkillContainer,
        )
        .expect("reference")
    }

    fn journal_record(store: &dyn StateStore, reference: &ResourceRef) -> Option<Vec<u8>> {
        store.read(&journal_key(reference)).expect("journal read")
    }

    fn stored_receipt(store: &dyn StateStore, reference: &ResourceRef) -> Option<Receipt> {
        read_record(store, &receipt_key(reference), "Receipt").expect("receipt read")
    }

    fn put_receipt(store: &dyn StateStore, reference: &ResourceRef, receipt: &Receipt) {
        write_record(store, &receipt_key(reference), receipt, None).expect("receipt record");
    }

    fn transaction_entries(reference: &ResourceRef) -> Vec<PathBuf> {
        let parent = reference.destination().parent().expect("parent");
        fs::read_dir(parent)
            .expect("listing")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| crate::authority::management_name(&name.to_string_lossy()))
            })
            .collect()
    }

    fn revision(data: &[u8]) -> Revision {
        Revision::new(vec![
            Node::new(
                String::new(),
                Some(data.to_vec()),
                Count::Small(0o600),
                Count::Small(i64::from(unsafe { libc::geteuid() })),
                Count::Small(i64::from(unsafe { libc::getegid() })),
                Count::Small(0),
                Count::Small(0),
                Vec::new(),
            )
            .expect("node"),
        ])
        .expect("revision")
    }

    fn exclusive_write(path: &Path, data: &[u8], mode: u32) {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .expect("exclusive file");
        file.write_all(data).expect("file contents");
    }

    fn fixture_revision_bytes(revision: &crate::json::Json) -> Option<Vec<u8>> {
        let hex = revision
            .get("fields")?
            .get("nodes")?
            .as_array()?
            .first()?
            .get("fields")?
            .get("data")?
            .get("bytes")?
            .as_str()?;
        Some(crate::test_support::from_hex(hex))
    }

    fn fixture_transaction_revision(
        case: &crate::json::Json,
        phase: &str,
        location: &str,
    ) -> Option<Vec<u8>> {
        fixture_revision_bytes(case.get(phase)?.get(location)?)
    }

    fn receipt(reference: &ResourceRef, transaction: char) -> Receipt {
        Receipt::new(
            reference.clone(),
            transaction.to_string().repeat(32),
            Vec::new(),
            DocumentProvenance::default(),
            Vec::new(),
            2,
        )
        .expect("receipt")
    }

    fn plan(
        store: &dyn StateStore,
        reference: &ResourceRef,
        before: Revision,
        after: Revision,
        previous: Option<Receipt>,
        next: Receipt,
    ) -> ResourcePlan {
        let directory = crate::state::choose_transaction_directory(
            store,
            reference.destination(),
            next.transaction_id(),
        )
        .expect("transaction directory");
        ResourcePlan::new(
            reference.clone(),
            before,
            after,
            previous,
            next,
            resource_ancestors(reference.destination(), &[&directory.join("staging")])
                .expect("ancestors"),
            directory,
            pending_recovery(store, reference).expect("recovery"),
            false,
            None,
        )
        .expect("plan")
    }

    #[test]
    fn prepare_publish_and_cleanup_commit_a_resource_and_receipt() {
        let tmp = checkout_temp_dir("transaction-commit");
        let reference = reference(&tmp.join("AGENTS.md"));
        let store = MemoryStore::new();
        admit(&store, &reference).expect("admit");
        let before = observe(reference.destination()).expect("before");
        let next = receipt(&reference, '1');
        let plan = plan(
            &store,
            &reference,
            before,
            revision(b"generated\n"),
            None,
            next.clone(),
        );
        let guard = ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");
        let journal = prepare(&store, &plan, &guard).expect("prepare");
        assert!(journal_record(&store, &reference).is_some());
        publish(&store, &journal).expect("publish");
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
        assert_eq!(stored_receipt(&store, &reference), Some(next));
        cleanup(&store, &journal).expect("cleanup");
        assert_eq!(
            pending_recovery(&store, &reference).expect("state"),
            [] as [PathBuf; 0]
        );
        assert_eq!(transaction_entries(&reference), [] as [PathBuf; 0]);
    }

    #[test]
    fn recover_rolls_back_prepared_changes_and_exact_prefix_cleanup() {
        let tmp = checkout_temp_dir("transaction-rollback");
        let destination = tmp.join("AGENTS.md");
        fs::write(&destination, b"before\n").expect("foreign");
        let reference = reference(&destination);
        let store = MemoryStore::new();
        admit(&store, &reference).expect("admit");
        let before = observe(&destination).expect("before");
        let next = receipt(&reference, '2');
        let plan = plan(
            &store,
            &reference,
            before.clone(),
            revision(b"after\n"),
            None,
            next,
        );
        let guard = ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");
        let journal = prepare(&store, &plan, &guard).expect("prepare");
        let staged_path = paths(plan.transaction_directory()).0;
        let staged = observe(&staged_path).expect("staged");
        assert!(cleanup_revision(&staged, journal.staged()));
        recover(&store, &guard).expect("recover");
        assert_eq!(observe(&destination).expect("restored"), before);
        assert_eq!(
            pending_recovery(&store, &reference).expect("state"),
            [] as [PathBuf; 0]
        );
        assert_eq!(transaction_entries(&reference), [] as [PathBuf; 0]);
    }

    fn boundary(directory: &Path) -> SystemTime {
        fs::symlink_metadata(directory)
            .and_then(|metadata| metadata.modified())
            .expect("transaction directory modification time")
    }

    fn revise_modification_time(path: &Path, later: SystemTime) {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for revised time");
        file.set_times(fs::FileTimes::new().set_modified(later))
            .expect("revised modification time");
    }

    #[test]
    fn recover_refuses_a_replacement_of_the_published_revision() {
        let tmp = checkout_temp_dir("transaction-published-replacement");
        let destination = tmp.join("AGENTS.md");
        exclusive_write(&destination, b"before\n", 0o600);
        let reference = reference(&destination);
        let store = MemoryStore::new();
        admit(&store, &reference).expect("admit");
        let old = receipt(&reference, '0');
        put_receipt(&store, &reference, &old);
        let before = observe(&destination).expect("before");
        let plan = plan(
            &store,
            &reference,
            before,
            revision(b"after\n"),
            Some(old.clone()),
            receipt(&reference, '1'),
        );
        let guard = ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");
        let journal = prepare(&store, &plan, &guard).expect("prepare");
        move_revision(
            &destination,
            &paths(plan.transaction_directory()).1,
            journal.backup(),
        )
        .expect("backup move");
        move_revision(
            &paths(plan.transaction_directory()).0,
            &destination,
            journal.staged(),
        )
        .expect("publish move");
        // Recycled inodes need the post-preparation identity check.
        revise_modification_time(
            &destination,
            boundary(plan.transaction_directory())
                .checked_add(Duration::from_secs(60))
                .expect("replacement time"),
        );
        let error = recover(&store, &guard).expect_err("recovery refuses the replacement");
        assert_eq!(error.code(), ErrorCode::RecoveryNeeded);
        assert_eq!(
            error.message(),
            "destination revision changed after preparation"
        );
        assert_eq!(error.path(), Some(destination.as_path()));
        assert_eq!(fs::read(&destination).expect("destination"), b"after\n");
        assert_eq!(
            stored_receipt(&store, &reference).expect("receipt present"),
            old,
            "the previous receipt is retained"
        );
        assert!(
            journal_record(&store, &reference).is_some(),
            "journal is retained"
        );
        assert!(
            paths(plan.transaction_directory()).1.exists(),
            "backup is retained"
        );
    }

    #[test]
    fn recover_refuses_a_replacement_of_the_previous_revision() {
        let tmp = checkout_temp_dir("transaction-previous-replacement");
        let destination = tmp.join("AGENTS.md");
        exclusive_write(&destination, b"before\n", 0o600);
        let reference = reference(&destination);
        let store = MemoryStore::new();
        admit(&store, &reference).expect("admit");
        let old = receipt(&reference, '0');
        put_receipt(&store, &reference, &old);
        let before = observe(&destination).expect("before");
        let plan = plan(
            &store,
            &reference,
            before,
            revision(b"after\n"),
            Some(old),
            receipt(&reference, '1'),
        );
        let guard = ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");
        prepare(&store, &plan, &guard).expect("prepare");
        revise_modification_time(
            &destination,
            boundary(plan.transaction_directory())
                .checked_add(Duration::from_secs(60))
                .expect("replacement time"),
        );
        let error = recover(&store, &guard).expect_err("recovery refuses the replacement");
        assert_eq!(error.code(), ErrorCode::RecoveryNeeded);
        assert_eq!(
            error.message(),
            "destination revision changed after preparation"
        );
        assert_eq!(fs::read(&destination).expect("destination"), b"before\n");
        assert!(
            journal_record(&store, &reference).is_some(),
            "journal is retained"
        );
        assert!(
            paths(plan.transaction_directory()).0.exists(),
            "staging is retained"
        );
    }

    #[test]
    fn resource_locks_are_acquired_in_destination_order() {
        let tmp = checkout_temp_dir("transaction-lock-order");
        let store = MemoryStore::new();
        let first = reference(&tmp.join("z"));
        let second = reference(&tmp.join("a"));
        let guards = lock_resources(&store, &[first.clone(), second.clone()], Duration::ZERO)
            .expect("locks");
        let order: Vec<_> = guards.iter().map(ResourceLockGuard::reference).collect();
        assert_eq!(order, [&second, &first]);
        assert_eq!(
            ResourceLockGuard::acquire(&store, &first, Duration::ZERO)
                .err()
                .map(|error| error.code()),
            Some(ErrorCode::Busy)
        );
    }

    #[test]
    fn revision_observation_captures_physical_identity() {
        let tmp = checkout_temp_dir("transaction-observe");
        let path = tmp.join("file");
        fs::write(&path, b"x").expect("file");
        let revision = observe(&path).expect("revision");
        let metadata = fs::symlink_metadata(path).expect("metadata");
        assert_eq!(revision.nodes()[0].device().as_u64(), Some(metadata.dev()));
        assert_eq!(revision.nodes()[0].inode().as_u64(), Some(metadata.ino()));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn all_shared_transaction_vectors_drive_guarded_recovery() {
        let Some(fixture) = crate::test_support::fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let cases = fixture
            .get("transactions")
            .and_then(crate::json::Json::as_array)
            .expect("transaction vectors");
        let names = cases
            .iter()
            .filter_map(|case| case.get("name").and_then(crate::json::Json::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "prepared",
                "backup-moved",
                "published-before-receipt",
                "ownership-only-adoption",
                "receipt-committed",
                "foreign-edit-before-commit",
                "permissions-changed-before-commit",
                "identity-changed-before-commit",
                "committed-foreign-edit",
            ]
        );
        let prepared = cases
            .iter()
            .find(|case| case.get("name").and_then(crate::json::Json::as_str) == Some("prepared"))
            .expect("prepared vector");
        let foreign = fixture_transaction_revision(prepared, "observed", "resource")
            .expect("foreign fixture bytes");
        let generated = fixture_transaction_revision(prepared, "observed", "staged")
            .expect("generated fixture bytes");

        for case in cases {
            let name = case
                .get("name")
                .and_then(crate::json::Json::as_str)
                .expect("transaction name");
            let tmp = checkout_temp_dir(&format!("transaction-vector-{name}"));
            let destination = tmp.join("AGENTS.md");
            let initial = if case.get("journal").and_then(crate::json::Json::as_str)
                == Some("adopt-section")
            {
                fixture_transaction_revision(case, "observed", "resource")
                    .expect("adoption resource bytes")
            } else {
                foreign.clone()
            };
            exclusive_write(&destination, &initial, 0o600);
            let reference = reference(&destination);
            let store = MemoryStore::new();
            admit(&store, &reference).expect("admit");
            let old = receipt(&reference, '0');
            put_receipt(&store, &reference, &old);
            let before = observe(&destination).expect("before");
            let next = receipt(&reference, '1');
            let after = if name == "ownership-only-adoption" {
                before.clone()
            } else {
                revision(&generated)
            };
            let plan = plan(
                &store,
                &reference,
                before.clone(),
                after,
                Some(old.clone()),
                next.clone(),
            );
            let guard =
                ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");
            let journal = prepare(&store, &plan, &guard).expect("prepare");

            match name {
                "prepared" | "ownership-only-adoption" => {}
                "backup-moved" => {
                    move_revision(
                        &destination,
                        &paths(plan.transaction_directory()).1,
                        journal.backup(),
                    )
                    .expect("backup move");
                }
                "published-before-receipt"
                | "foreign-edit-before-commit"
                | "permissions-changed-before-commit"
                | "identity-changed-before-commit" => {
                    move_revision(
                        &destination,
                        &paths(plan.transaction_directory()).1,
                        journal.backup(),
                    )
                    .expect("backup move");
                    move_revision(
                        &paths(plan.transaction_directory()).0,
                        &destination,
                        journal.staged(),
                    )
                    .expect("publish move");
                    match name {
                        "foreign-edit-before-commit" => {
                            fs::write(
                                &destination,
                                fixture_transaction_revision(case, "observed", "resource")
                                    .expect("foreign fixture edit"),
                            )
                            .expect("foreign edit");
                        }
                        "permissions-changed-before-commit" => {
                            let mode = case
                                .get("observed")
                                .and_then(|value| value.get("resource"))
                                .and_then(|value| value.get("fields"))
                                .and_then(|value| value.get("nodes"))
                                .and_then(crate::json::Json::as_array)
                                .and_then(|nodes| nodes.first())
                                .and_then(|value| value.get("fields"))
                                .and_then(|value| value.get("mode"))
                                .and_then(crate::json::Json::as_int)
                                .and_then(|value| u32::try_from(value).ok())
                                .expect("fixture mode");
                            fs::set_permissions(&destination, fs::Permissions::from_mode(mode))
                                .expect("mode change");
                        }
                        "identity-changed-before-commit" => {
                            // Created before the swap so ext4 cannot hand back the old inode.
                            let replacement = tmp.join("AGENTS.md.replacement");
                            exclusive_write(
                                &replacement,
                                &fixture_transaction_revision(case, "observed", "resource")
                                    .expect("replacement fixture bytes"),
                                0o600,
                            );
                            fs::rename(&replacement, &destination).expect("replace destination");
                        }
                        _ => {}
                    }
                }
                "receipt-committed" | "committed-foreign-edit" => {
                    publish(&store, &journal).expect("publish and commit");
                    if name == "committed-foreign-edit" {
                        fs::write(
                            &destination,
                            fixture_transaction_revision(case, "observed", "resource")
                                .expect("committed fixture edit"),
                        )
                        .expect("foreign edit");
                    }
                }
                _ => panic!("unknown fixture transaction"),
            }

            let expected = case.get("expected").expect("expected transaction");
            let expected_failure =
                expected.get("status").and_then(crate::json::Json::as_str) == Some("INCOMPLETE");
            let recovered = recover(&store, &guard);
            assert_eq!(recovered.is_err(), expected_failure, "{name}");
            assert_eq!(
                journal_record(&store, &reference).is_some(),
                expected
                    .get("retain_journal")
                    .and_then(crate::json::Json::as_bool)
                    .expect("retain journal"),
                "{name} journal retention"
            );
            assert_eq!(
                paths(plan.transaction_directory()).1.exists(),
                expected
                    .get("retain_backup")
                    .and_then(crate::json::Json::as_bool)
                    .expect("retain backup"),
                "{name} backup retention"
            );
            let expected_data = fixture_transaction_revision(case, "expected", "resource")
                .expect("expected fixture bytes");
            assert_eq!(
                fs::read(&destination).expect("destination"),
                expected_data,
                "{name} destination"
            );
            let retained = stored_receipt(&store, &reference).expect("receipt present");
            assert_eq!(
                retained,
                if expected.get("receipt").and_then(crate::json::Json::as_str) == Some("new") {
                    next
                } else {
                    old
                },
                "{name} receipt"
            );
        }
    }

    #[test]
    fn every_fault_seam_recovers_to_a_recognized_revision() {
        for seam in [
            "prepare",
            "backup",
            "staging",
            "rename",
            "publish",
            "receipt",
            "receipt-reread",
            "cleanup",
            "rollback",
        ] {
            let tmp = checkout_temp_dir(&format!("transaction-fault-{seam}"));
            let destination = tmp.join("AGENTS.md");
            exclusive_write(&destination, b"before\n", 0o600);
            let reference = reference(&destination);
            let store = MemoryStore::new();
            admit(&store, &reference).expect("admit");
            let old = receipt(&reference, '0');
            put_receipt(&store, &reference, &old);
            let before = observe(&destination).expect("before");
            let plan = plan(
                &store,
                &reference,
                before,
                revision(b"after\n"),
                Some(old.clone()),
                receipt(&reference, '1'),
            );
            let guard =
                ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");

            let journal = if seam == "prepare" {
                set_fault(Some(seam));
                assert!(prepare(&store, &plan, &guard).is_err());
                None
            } else {
                Some(prepare(&store, &plan, &guard).expect("prepare"))
            };
            if let Some(journal) = &journal {
                set_fault(Some(seam));
                if seam == "rollback" {
                    assert!(recover(&store, &guard).is_err());
                } else {
                    let result = publish(&store, journal).and_then(|()| cleanup(&store, journal));
                    assert!(result.is_err());
                }
            }
            set_fault(None);
            recover(&store, &guard).expect("recover after seam");
            recover(&store, &guard).expect("idempotent recovery");
            assert_eq!(
                pending_recovery(&store, &reference).expect("state"),
                [] as [PathBuf; 0]
            );
            assert_eq!(transaction_entries(&reference), [] as [PathBuf; 0]);
            let committed = matches!(seam, "receipt-reread" | "cleanup");
            assert_eq!(
                fs::read(&destination).expect("destination"),
                if committed {
                    b"after\n".as_slice()
                } else {
                    b"before\n".as_slice()
                },
                "{seam}"
            );
        }
    }

    #[test]
    fn journal_binds_created_directories_to_staged_physical_nodes() {
        let tmp = checkout_temp_dir("transaction-created-directories");
        let reference = container_reference(&tmp.join("skills"));
        let store = MemoryStore::new();
        admit(&store, &reference).expect("admit");
        let before = Revision::empty();
        let after = Revision::new(vec![
            Node::new(
                String::new(),
                None,
                Count::Small(0o755),
                Count::Small(i64::from(unsafe { libc::geteuid() })),
                Count::Small(i64::from(unsafe { libc::getegid() })),
                Count::Small(0),
                Count::Small(0),
                Vec::new(),
            )
            .expect("root"),
            Node::new(
                "review".to_owned(),
                None,
                Count::Small(0o755),
                Count::Small(i64::from(unsafe { libc::geteuid() })),
                Count::Small(i64::from(unsafe { libc::getegid() })),
                Count::Small(0),
                Count::Small(0),
                Vec::new(),
            )
            .expect("directory"),
            Node::new(
                "review/SKILL.md".to_owned(),
                Some(b"# Review\n".to_vec()),
                Count::Small(0o644),
                Count::Small(i64::from(unsafe { libc::geteuid() })),
                Count::Small(i64::from(unsafe { libc::getegid() })),
                Count::Small(0),
                Count::Small(0),
                Vec::new(),
            )
            .expect("file"),
        ])
        .expect("revision");
        let marker = after.nodes()[1].clone();
        let next = Receipt::new(
            reference.clone(),
            "33333333333333333333333333333333".to_owned(),
            Vec::new(),
            DocumentProvenance::default(),
            vec![marker.clone()],
            2,
        )
        .expect("receipt");
        let plan = plan(&store, &reference, before, after, None, next);
        let guard = ResourceLockGuard::acquire(&store, &reference, Duration::ZERO).expect("lock");
        let journal = prepare(&store, &plan, &guard).expect("prepare");
        let physical = journal
            .staged()
            .nodes()
            .iter()
            .find(|node| node.path() == "review")
            .expect("physical marker");
        assert_eq!(
            journal.receipt().created_directories(),
            std::slice::from_ref(physical)
        );
        assert_ne!(physical.inode(), marker.inode());
    }

    #[test]
    fn transaction_orchestrator_publishes_index_before_resources_and_completes_last() {
        let tmp = checkout_temp_dir("transaction-orchestrator");
        let store = MemoryStore::new();
        let reference = reference(&tmp.join("AGENTS.md"));
        admit(&store, &reference).expect("admit");
        let before = observe(reference.destination()).expect("before");
        let plan = plan(
            &store,
            &reference,
            before,
            revision(b"generated\n"),
            None,
            receipt(&reference, '4'),
        );
        let locks = TransactionLocks::acquire(
            &store,
            &index_key(),
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let pending = crate::resource_plan::pending_index(
            "team",
            &"a".repeat(64),
            None,
            None,
            vec![reference.clone()],
        )
        .expect("pending");
        let completed = crate::resource_plan::completed_index("team", &"a".repeat(64), None)
            .expect("completed");
        let outcome =
            execute_transaction(&locks, &[plan], None, &pending, &completed).expect("transaction");
        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert_eq!(outcome.unchanged(), [] as [ResourceRef; 0]);
        assert!(outcome.error().is_none());
        assert_eq!(
            crate::state::read_record::<Index>(&store, &index_key(), "Index").expect("index"),
            Some(completed)
        );
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
    }

    #[test]
    fn index_completion_failure_retains_applied_resource_outcomes() {
        let tmp = checkout_temp_dir("transaction-index-completion-failure");
        let store = MemoryStore::new();
        let reference = reference(&tmp.join("AGENTS.md"));
        admit(&store, &reference).expect("admit");
        let plan = plan(
            &store,
            &reference,
            observe(reference.destination()).expect("before"),
            revision(b"generated\n"),
            None,
            receipt(&reference, '5'),
        );
        let locks = TransactionLocks::acquire(
            &store,
            &index_key(),
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let pending = crate::resource_plan::pending_index(
            "team",
            &"a".repeat(64),
            None,
            None,
            vec![reference.clone()],
        )
        .expect("pending");
        let completed = crate::resource_plan::completed_index("team", &"a".repeat(64), None)
            .expect("completed");
        set_fault(Some("index-completion"));
        let outcome = execute_transaction(&locks, &[plan], None, &pending, &completed)
            .expect("partial transaction outcome");
        set_fault(None);
        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert!(outcome.error().is_some());
        assert_eq!(
            crate::state::read_record::<Index>(&store, &index_key(), "Index").expect("index"),
            Some(pending)
        );
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
    }

    #[test]
    fn cleanup_failure_preserves_the_permanently_committed_resource() {
        let tmp = checkout_temp_dir("transaction-cleanup-commit");
        let store = MemoryStore::new();
        let reference = reference(&tmp.join("AGENTS.md"));
        admit(&store, &reference).expect("admit");
        let next = receipt(&reference, '6');
        let plan = plan(
            &store,
            &reference,
            observe(reference.destination()).expect("before"),
            revision(b"generated\n"),
            None,
            next.clone(),
        );
        let locks = TransactionLocks::acquire(
            &store,
            &index_key(),
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let pending = crate::resource_plan::pending_index(
            "team",
            &"a".repeat(64),
            None,
            None,
            vec![reference.clone()],
        )
        .expect("pending");
        let completed = crate::resource_plan::completed_index("team", &"a".repeat(64), None)
            .expect("completed");

        set_fault(Some("cleanup"));
        let outcome = execute_transaction(&locks, &[plan], None, &pending, &completed)
            .expect("transaction outcome");
        set_fault(None);

        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert_eq!(outcome.failed(), Some(&reference));
        assert!(outcome.error().is_some());
        assert_eq!(stored_receipt(&store, &reference), Some(next));
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
        recover(&store, &locks.resources()[0]).expect("recover cleanup");
    }

    #[test]
    fn receipt_reread_failure_preserves_the_permanently_committed_resource() {
        let tmp = checkout_temp_dir("transaction-receipt-reread-commit");
        let store = MemoryStore::new();
        let reference = reference(&tmp.join("AGENTS.md"));
        admit(&store, &reference).expect("admit");
        let next = receipt(&reference, '9');
        let plan = plan(
            &store,
            &reference,
            observe(reference.destination()).expect("before"),
            revision(b"generated\n"),
            None,
            next.clone(),
        );
        let locks = TransactionLocks::acquire(
            &store,
            &index_key(),
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let pending = crate::resource_plan::pending_index(
            "team",
            &"a".repeat(64),
            None,
            None,
            vec![reference.clone()],
        )
        .expect("pending");
        let completed = crate::resource_plan::completed_index("team", &"a".repeat(64), None)
            .expect("completed");

        set_fault(Some("receipt-reread"));
        let outcome = execute_transaction(&locks, &[plan], None, &pending, &completed)
            .expect("transaction outcome");
        set_fault(None);

        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert_eq!(outcome.failed(), Some(&reference));
        assert!(outcome.error().is_some());
        assert_eq!(stored_receipt(&store, &reference), Some(next));
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
        recover(&store, &locks.resources()[0]).expect("recover cleanup");
    }

    #[test]
    fn later_precondition_failure_retains_earlier_commit() {
        for seam in ["resource-precondition", "index-require", "guard-lookup"] {
            let tmp = checkout_temp_dir(&format!("transaction-later-{seam}"));
            let store = MemoryStore::new();
            let first = reference(&tmp.join("first"));
            let second = reference(&tmp.join("second"));
            for reference in [&first, &second] {
                admit(&store, reference).expect("admit");
            }
            let plans = vec![
                plan(
                    &store,
                    &first,
                    observe(first.destination()).expect("first before"),
                    revision(b"one\n"),
                    None,
                    receipt(&first, '7'),
                ),
                plan(
                    &store,
                    &second,
                    observe(second.destination()).expect("second before"),
                    revision(b"two\n"),
                    None,
                    receipt(&second, '8'),
                ),
            ];
            let locks = TransactionLocks::acquire(
                &store,
                &index_key(),
                &[first.clone(), second.clone()],
                Duration::ZERO,
            )
            .expect("locks");
            let pending = crate::resource_plan::pending_index(
                "team",
                &"a".repeat(64),
                None,
                None,
                vec![first.clone(), second.clone()],
            )
            .expect("pending");
            let completed = crate::resource_plan::completed_index("team", &"a".repeat(64), None)
                .expect("completed");

            set_fault_after(seam, 1);
            let outcome = execute_transaction(&locks, &plans, None, &pending, &completed)
                .expect("transaction outcome");
            set_fault(None);

            assert_eq!(outcome.completed(), std::slice::from_ref(&first), "{seam}");
            assert_eq!(outcome.failed(), Some(&second), "{seam}");
            assert!(outcome.error().is_some(), "{seam}");
            assert_eq!(fs::read(first.destination()).expect("first"), b"one\n");
            assert!(!second.destination().exists(), "{seam}");
        }
    }
}
