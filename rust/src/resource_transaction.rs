use crate::authority::{AuthorityError, hierarchy_conflicts};
use crate::filesystem::{TargetLockGuard, target_lock};
use crate::observation::{failure, io_failure};
use crate::observations::{ErrorCode, TargetError};
use crate::resource_io::{
    ancestors, atomic, atomic_with_publication, cleanup_revision, destroy, move_revision, observe,
    read, require, require_unchanged_since, validate_kind, validate_state, write_revision,
};
use crate::resource_models::{Ancestor, Index, Receipt, ResourceRef, Revision};
use crate::resource_plan::{ResourcePlan, prepared_index_promotes};
use crate::security::ensure_private;
use crate::values::ValueError;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::PathBuf;
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

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) struct ResourceLockGuard {
    reference: ResourceRef,
    _guard: TargetLockGuard,
}

#[allow(dead_code)]
impl ResourceLockGuard {
    pub(crate) fn acquire(reference: &ResourceRef, timeout: Duration) -> Result<Self, TargetError> {
        Self::acquire_recording(reference, timeout, None)
    }

    fn acquire_recording(
        reference: &ResourceRef,
        timeout: Duration,
        on_created: Option<&mut dyn FnMut(&std::path::Path)>,
    ) -> Result<Self, TargetError> {
        let guard = target_lock(reference.state_root(), timeout, on_created)?;
        Ok(Self {
            reference: reference.clone(),
            _guard: guard,
        })
    }

    pub(crate) const fn reference(&self) -> &ResourceRef {
        &self.reference
    }
}

#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub struct InteropResourceLockGuard {
    _guard: ResourceLockGuard,
}

#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub fn interop_resource_lock(
    reference: &ResourceRef,
    timeout: Duration,
) -> Result<InteropResourceLockGuard, TargetError> {
    ResourceLockGuard::acquire(reference, timeout)
        .map(|guard| InteropResourceLockGuard { _guard: guard })
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn lock_resources(
    references: &[ResourceRef],
    timeout: Duration,
) -> Result<Vec<ResourceLockGuard>, TargetError> {
    let mut ordered = references.to_vec();
    crate::resource_plan::state_root_order(&mut ordered);
    ordered
        .iter()
        .map(|reference| ResourceLockGuard::acquire(reference, timeout))
        .collect()
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) struct TransactionLocks {
    index_root: PathBuf,
    _index: TargetLockGuard,
    resources: Vec<ResourceLockGuard>,
    created: Vec<Ancestor>,
}

#[allow(dead_code)]
impl TransactionLocks {
    pub(crate) fn acquire(
        index_root: &std::path::Path,
        references: &[ResourceRef],
        timeout: Duration,
    ) -> Result<Self, TargetError> {
        Self::acquire_after_index_validation(index_root, references, timeout, || Ok(()))
    }

    pub(crate) fn acquire_after_index_validation(
        index_root: &std::path::Path,
        references: &[ResourceRef],
        timeout: Duration,
        validate: impl FnOnce() -> Result<(), TargetError>,
    ) -> Result<Self, TargetError> {
        let mut created_paths = Vec::new();
        let mut record = |path: &std::path::Path| created_paths.push(path.to_path_buf());
        let index = target_lock(index_root, timeout, Some(&mut record))?;
        validate()?;
        let mut ordered = references.to_vec();
        crate::resource_plan::state_root_order(&mut ordered);
        let mut resources = Vec::with_capacity(ordered.len());
        for reference in &ordered {
            validate_kind(reference)?;
            resources.push(ResourceLockGuard::acquire_recording(
                reference,
                timeout,
                Some(&mut record),
            )?);
            crate::resource_io::admit(reference)?;
        }
        let mut created = Vec::new();
        for path in created_paths {
            if let Some(item) = ancestors(&path.join("lock"))?
                .into_iter()
                .find(|item| item.path() == path)
            {
                created.push(item);
            }
        }
        Ok(Self {
            index_root: index_root.to_path_buf(),
            _index: index,
            resources,
            created,
        })
    }

    pub(crate) fn resources(&self) -> &[ResourceLockGuard] {
        &self.resources
    }

    pub(crate) fn created(&self) -> &[Ancestor] {
        &self.created
    }
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn publish_index_transition(
    locks: &TransactionLocks,
    index: &Index,
    expected_revision: &Revision,
    expected_ancestors: &[Ancestor],
) -> Result<Revision, TargetError> {
    let index_root = &locks.index_root;
    require(index_root, expected_revision)?;
    let path = index_root.join("index.json");
    if ancestors(&path)? != expected_ancestors {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "installation index ancestor changed",
            index_root,
        ));
    }
    atomic(&path, index, Some(expected_ancestors))?;
    observe(index_root)
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn recover_locked_resources(
    references: &[ResourceRef],
    guards: &[ResourceLockGuard],
) -> Result<(), TargetError> {
    if references.len() != guards.len()
        || references
            .iter()
            .any(|reference| !guards.iter().any(|guard| guard.reference() == reference))
    {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource locks disagree",
            references
                .first()
                .map_or_else(|| std::path::Path::new("/"), ResourceRef::state_root),
        ));
    }
    for guard in guards {
        recover(guard)?;
    }
    Ok(())
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn reconcile_prepared_index(
    locks: &TransactionLocks,
    current: Option<&Index>,
    prepared: &Index,
    expected_revision: &Revision,
    expected_ancestors: &[Ancestor],
) -> Result<(), TargetError> {
    let index_root = &locks.index_root;
    let promote = prepared_index_promotes(current, prepared).map_err(|error| {
        failure(
            ErrorCode::RecoveryNeeded,
            error.message(),
            index_root.join("index.json.next"),
        )
    })?;
    require(index_root, expected_revision)?;
    if ancestors(&index_root.join("index.json"))? != expected_ancestors {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "index ancestor changed",
            index_root,
        ));
    }
    let next = index_root.join("index.json.next");
    if promote {
        fs::rename(&next, index_root.join("index.json")).map_err(|error| io_failure(&error, &next))
    } else {
        fs::remove_file(&next).map_err(|error| io_failure(&error, &next))
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
    dead_code,
    clippy::redundant_pub_crate,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub(crate) fn execute_transaction(
    locks: &TransactionLocks,
    plans: &[ResourcePlan],
    pending: &Index,
    completed: &Index,
    index_revision: &Revision,
    index_ancestors: &[Ancestor],
) -> Result<TransactionOutcome, TargetError> {
    let expected_completion = Index::new(
        pending.bundle_id().to_owned(),
        pending.context_digest().to_owned(),
        pending.pending().cloned(),
        None,
        None,
        Vec::new(),
        2,
    )
    .map_err(|error| failure(ErrorCode::InvalidState, error.message(), &locks.index_root))?;
    if completed != &expected_completion {
        return Err(failure(
            ErrorCode::InvalidState,
            "installation index completion disagrees with preparation",
            &locks.index_root,
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
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource locks disagree",
            &locks.index_root,
        ));
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
    let pending_revision =
        publish_index_transition(locks, pending, index_revision, index_ancestors)?;
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
            require(&locks.index_root, &pending_revision)?;
            inject("index-ancestor")?;
            if ancestors(&locks.index_root.join("index.json"))? != index_ancestors {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "installation index ancestor changed",
                    &locks.index_root,
                ));
            }
            inject("guard-lookup")?;
            let acquired = locks
                .resources()
                .iter()
                .find(|guard| guard.reference() == plan.resource())
                .ok_or_else(|| {
                    failure(
                        ErrorCode::ConcurrentChange,
                        "resource lock disagrees",
                        plan.resource().state_root(),
                    )
                })?;
            guard = Some(acquired);
            let prepared = prepare(plan, acquired)?;
            journal = Some(prepared.clone());
            require(&locks.index_root, &pending_revision)?;
            publish_with(&prepared, || committed.set(true))?;
            cleanup(&prepared)
        })();
        if let Err(error) = result {
            if !committed.get() {
                committed.set(journal.as_ref().is_some_and(|journal| {
                    current_receipt(journal)
                        .is_ok_and(|current| current.as_ref() == Some(journal.receipt()))
                }));
            }
            let recovery_error = guard.and_then(|guard| recover(guard).err());
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
    if let Err(error) = inject("index-completion").and_then(|()| {
        publish_index_transition(locks, completed, &pending_revision, index_ancestors).map(drop)
    }) {
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

fn paths(reference: &ResourceRef) -> (PathBuf, PathBuf, PathBuf) {
    (
        reference.state_root().join("transaction.json"),
        reference.state_root().join("staging/resource"),
        reference.state_root().join("backup/resource"),
    )
}

fn layout(reference: &ResourceRef) -> Result<(), TargetError> {
    validate_state(reference)?;
    for name in ["staging", "backup"] {
        let root = reference.state_root().join(name);
        let revision = observe(&root)?;
        if revision
            .nodes()
            .iter()
            .skip(1)
            .any(|node| node.path() != "resource" && !node.path().starts_with("resource/"))
        {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "unknown transaction data",
                &root,
            ));
        }
    }
    Ok(())
}

fn path_exists(path: &std::path::Path) -> Result<bool, TargetError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_failure(&error, path)),
    }
}

fn transaction_ancestors(reference: &ResourceRef) -> Result<Vec<Ancestor>, TargetError> {
    let (_, staged, backup) = paths(reference);
    let mut result = ancestors(&staged)?;
    for ancestor in ancestors(&backup)? {
        if !result.contains(&ancestor) {
            result.push(ancestor);
        }
    }
    Ok(result)
}

fn authority_error(error: AuthorityError) -> TargetError {
    match error {
        AuthorityError::Invalid(error) => {
            TargetError::new(ErrorCode::InvalidState, error.message(), None, None)
        }
        AuthorityError::Unsafe { message, path } => {
            TargetError::new(ErrorCode::UnsafePath, message, Some(path), None)
        }
        AuthorityError::Io {
            message,
            path,
            errno,
        } => TargetError::new(ErrorCode::IoError, message, path, errno),
    }
}

fn validate_ancestors(journal: &Journal) -> Result<(), TargetError> {
    if transaction_ancestors(journal.resource())? != journal.ancestors() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            journal.resource().destination(),
        ));
    }
    let (_, staged, backup) = paths(journal.resource());
    for path in [
        journal.resource().state_root(),
        staged
            .parent()
            .unwrap_or_else(|| journal.resource().state_root()),
        backup
            .parent()
            .unwrap_or_else(|| journal.resource().state_root()),
    ] {
        ensure_private(path)?;
    }
    let authority = journal
        .resource()
        .authority_checked()
        .map_err(authority_error)?;
    let conflicts = hierarchy_conflicts(
        &authority,
        journal.resource().kind() != crate::resource_models::ResourceKind::Document,
    )
    .map_err(authority_error)?;
    if let Some(conflict) = conflicts.first() {
        return Err(failure(
            ErrorCode::Conflict,
            "resource hierarchy reservation changed",
            conflict,
        ));
    }
    Ok(())
}

fn current_receipt(journal: &Journal) -> Result<Option<Receipt>, TargetError> {
    read(
        &journal.resource().state_root().join("receipt.json"),
        "Receipt",
    )
}

fn require_journal(journal: &Journal) -> Result<(), TargetError> {
    if read::<Journal>(&paths(journal.resource()).0, "Journal")?.as_ref() != Some(journal) {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "transaction record changed",
            paths(journal.resource()).0,
        ));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Option<(&'static str, usize)>> =
        const { std::cell::RefCell::new(None) };
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

#[cfg(any(test, feature = "interop-test-seams"))]
fn inject(boundary: &'static str) -> Result<(), TargetError> {
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

#[allow(dead_code, clippy::redundant_pub_crate, clippy::too_many_lines)]
pub(crate) fn prepare(
    plan: &ResourcePlan,
    guard: &ResourceLockGuard,
) -> Result<Journal, TargetError> {
    if guard.reference() != plan.resource() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource lock disagrees",
            plan.resource().state_root(),
        ));
    }
    let reference = plan.resource();
    if ancestors(&paths(reference).0)? != plan.ancestors() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            reference.destination(),
        ));
    }
    require(reference.state_root(), plan.state_revision())?;
    require(reference.destination(), plan.before())?;
    if plan.require_existing() && plan.before().nodes().is_empty() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "required existing resource disappeared",
            reference.destination(),
        ));
    }
    let receipt: Option<Receipt> = read(&reference.state_root().join("receipt.json"), "Receipt")?;
    if receipt.as_ref() != plan.previous() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "receipt changed during preparation",
            reference.state_root(),
        ));
    }
    ensure_private(reference.state_root())?;
    let (_, staged_path, backup_path) = paths(reference);
    let mut storage = Vec::new();
    for (name, root) in [
        ("staging", staged_path.parent()),
        ("backup", backup_path.parent()),
    ]
    .into_iter()
    .filter_map(|(name, root)| root.map(|root| (name, root)))
    {
        let expected = if let Some(node) = plan
            .state_revision()
            .nodes()
            .iter()
            .find(|node| node.path() == name)
        {
            Revision::new(vec![
                crate::resource_models::Node::new(
                    String::new(),
                    node.data().map(<[u8]>::to_vec),
                    node.mode().clone(),
                    node.uid().clone(),
                    node.gid().clone(),
                    node.device().clone(),
                    node.inode().clone(),
                    node.security().to_vec(),
                )
                .map_err(|error| {
                    failure(
                        ErrorCode::InvalidState,
                        error.message(),
                        reference.state_root(),
                    )
                })?,
            ])
            .map_err(|error| {
                failure(
                    ErrorCode::InvalidState,
                    error.message(),
                    reference.state_root(),
                )
            })?
        } else {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(root)
                .map_err(|error| io_failure(&error, root))?;
            observe(root)?
        };
        require(root, &expected)?;
        ensure_private(root)?;
        storage.push((root.to_path_buf(), expected));
    }
    let storage_ancestors = transaction_ancestors(reference)?;
    for (path, expected) in &storage {
        require(path, expected)?;
    }
    let filtered = storage_ancestors
        .iter()
        .filter(|item| {
            item.path()
                != staged_path
                    .parent()
                    .unwrap_or_else(|| reference.state_root())
                && item.path()
                    != backup_path
                        .parent()
                        .unwrap_or_else(|| reference.state_root())
        })
        .cloned()
        .collect::<Vec<_>>();
    if filtered != plan.ancestors() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            reference.destination(),
        ));
    }
    let staged = if plan.after() == plan.before() {
        plan.before().clone()
    } else {
        write_revision(&staged_path, plan.after())?
    };
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
        .map_err(|error| {
            failure(
                ErrorCode::InvalidState,
                error.message(),
                reference.state_root(),
            )
        })?;
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
    .map_err(|error| {
        failure(
            ErrorCode::InvalidState,
            error.message(),
            reference.state_root(),
        )
    })?;
    validate_ancestors(&journal)?;
    require(reference.destination(), plan.before())?;
    atomic(&paths(reference).0, &journal, None)?;
    inject("prepare")?;
    Ok(journal)
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn publish_receipt(journal: &Journal) -> Result<(), TargetError> {
    publish_receipt_with(journal, || {})
}

fn publish_receipt_with(journal: &Journal, committed: impl FnOnce()) -> Result<(), TargetError> {
    require_journal(journal)?;
    layout(journal.resource())?;
    validate_ancestors(journal)?;
    let (_, staged, backup) = paths(journal.resource());
    let empty = Revision::new(Vec::new())
        .map_err(|error| failure(ErrorCode::InvalidState, error.message(), &staged))?;
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
    for name in [
        "authority.json.next",
        "transaction.json.next",
        "receipt.json.next",
    ] {
        let path = journal.resource().state_root().join(name);
        if path_exists(&path)? {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "unexpected metadata preparation before commit",
                path,
            ));
        }
    }
    if current_receipt(journal)?.as_ref() != journal.previous() {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "receipt changed before commit",
            journal.resource().state_root(),
        ));
    }
    inject("receipt")?;
    atomic_with_publication(
        &journal.resource().state_root().join("receipt.json"),
        journal.receipt(),
        None,
        || {
            committed();
            inject("receipt-reread")
        },
    )?;
    inject("receipt-committed")
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn publish(journal: &Journal) -> Result<(), TargetError> {
    publish_with(journal, || {})
}

fn publish_with(journal: &Journal, committed: impl FnOnce()) -> Result<(), TargetError> {
    let reference = journal.resource();
    let (_, staged, backup) = paths(reference);
    require_journal(journal)?;
    validate_ancestors(journal)?;
    require(reference.destination(), journal.before())?;
    if journal.before() != journal.staged() {
        require(&staged, journal.staged())?;
        if !journal.before().nodes().is_empty() {
            move_revision(reference.destination(), &backup, journal.backup())?;
            inject("backup")?;
            inject("rename")?;
            require(&backup, journal.backup())?;
        }
        if !journal.staged().nodes().is_empty() {
            inject("staging")?;
            inject("rename")?;
            move_revision(&staged, reference.destination(), journal.staged())?;
        }
        require(reference.destination(), journal.published())?;
    }
    inject("publish")?;
    publish_receipt_with(journal, committed)
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn cleanup(journal: &Journal) -> Result<(), TargetError> {
    require_journal(journal)?;
    layout(journal.resource())?;
    validate_ancestors(journal)?;
    let (_, staged, backup) = paths(journal.resource());
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
    let next = journal.resource().state_root().join("receipt.json.next");
    if path_exists(&next)? {
        if read::<Receipt>(&next, "Receipt")?.as_ref() != Some(journal.receipt()) {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "staged receipt changed",
                &next,
            ));
        }
        fs::remove_file(&next).map_err(|error| io_failure(&error, &next))?;
    }
    inject("cleanup")?;
    require_journal(journal)?;
    let path = paths(journal.resource()).0;
    fs::remove_file(&path).map_err(|error| io_failure(&error, &path))
}

fn preparation_time(reference: &ResourceRef) -> Result<SystemTime, TargetError> {
    let path = paths(reference).0;
    let metadata = fs::symlink_metadata(&path).map_err(|error| io_failure(&error, &path))?;
    metadata
        .modified()
        .map_err(|error| io_failure(&error, &path))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn rollback(journal: &Journal) -> Result<(), TargetError> {
    let reference = journal.resource();
    let (_, staged_path, backup_path) = paths(reference);
    layout(reference)?;
    validate_ancestors(journal)?;
    let actual = observe(reference.destination())?;
    let staged = observe(&staged_path)?;
    let backup = observe(&backup_path)?;
    if !actual.nodes().is_empty()
        && (actual == *journal.before()
            || actual == *journal.staged()
            || actual == *journal.published())
    {
        require_unchanged_since(
            reference.destination(),
            &actual,
            preparation_time(reference)?,
        )?;
    }
    if actual == *journal.before() && backup.nodes().is_empty() {
        inject("rollback")?;
        return cleanup(journal);
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
        require(reference.destination(), journal.before())?;
    } else if !backup.nodes().is_empty() {
        if !actual.nodes().is_empty() {
            if (actual != *journal.published() && actual != *journal.staged())
                || !staged.nodes().is_empty()
            {
                return Err(failure(
                    ErrorCode::RecoveryNeeded,
                    "unrecognized published revision",
                    reference.destination(),
                ));
            }
            move_revision(reference.destination(), &staged_path, &actual)?;
        }
        move_revision(&backup_path, reference.destination(), &backup)?;
        require(reference.destination(), journal.before())?;
    } else if actual == *journal.before() {
    } else if journal.before().nodes().is_empty()
        && actual == *journal.published()
        && staged.nodes().is_empty()
    {
        move_revision(reference.destination(), &staged_path, &actual)?;
    } else {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "cannot restore complete previous revision",
            reference.destination(),
        ));
    }
    inject("rollback")?;
    cleanup(journal)
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn recover(guard: &ResourceLockGuard) -> Result<(), TargetError> {
    let reference = guard.reference();
    let pending = validate_state(reference)?;
    if pending.is_empty() {
        return Ok(());
    }
    let journal_path = paths(reference).0;
    let Some(first_pending) = pending.first() else {
        return Ok(());
    };
    let mut journal: Option<Journal> = read(&journal_path, "Journal")?;
    if journal.is_none() {
        let next = reference.state_root().join("transaction.json.next");
        let candidate: Option<Journal> = read(&next, "Journal")?;
        if candidate
            .as_ref()
            .is_some_and(|value| value.resource() == reference)
        {
            fs::rename(&next, &journal_path).map_err(|error| io_failure(&error, &next))?;
            journal = candidate;
        }
    }
    let Some(journal) = journal else {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "unrecognized preparation data retained",
            first_pending,
        ));
    };
    if journal.resource() != reference {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "unrecognized preparation data retained",
            first_pending,
        ));
    }
    let current = current_receipt(&journal)?;
    if current.as_ref() == Some(journal.receipt()) {
        cleanup(&journal)
    } else if current.as_ref() == journal.previous() {
        rollback(&journal)
    } else {
        Err(failure(
            ErrorCode::RecoveryNeeded,
            "receipt disagrees with transaction",
            reference.state_root(),
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
    use crate::resource_io::{admit, reserve};
    use crate::resource_models::{Count, Generation, Node, ResourceKind};
    use crate::test_support::checkout_temp_dir;
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
    use std::path::Path;

    fn reference(destination: &Path) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::Document,
            authority.state_root().to_path_buf(),
        )
        .expect("reference")
    }

    fn container_reference(destination: &Path) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::SkillContainer,
            authority.state_root().to_path_buf(),
        )
        .expect("reference")
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
        reference: &ResourceRef,
        before: Revision,
        after: Revision,
        previous: Option<Receipt>,
        next: Receipt,
    ) -> ResourcePlan {
        ResourcePlan::new(
            reference.clone(),
            before,
            after,
            previous,
            next,
            ancestors(&paths(reference).0).expect("ancestors"),
            observe(reference.state_root()).expect("state revision"),
            validate_state(reference).expect("recovery"),
            false,
            None,
        )
        .expect("plan")
    }

    #[test]
    fn prepare_publish_and_cleanup_commit_a_resource_and_receipt() {
        let tmp = checkout_temp_dir("transaction-commit");
        let reference = reference(&tmp.join("AGENTS.md"));
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let before = observe(reference.destination()).expect("before");
        let next = receipt(&reference, '1');
        let plan = plan(
            &reference,
            before,
            revision(b"generated\n"),
            None,
            next.clone(),
        );
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&plan, &guard).expect("prepare");
        assert!(paths(&reference).0.is_file());
        publish(&journal).expect("publish");
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
        assert_eq!(
            read::<Receipt>(&reference.state_root().join("receipt.json"), "Receipt")
                .expect("read receipt"),
            Some(next)
        );
        cleanup(&journal).expect("cleanup");
        assert_eq!(
            validate_state(&reference).expect("state"),
            [] as [PathBuf; 0]
        );
    }

    #[test]
    fn recover_rolls_back_prepared_changes_and_exact_prefix_cleanup() {
        let tmp = checkout_temp_dir("transaction-rollback");
        let destination = tmp.join("AGENTS.md");
        fs::write(&destination, b"before\n").expect("foreign");
        let reference = reference(&destination);
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let before = observe(&destination).expect("before");
        let next = receipt(&reference, '2');
        let plan = plan(&reference, before.clone(), revision(b"after\n"), None, next);
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&plan, &guard).expect("prepare");
        let staged_path = paths(&reference).1;
        let staged = observe(&staged_path).expect("staged");
        assert!(cleanup_revision(&staged, journal.staged()));
        recover(&guard).expect("recover");
        assert_eq!(observe(&destination).expect("restored"), before);
        assert_eq!(
            validate_state(&reference).expect("state"),
            [] as [PathBuf; 0]
        );
    }

    fn journal_modification_time(reference: &ResourceRef) -> SystemTime {
        fs::symlink_metadata(paths(reference).0)
            .and_then(|metadata| metadata.modified())
            .expect("journal modification time")
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
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let old = receipt(&reference, '0');
        atomic(&reference.state_root().join("receipt.json"), &old, None).expect("old receipt");
        let before = observe(&destination).expect("before");
        let plan = plan(
            &reference,
            before,
            revision(b"after\n"),
            Some(old.clone()),
            receipt(&reference, '1'),
        );
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&plan, &guard).expect("prepare");
        move_revision(&destination, &paths(&reference).2, journal.backup()).expect("backup move");
        move_revision(&paths(&reference).1, &destination, journal.staged()).expect("publish move");
        // Recycled inodes need the post-preparation identity check.
        revise_modification_time(
            &destination,
            journal_modification_time(&reference)
                .checked_add(Duration::from_secs(60))
                .expect("replacement time"),
        );
        let error = recover(&guard).expect_err("recovery refuses the replacement");
        assert_eq!(error.code(), ErrorCode::RecoveryNeeded);
        assert_eq!(
            error.message(),
            "destination revision changed after preparation"
        );
        assert_eq!(error.path(), Some(destination.as_path()));
        assert_eq!(fs::read(&destination).expect("destination"), b"after\n");
        assert_eq!(
            read::<Receipt>(&reference.state_root().join("receipt.json"), "Receipt")
                .expect("receipt read")
                .expect("receipt present"),
            old,
            "the previous receipt is retained"
        );
        assert!(paths(&reference).0.exists(), "journal is retained");
        assert!(paths(&reference).2.exists(), "backup is retained");
    }

    #[test]
    fn recover_refuses_a_replacement_of_the_previous_revision() {
        let tmp = checkout_temp_dir("transaction-previous-replacement");
        let destination = tmp.join("AGENTS.md");
        exclusive_write(&destination, b"before\n", 0o600);
        let reference = reference(&destination);
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let old = receipt(&reference, '0');
        atomic(&reference.state_root().join("receipt.json"), &old, None).expect("old receipt");
        let before = observe(&destination).expect("before");
        let plan = plan(
            &reference,
            before,
            revision(b"after\n"),
            Some(old),
            receipt(&reference, '1'),
        );
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        prepare(&plan, &guard).expect("prepare");
        revise_modification_time(
            &destination,
            journal_modification_time(&reference)
                .checked_add(Duration::from_secs(60))
                .expect("replacement time"),
        );
        let error = recover(&guard).expect_err("recovery refuses the replacement");
        assert_eq!(error.code(), ErrorCode::RecoveryNeeded);
        assert_eq!(
            error.message(),
            "destination revision changed after preparation"
        );
        assert_eq!(fs::read(&destination).expect("destination"), b"before\n");
        assert!(paths(&reference).0.exists(), "journal is retained");
        assert!(paths(&reference).1.exists(), "staging is retained");
    }

    #[test]
    fn resource_locks_are_acquired_in_state_root_order() {
        let tmp = checkout_temp_dir("transaction-lock-order");
        let first = reference(&tmp.join("z"));
        let second = reference(&tmp.join("a"));
        let guards = lock_resources(&[first, second], Duration::ZERO).expect("locks");
        let roots: Vec<_> = guards
            .iter()
            .map(|guard| guard.reference().state_root())
            .collect();
        let mut sorted = roots.clone();
        sorted.sort();
        assert_eq!(roots, sorted);
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
            reserve(&reference).expect("reserve");
            admit(&reference).expect("admit");
            let old = receipt(&reference, '0');
            atomic(&reference.state_root().join("receipt.json"), &old, None).expect("old receipt");
            let before = observe(&destination).expect("before");
            let next = receipt(&reference, '1');
            let after = if name == "ownership-only-adoption" {
                before.clone()
            } else {
                revision(&generated)
            };
            let plan = plan(
                &reference,
                before.clone(),
                after,
                Some(old.clone()),
                next.clone(),
            );
            let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
            let journal = prepare(&plan, &guard).expect("prepare");

            match name {
                "prepared" | "ownership-only-adoption" => {}
                "backup-moved" => {
                    move_revision(&destination, &paths(&reference).2, journal.backup())
                        .expect("backup move");
                }
                "published-before-receipt"
                | "foreign-edit-before-commit"
                | "permissions-changed-before-commit"
                | "identity-changed-before-commit" => {
                    move_revision(&destination, &paths(&reference).2, journal.backup())
                        .expect("backup move");
                    move_revision(&paths(&reference).1, &destination, journal.staged())
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
                            fs::remove_file(&destination).expect("replace destination");
                            exclusive_write(
                                &destination,
                                &fixture_transaction_revision(case, "observed", "resource")
                                    .expect("replacement fixture bytes"),
                                0o600,
                            );
                        }
                        _ => {}
                    }
                }
                "receipt-committed" | "committed-foreign-edit" => {
                    publish(&journal).expect("publish and commit");
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
            let recovered = recover(&guard);
            assert_eq!(recovered.is_err(), expected_failure, "{name}");
            assert_eq!(
                paths(&reference).0.exists(),
                expected
                    .get("retain_journal")
                    .and_then(crate::json::Json::as_bool)
                    .expect("retain journal"),
                "{name} journal retention"
            );
            assert_eq!(
                paths(&reference).2.exists(),
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
            let retained: Receipt = read(&reference.state_root().join("receipt.json"), "Receipt")
                .expect("receipt read")
                .expect("receipt present");
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
            reserve(&reference).expect("reserve");
            admit(&reference).expect("admit");
            let old = receipt(&reference, '0');
            atomic(&reference.state_root().join("receipt.json"), &old, None).expect("old receipt");
            let before = observe(&destination).expect("before");
            let plan = plan(
                &reference,
                before,
                revision(b"after\n"),
                Some(old.clone()),
                receipt(&reference, '1'),
            );
            let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");

            let journal = if seam == "prepare" {
                set_fault(Some(seam));
                assert!(prepare(&plan, &guard).is_err());
                None
            } else {
                Some(prepare(&plan, &guard).expect("prepare"))
            };
            if let Some(journal) = &journal {
                set_fault(Some(seam));
                if seam == "rollback" {
                    assert!(recover(&guard).is_err());
                } else {
                    let result = publish(journal).and_then(|()| cleanup(journal));
                    assert!(result.is_err());
                }
            }
            set_fault(None);
            recover(&guard).expect("recover after seam");
            recover(&guard).expect("idempotent recovery");
            assert_eq!(
                validate_state(&reference).expect("state"),
                [] as [PathBuf; 0]
            );
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
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
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
        let plan = plan(&reference, before, after, None, next);
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&plan, &guard).expect("prepare");
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
    fn index_preparation_is_reconciled_only_while_index_lock_is_held() {
        let tmp = checkout_temp_dir("transaction-index");
        let index_root = tmp.join("index");
        let reference = reference(&tmp.join("AGENTS.md"));
        let locks = TransactionLocks::acquire(
            &index_root,
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let generation = Generation::new(
            "1".to_owned(),
            "b".repeat(64),
            "c".repeat(64),
            vec![(reference.clone(), Vec::new())],
        )
        .expect("generation");
        let prepared = crate::resource_plan::pending_index(
            "team",
            &"a".repeat(64),
            None,
            Some(generation),
            vec![reference],
        )
        .expect("prepared index");
        let next = index_root.join("index.json.next");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&next)
            .expect("prepared file");
        file.write_all(&crate::codec::encode(&prepared))
            .expect("prepared bytes");
        drop(file);
        let revision = observe(&index_root).expect("index revision");
        let parents = ancestors(&index_root.join("index.json")).expect("index ancestors");
        reconcile_prepared_index(&locks, None, &prepared, &revision, &parents)
            .expect("promote preparation");
        assert_eq!(
            read::<Index>(&index_root.join("index.json"), "Index").expect("read index"),
            Some(prepared)
        );
        assert!(!next.exists());
    }

    #[test]
    fn transaction_orchestrator_publishes_index_before_resources_and_completes_last() {
        let tmp = checkout_temp_dir("transaction-orchestrator");
        let index_root = tmp.join("index");
        let reference = reference(&tmp.join("AGENTS.md"));
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let before = observe(reference.destination()).expect("before");
        let plan = plan(
            &reference,
            before,
            revision(b"generated\n"),
            None,
            receipt(&reference, '4'),
        );
        let locks = TransactionLocks::acquire(
            &index_root,
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let initial = observe(&index_root).expect("index revision");
        let parents = ancestors(&index_root.join("index.json")).expect("index ancestors");
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
            execute_transaction(&locks, &[plan], &pending, &completed, &initial, &parents)
                .expect("transaction");
        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert_eq!(outcome.unchanged(), [] as [ResourceRef; 0]);
        assert!(outcome.error().is_none());
        assert_eq!(
            read::<Index>(&index_root.join("index.json"), "Index").expect("index"),
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
        let index_root = tmp.join("index");
        let reference = reference(&tmp.join("AGENTS.md"));
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let plan = plan(
            &reference,
            observe(reference.destination()).expect("before"),
            revision(b"generated\n"),
            None,
            receipt(&reference, '5'),
        );
        let locks = TransactionLocks::acquire(
            &index_root,
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let initial = observe(&index_root).expect("index revision");
        let parents = ancestors(&index_root.join("index.json")).expect("index ancestors");
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
        let outcome =
            execute_transaction(&locks, &[plan], &pending, &completed, &initial, &parents)
                .expect("partial transaction outcome");
        set_fault(None);
        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert!(outcome.error().is_some());
        assert_eq!(
            read::<Index>(&index_root.join("index.json"), "Index").expect("index"),
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
        let index_root = tmp.join("index");
        let reference = reference(&tmp.join("AGENTS.md"));
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let next = receipt(&reference, '6');
        let plan = plan(
            &reference,
            observe(reference.destination()).expect("before"),
            revision(b"generated\n"),
            None,
            next.clone(),
        );
        let locks = TransactionLocks::acquire(
            &index_root,
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let initial = observe(&index_root).expect("index revision");
        let parents = ancestors(&index_root.join("index.json")).expect("index ancestors");
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
        let outcome =
            execute_transaction(&locks, &[plan], &pending, &completed, &initial, &parents)
                .expect("transaction outcome");
        set_fault(None);

        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert_eq!(outcome.failed(), Some(&reference));
        assert!(outcome.error().is_some());
        assert_eq!(
            read::<Receipt>(&reference.state_root().join("receipt.json"), "Receipt")
                .expect("receipt"),
            Some(next)
        );
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
        recover(&locks.resources()[0]).expect("recover cleanup");
    }

    #[test]
    fn receipt_reread_failure_preserves_the_permanently_committed_resource() {
        let tmp = checkout_temp_dir("transaction-receipt-reread-commit");
        let index_root = tmp.join("index");
        let reference = reference(&tmp.join("AGENTS.md"));
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let next = receipt(&reference, '9');
        let plan = plan(
            &reference,
            observe(reference.destination()).expect("before"),
            revision(b"generated\n"),
            None,
            next.clone(),
        );
        let locks = TransactionLocks::acquire(
            &index_root,
            std::slice::from_ref(&reference),
            Duration::ZERO,
        )
        .expect("locks");
        let initial = observe(&index_root).expect("index revision");
        let parents = ancestors(&index_root.join("index.json")).expect("index ancestors");
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
        let outcome =
            execute_transaction(&locks, &[plan], &pending, &completed, &initial, &parents)
                .expect("transaction outcome");
        set_fault(None);

        assert_eq!(outcome.completed(), std::slice::from_ref(&reference));
        assert_eq!(outcome.failed(), Some(&reference));
        assert!(outcome.error().is_some());
        assert_eq!(
            read::<Receipt>(&reference.state_root().join("receipt.json"), "Receipt")
                .expect("receipt"),
            Some(next)
        );
        assert_eq!(
            fs::read(reference.destination()).expect("resource"),
            b"generated\n"
        );
        recover(&locks.resources()[0]).expect("recover cleanup");
    }

    #[test]
    fn later_precondition_failure_retains_earlier_commit() {
        for seam in [
            "resource-precondition",
            "index-require",
            "index-ancestor",
            "guard-lookup",
        ] {
            let tmp = checkout_temp_dir(&format!("transaction-later-{seam}"));
            let index_root = tmp.join("index");
            let first = reference(&tmp.join("first"));
            let second = reference(&tmp.join("second"));
            for reference in [&first, &second] {
                reserve(reference).expect("reserve");
                admit(reference).expect("admit");
            }
            let plans = vec![
                plan(
                    &first,
                    observe(first.destination()).expect("first before"),
                    revision(b"one\n"),
                    None,
                    receipt(&first, '7'),
                ),
                plan(
                    &second,
                    observe(second.destination()).expect("second before"),
                    revision(b"two\n"),
                    None,
                    receipt(&second, '8'),
                ),
            ];
            let locks = TransactionLocks::acquire(
                &index_root,
                &[first.clone(), second.clone()],
                Duration::ZERO,
            )
            .expect("locks");
            let initial = observe(&index_root).expect("index revision");
            let parents = ancestors(&index_root.join("index.json")).expect("index ancestors");
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
            let outcome =
                execute_transaction(&locks, &plans, &pending, &completed, &initial, &parents)
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
