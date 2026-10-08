use crate::authority::{AuthorityError, Claim, ClaimSelector, Ownership, ResourceAuthority};
use crate::bundle::Bundle;
use crate::content::{Content, DocumentSchema, FileMode};
use crate::editors::{
    Acquisition, Disposition, DocumentEdit, DocumentProvenance, DocumentRequest, EditStatus,
    OwnedSelection, content_claim, observe_selections,
};
use crate::observation::failure;
use crate::observations::{ErrorCode, TargetError};
use crate::rendered::{RenderedArtifact, RenderedBundle};
use crate::resource_io::{ancestors, observe, read, validate_state};
use crate::resource_models::{
    Ancestor, Count, Generation, Index, Node, OwnedClaim, Receipt, ResourceKind, ResourceRef,
    Revision,
};
use crate::values::ValueError;
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

type Requirements = Vec<(PathBuf, String)>;
type RequirementsByArtifact = HashMap<String, Requirements>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePlan {
    resource: ResourceRef,
    before: Revision,
    after: Revision,
    previous: Option<Receipt>,
    receipt: Receipt,
    ancestors: Vec<Ancestor>,
    state_revision: Revision,
    recovery_paths: Vec<PathBuf>,
    require_existing: bool,
    error: Option<TargetError>,
}

impl ResourcePlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        resource: ResourceRef,
        before: Revision,
        after: Revision,
        previous: Option<Receipt>,
        receipt: Receipt,
        ancestors: Vec<Ancestor>,
        state_revision: Revision,
        recovery_paths: Vec<PathBuf>,
        require_existing: bool,
        error: Option<TargetError>,
    ) -> Result<Self, ValueError> {
        if receipt.resource() != &resource
            || previous
                .as_ref()
                .is_some_and(|value| value.resource() != &resource)
        {
            return Err(ValueError::new("planned receipts disagree with resource"));
        }
        if recovery_paths.iter().any(|path| !path.is_absolute()) {
            return Err(ValueError::new("expected an absolute concrete path"));
        }
        Ok(Self {
            resource,
            before,
            after,
            previous,
            receipt,
            ancestors,
            state_revision,
            recovery_paths,
            require_existing,
            error,
        })
    }

    pub const fn resource(&self) -> &ResourceRef {
        &self.resource
    }
    pub const fn before(&self) -> &Revision {
        &self.before
    }
    pub const fn after(&self) -> &Revision {
        &self.after
    }
    pub const fn previous(&self) -> Option<&Receipt> {
        self.previous.as_ref()
    }
    pub const fn receipt(&self) -> &Receipt {
        &self.receipt
    }
    pub fn ancestors(&self) -> &[Ancestor] {
        &self.ancestors
    }
    pub const fn state_revision(&self) -> &Revision {
        &self.state_revision
    }
    pub fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }
    pub const fn require_existing(&self) -> bool {
        self.require_existing
    }
    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }

    pub(crate) fn with_observations(
        &self,
        state_revision: Revision,
        ancestors: Vec<Ancestor>,
    ) -> Self {
        let mut result = self.clone();
        result.state_revision = state_revision;
        result.ancestors = ancestors;
        result
    }

    pub(crate) fn with_error(&self, error: TargetError) -> Self {
        let mut result = self.clone();
        result.after = result.before.clone();
        result.error = Some(error);
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) struct ClaimObservation {
    claim_id: String,
    bundle_id: String,
    artifact_id: String,
    version: String,
    bundle_digest: String,
    render_digest: String,
    status: EditStatus,
}

#[allow(dead_code)]
impl ClaimObservation {
    pub(crate) fn claim_id(&self) -> &str {
        &self.claim_id
    }

    pub(crate) fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub(crate) fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn bundle_digest(&self) -> &str {
        &self.bundle_digest
    }

    pub(crate) fn render_digest(&self) -> &str {
        &self.render_digest
    }

    pub(crate) const fn status(&self) -> EditStatus {
        self.status
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) struct ResourceObservation {
    destination: PathBuf,
    state_root: PathBuf,
    claims: Vec<ClaimObservation>,
    content_digest: String,
    recovery_paths: Vec<PathBuf>,
}

#[allow(dead_code)]
impl ResourceObservation {
    pub(crate) fn destination(&self) -> &Path {
        &self.destination
    }

    pub(crate) fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub(crate) fn claims(&self) -> &[ClaimObservation] {
        &self.claims
    }

    pub(crate) fn content_digest(&self) -> &str {
        &self.content_digest
    }

    pub(crate) fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }
}

#[allow(clippy::needless_pass_by_value)]
fn invalid(error: ValueError, path: &Path) -> TargetError {
    failure(ErrorCode::InvalidState, error.message(), path)
}

#[allow(dead_code)]
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

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn artifact_claim(artifact: &RenderedArtifact) -> Result<Claim, ValueError> {
    if let Some(subtree) = artifact.subtree() {
        return Claim::new(
            Ownership::Subtree,
            ClaimSelector::Subtree(subtree.to_owned()),
        );
    }
    match artifact.content() {
        Content::File(_) => Claim::new(Ownership::File, ClaimSelector::Whole),
        Content::Tree(_) => Claim::new(Ownership::Tree, ClaimSelector::Whole),
        content => content_claim(content),
    }
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn resource(artifact: &RenderedArtifact) -> Result<ResourceRef, TargetError> {
    let authority = ResourceAuthority::new(artifact.destination()).map_err(authority_error)?;
    let kind = if artifact.subtree().is_some() {
        ResourceKind::SkillContainer
    } else {
        match artifact.content() {
            Content::Tree(_) => ResourceKind::Tree,
            _ => ResourceKind::Document,
        }
    };
    ResourceRef::new(
        authority.destination().to_path_buf(),
        kind,
        authority.state_root().to_path_buf(),
    )
    .map_err(|error| invalid(error, artifact.destination()))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn intact(actual: &Revision, expected: &Revision) -> bool {
    actual
        .nodes()
        .iter()
        .map(|node| (node.path(), node.data(), node.mode().as_u32()))
        .eq(expected
            .nodes()
            .iter()
            .map(|node| (node.path(), node.data(), node.mode().as_u32())))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn selected(
    revision: &Revision,
    selector: Option<&str>,
) -> Result<Revision, ValueError> {
    let Some(selector) = selector else {
        return Ok(revision.clone());
    };
    let prefix = format!("{selector}/");
    let nodes = revision
        .nodes()
        .iter()
        .filter(|node| node.path() == selector || node.path().starts_with(&prefix))
        .map(|node| {
            Node::new(
                if node.path() == selector {
                    String::new()
                } else {
                    node.path()
                        .strip_prefix(&prefix)
                        .unwrap_or_default()
                        .to_owned()
                },
                node.data().map(<[u8]>::to_vec),
                node.mode().clone(),
                node.uid().clone(),
                node.gid().clone(),
                node.device().clone(),
                node.inode().clone(),
                node.security().to_vec(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Revision::new(nodes)
}

fn rebase_node(node: &Node, path: String) -> Result<Node, ValueError> {
    Node::new(
        path,
        node.data().map(<[u8]>::to_vec),
        node.mode().clone(),
        node.uid().clone(),
        node.gid().clone(),
        node.device().clone(),
        node.inode().clone(),
        node.security().to_vec(),
    )
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn replace_selection(
    revision: &Revision,
    claim: &Claim,
    content: &Revision,
) -> Result<Revision, ValueError> {
    let ClaimSelector::Subtree(selector) = claim.selector() else {
        return Ok(content.clone());
    };
    let prefix = format!("{selector}/");
    let mut nodes = revision
        .nodes()
        .iter()
        .filter(|node| node.path() != selector && !node.path().starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    if !content.nodes().is_empty() && nodes.is_empty() {
        nodes.push(new_node(String::new(), None, 0o755)?);
    }
    nodes.extend(
        content
            .nodes()
            .iter()
            .map(|node| {
                rebase_node(
                    node,
                    if node.path().is_empty() {
                        selector.clone()
                    } else {
                        format!("{prefix}{}", node.path())
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    if !content.nodes().is_empty() {
        let mut existing: HashSet<String> =
            nodes.iter().map(|node| node.path().to_owned()).collect();
        let parts = selector.split('/').collect::<Vec<_>>();
        for index in 1..parts.len() {
            let parent = parts.get(..index).unwrap_or_default().join("/");
            if existing.insert(parent.clone()) {
                nodes.push(new_node(parent, None, 0o755)?);
            }
        }
    }
    nodes.sort_by(|left, right| left.path().cmp(right.path()));
    Revision::new(nodes)
}

fn new_node(path: String, data: Option<Vec<u8>>, mode: i64) -> Result<Node, ValueError> {
    Node::new(
        path,
        data,
        Count::Small(mode),
        Count::Small(i64::from(unsafe { libc::geteuid() })),
        Count::Small(i64::from(unsafe { libc::getegid() })),
        Count::Small(0),
        Count::Small(0),
        Vec::new(),
    )
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn desired_revision(content: &Content) -> Result<Revision, ValueError> {
    match content {
        Content::File(file) => {
            let mode = match file.mode() {
                FileMode::Private => 0o600,
                FileMode::Readable => 0o644,
                FileMode::Executable => 0o755,
            };
            Revision::new(vec![new_node(
                String::new(),
                Some(file.data().to_vec()),
                mode,
            )?])
        }
        Content::Tree(tree) => {
            let mut nodes = vec![new_node(String::new(), None, 0o755)?];
            nodes.extend(
                tree.entries()
                    .iter()
                    .map(|entry| {
                        new_node(
                            entry.path().to_owned(),
                            entry.data().map(<[u8]>::to_vec),
                            if entry.is_directory() || entry.executable() {
                                0o755
                            } else {
                                0o644
                            },
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
            Revision::new(nodes)
        }
        _ => Err(ValueError::new(
            "whole resource requires file or tree content",
        )),
    }
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn claim_observations(
    receipt: &Receipt,
    revision: &Revision,
) -> Result<Vec<ClaimObservation>, ValueError> {
    let selections = receipt
        .claims()
        .iter()
        .filter_map(|owned| owned.selection().cloned())
        .collect::<Vec<_>>();
    let mut statuses = observe_selections(revision.data(), &selections)?
        .into_iter()
        .map(|item| (item.claim().clone(), item.status()))
        .collect::<HashMap<_, _>>();
    for owned in receipt.claims() {
        if let Some(installed) = owned.installed() {
            statuses.insert(
                owned.claim().clone(),
                if intact(
                    &selected(
                        revision,
                        match owned.claim().selector() {
                            ClaimSelector::Subtree(path) => Some(path),
                            _ => None,
                        },
                    )?,
                    installed,
                ) {
                    EditStatus::Current
                } else {
                    EditStatus::Modified
                },
            );
        }
    }
    for owned in receipt.claims() {
        if !requirements_match(revision.data(), owned.schema_requirements())? {
            statuses.insert(owned.claim().clone(), EditStatus::Modified);
        }
    }
    receipt
        .claims()
        .iter()
        .map(|owned| {
            Ok(ClaimObservation {
                claim_id: owned.claim_id().to_owned(),
                bundle_id: owned.bundle_id().to_owned(),
                artifact_id: owned.artifact_id().to_owned(),
                version: owned.version().to_owned(),
                bundle_digest: owned.bundle_digest().to_owned(),
                render_digest: owned.render_digest().to_owned(),
                status: *statuses
                    .get(owned.claim())
                    .ok_or_else(|| ValueError::new("claim observation is missing"))?,
            })
        })
        .collect()
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn summary(reference: &ResourceRef) -> Result<ResourceObservation, TargetError> {
    let recovery_paths = validate_state(reference)?;
    let receipt: Option<Receipt> = read(&reference.state_root().join("receipt.json"), "Receipt")?;
    if receipt
        .as_ref()
        .is_some_and(|receipt| receipt.resource() != reference)
    {
        return Err(failure(
            ErrorCode::InvalidState,
            "receipt resource disagrees",
            reference.state_root(),
        ));
    }
    let current = observe(reference.destination())?;
    let claims = receipt.as_ref().map_or_else(
        || Ok(Vec::new()),
        |receipt| {
            claim_observations(receipt, &current)
                .map_err(|error| invalid(error, reference.destination()))
        },
    )?;
    Ok(ResourceObservation {
        destination: reference.destination().to_path_buf(),
        state_root: reference.state_root().to_path_buf(),
        claims,
        content_digest: current.content_digest(),
        recovery_paths,
    })
}

#[allow(clippy::too_many_arguments)]
fn owned_claim(
    artifact: &RenderedArtifact,
    bundle: &Bundle,
    rendered: &RenderedBundle,
    reference: &ResourceRef,
    requirements: Vec<(PathBuf, String)>,
    stable_requirements: Vec<(PathBuf, String)>,
    selection: Option<OwnedSelection>,
    installed: Option<Revision>,
    disposition: Disposition,
    baseline: Option<Revision>,
) -> Result<OwnedClaim, ValueError> {
    let claim = artifact_claim(artifact)?;
    OwnedClaim::new(
        claim.clone(),
        claim.identity(&reference.recorded_authority())?,
        bundle.id().to_owned(),
        artifact.id().to_owned(),
        bundle.version().to_owned(),
        bundle.content_digest().to_owned(),
        rendered.content_digest().to_owned(),
        selection,
        installed,
        disposition,
        baseline,
        requirements,
        stable_requirements,
        artifact.schema_requirements().to_vec(),
    )
}

fn artifact_requirements(
    artifact: &RenderedArtifact,
    requirements: &RequirementsByArtifact,
    stable_requirements: &RequirementsByArtifact,
) -> Result<(Requirements, Requirements), ValueError> {
    Ok((
        requirements
            .get(artifact.id())
            .cloned()
            .ok_or_else(|| ValueError::new("planned artifact requirements are missing"))?,
        stable_requirements
            .get(artifact.id())
            .cloned()
            .ok_or_else(|| ValueError::new("planned artifact requirements are missing"))?,
    ))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn container_provenance(
    before: &Revision,
    after: &Revision,
    previous: Option<&Receipt>,
    claims: &[OwnedClaim],
) -> Result<(Revision, Vec<Node>), ValueError> {
    let existing = before
        .nodes()
        .iter()
        .map(|node| (node.path(), node))
        .collect::<HashMap<_, _>>();
    let mut created = previous
        .into_iter()
        .flat_map(Receipt::created_directories)
        .filter(|node| existing.get(node.path()).copied() == Some(*node))
        .map(|node| (node.path().to_owned(), node.clone()))
        .collect::<HashMap<_, _>>();
    let selectors = claims
        .iter()
        .filter_map(|claim| match claim.claim().selector() {
            ClaimSelector::Subtree(selector) => Some(selector.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut nodes = after
        .nodes()
        .iter()
        .map(|node| (node.path().to_owned(), node.clone()))
        .collect::<HashMap<_, _>>();
    for node in after.nodes() {
        if node.data().is_none()
            && !existing.contains_key(node.path())
            && selectors.iter().any(|selector| {
                node.path().is_empty() || selector.starts_with(&format!("{}/", node.path()))
            })
        {
            created.insert(node.path().to_owned(), node.clone());
        }
    }
    let mut retained = Vec::new();
    let mut paths = created.keys().cloned().collect::<Vec<_>>();
    paths.sort_by(|left, right| right.cmp(left));
    for path in paths {
        let still_owned = selectors
            .iter()
            .any(|selector| path.is_empty() || selector.starts_with(&format!("{path}/")));
        if still_owned {
            if let Some(marker) = created.get(&path) {
                retained.push(marker.clone());
            }
        } else if !nodes.keys().any(|other| {
            other != &path && (path.is_empty() || other.starts_with(&format!("{path}/")))
        }) {
            nodes.remove(&path);
        }
    }
    let mut final_nodes = nodes.into_values().collect::<Vec<_>>();
    final_nodes.sort_by(|left, right| left.path().cmp(right.path()));
    retained.sort_by(|left, right| left.path().cmp(right.path()));
    Ok((Revision::new(final_nodes)?, retained))
}

fn transaction_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(time.to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    hasher.update(sequence.to_be_bytes());
    crate::authority::sha256_hex(&hasher.finalize())
        .chars()
        .take(32)
        .collect()
}

#[allow(
    dead_code,
    clippy::if_not_else,
    clippy::redundant_pub_crate,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub(crate) fn plan_resource(
    reference: &ResourceRef,
    identifier: &str,
    bundle: Option<&Bundle>,
    rendered: &RenderedBundle,
    artifacts: &[RenderedArtifact],
    acquisition: Acquisition,
    replace_modified: bool,
    requirements: &RequirementsByArtifact,
    stable_requirements: &RequirementsByArtifact,
) -> Result<ResourcePlan, TargetError> {
    let recovery_paths = validate_state(reference)?;
    let previous: Option<Receipt> = read(&reference.state_root().join("receipt.json"), "Receipt")?;
    if previous
        .as_ref()
        .is_some_and(|receipt| receipt.resource() != reference)
    {
        return Err(failure(
            ErrorCode::InvalidState,
            "receipt resource disagrees",
            reference.state_root(),
        ));
    }
    let before = observe(reference.destination())?;
    let state_revision = observe(reference.state_root())?;
    let claims = previous
        .as_ref()
        .map_or_else(Vec::new, |receipt| receipt.claims().to_vec());
    let mut provenance = previous
        .as_ref()
        .map_or_else(DocumentProvenance::default, |receipt| {
            receipt.provenance().clone()
        });
    let mut after = before.clone();
    let mut error = None;
    let own = claims
        .iter()
        .filter(|owned| owned.bundle_id() == identifier)
        .map(|owned| (owned.claim().clone(), owned))
        .collect::<HashMap<_, _>>();
    let desired = artifacts
        .iter()
        .map(|artifact| artifact_claim(artifact).map(|claim| (claim, artifact)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|value| invalid(value, reference.destination()))?;
    if desired
        .iter()
        .map(|(claim, _)| claim)
        .collect::<HashSet<_>>()
        .len()
        != artifacts.len()
    {
        return Err(failure(
            ErrorCode::InvalidState,
            "rendered artifacts duplicate a physical claim",
            reference.destination(),
        ));
    }
    for (claim, _) in &desired {
        if claims.iter().any(|existing| {
            claim.overlaps(existing.claim())
                && (existing.bundle_id() != identifier || claim != existing.claim())
        }) {
            error = Some(TargetError::new(
                ErrorCode::Conflict,
                "claim overlaps recorded ownership",
                Some(reference.destination().to_path_buf()),
                None,
            ));
        }
    }
    let required = artifacts.iter().any(RenderedArtifact::require_existing);
    if required && before.nodes().is_empty() {
        error = Some(TargetError::new(
            ErrorCode::Conflict,
            "resource must already exist",
            Some(reference.destination().to_path_buf()),
            None,
        ));
    }
    if !before.nodes().is_empty()
        && (before.data().is_some()) != (reference.kind() == ResourceKind::Document)
    {
        error = Some(TargetError::new(
            ErrorCode::UnsafePath,
            "resource kind disagrees with existing destination",
            Some(reference.destination().to_path_buf()),
            None,
        ));
    }
    let mut result = claims
        .iter()
        .filter(|owned| owned.bundle_id() != identifier)
        .cloned()
        .collect::<Vec<_>>();
    let active_schemas = result
        .iter()
        .flat_map(|owned| owned.schema_requirements().iter().cloned())
        .chain(
            artifacts
                .iter()
                .flat_map(|artifact| artifact.schema_requirements().iter().cloned()),
        )
        .collect::<Vec<_>>();
    let shared_document = reference.kind() == ResourceKind::Document
        && own
            .keys()
            .chain(desired.iter().map(|(claim, _)| claim))
            .any(|claim| matches!(claim.kind(), Ownership::Section | Ownership::Structured));
    if shared_document {
        if !before.nodes().is_empty() && before.data().is_none() {
            error = Some(TargetError::new(
                ErrorCode::UnsafePath,
                "shared document is not a file",
                Some(reference.destination().to_path_buf()),
                None,
            ));
        }
        let mut selections = claims
            .iter()
            .filter_map(|owned| owned.selection().cloned())
            .collect::<Vec<_>>();
        let mut requests = own
            .keys()
            .filter(|claim| !desired.iter().any(|(wanted, _)| wanted == *claim))
            .map(|claim| DocumentRequest::retire(claim.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|value| invalid(value, reference.destination()))?;
        for artifact in artifacts {
            if !matches!(
                artifact.content(),
                Content::Section(_) | Content::Structured(_)
            ) {
                error = Some(TargetError::new(
                    ErrorCode::Conflict,
                    "whole and partial claims cannot share a document",
                    Some(reference.destination().to_path_buf()),
                    None,
                ));
            } else {
                requests.push(
                    DocumentRequest::set(artifact.content().clone(), acquisition)
                        .map_err(|value| invalid(value, reference.destination()))?,
                );
            }
        }
        if error.is_none() {
            let mut edit = edit_planned_document(
                before.data().map(<[u8]>::to_vec),
                &requests,
                &selections,
                &provenance,
                &active_schemas,
                !artifacts.is_empty(),
            )
            .map_err(|value| invalid(value, reference.destination()))?;
            if replace_modified && !edit.applicable() {
                for observation in edit
                    .observations()
                    .iter()
                    .filter(|item| item.status() == EditStatus::Modified)
                {
                    if let Some(current) = observation.current()
                        && let Some(selection) = selections
                            .iter_mut()
                            .find(|item| item.claim() == observation.claim())
                    {
                        *selection = selection
                            .with_installed(current.clone())
                            .map_err(|value| invalid(value, reference.destination()))?;
                    }
                }
                edit = edit_planned_document(
                    before.data().map(<[u8]>::to_vec),
                    &requests,
                    &selections,
                    &provenance,
                    &active_schemas,
                    !artifacts.is_empty(),
                )
                .map_err(|value| invalid(value, reference.destination()))?;
            }
            if !edit.applicable() {
                if let Some(issue) = edit.observations().iter().find(|item| {
                    matches!(item.status(), EditStatus::Modified | EditStatus::Conflict)
                }) {
                    error = Some(TargetError::new(
                        if issue.status() == EditStatus::Modified {
                            ErrorCode::Modified
                        } else {
                            ErrorCode::Conflict
                        },
                        if issue.message().is_empty() {
                            "owned selection cannot be changed"
                        } else {
                            issue.message()
                        },
                        Some(reference.destination().to_path_buf()),
                        None,
                    ));
                }
            } else {
                after = match edit.after() {
                    None => Revision::new(Vec::new()),
                    Some(bytes) => Revision::new(vec![
                        if let Some(node) = before.nodes().first() {
                            rebase_node(
                                &Node::new(
                                    node.path().to_owned(),
                                    Some(bytes.to_vec()),
                                    node.mode().clone(),
                                    node.uid().clone(),
                                    node.gid().clone(),
                                    node.device().clone(),
                                    node.inode().clone(),
                                    node.security().to_vec(),
                                )
                                .map_err(|value| invalid(value, reference.destination()))?,
                                String::new(),
                            )
                        } else {
                            new_node(String::new(), Some(bytes.to_vec()), 0o600)
                        }
                        .map_err(|value| invalid(value, reference.destination()))?,
                    ]),
                }
                .map_err(|value| invalid(value, reference.destination()))?;
                provenance = edit.provenance().clone();
                for (claim, artifact) in &desired {
                    let Some(bundle) = bundle else {
                        return Err(failure(
                            ErrorCode::InvalidState,
                            "desired content requires a bundle",
                            reference.destination(),
                        ));
                    };
                    let selection = edit
                        .ownership()
                        .iter()
                        .find(|owned| owned.claim() == claim)
                        .cloned()
                        .ok_or_else(|| {
                            failure(
                                ErrorCode::InvalidState,
                                "planned selection ownership is missing",
                                reference.destination(),
                            )
                        })?;
                    let (artifact_requirements, artifact_stable_requirements) =
                        artifact_requirements(artifact, requirements, stable_requirements)
                            .map_err(|value| invalid(value, reference.destination()))?;
                    result.push(
                        owned_claim(
                            artifact,
                            bundle,
                            rendered,
                            reference,
                            artifact_requirements,
                            artifact_stable_requirements,
                            Some(selection.clone()),
                            None,
                            selection.disposition(),
                            None,
                        )
                        .map_err(|value| invalid(value, reference.destination()))?,
                    );
                }
            }
        }
    } else if error.is_none() {
        let mut all_claims = own
            .keys()
            .chain(desired.iter().map(|(claim, _)| claim))
            .cloned()
            .collect::<Vec<_>>();
        all_claims.sort_by_key(|claim| {
            claim
                .identity(&reference.recorded_authority())
                .unwrap_or_default()
        });
        all_claims.dedup();
        for claim in all_claims {
            let old = own.get(&claim).copied();
            let selected_artifact = desired
                .iter()
                .find(|(candidate, _)| candidate == &claim)
                .map(|(_, artifact)| *artifact);
            let actual = selected(
                &before,
                match claim.selector() {
                    ClaimSelector::Subtree(path) => Some(path),
                    _ => None,
                },
            )
            .map_err(|value| invalid(value, reference.destination()))?;
            if old.is_some_and(|owned| {
                owned
                    .installed()
                    .is_some_and(|installed| !intact(&actual, installed))
            }) && !replace_modified
            {
                error = Some(TargetError::new(
                    ErrorCode::Modified,
                    "owned resource has local modifications",
                    Some(reference.destination().to_path_buf()),
                    None,
                ));
                break;
            }
            let replacement;
            if let Some(artifact) = selected_artifact {
                let Some(bundle) = bundle else {
                    return Err(failure(
                        ErrorCode::InvalidState,
                        "desired content requires a bundle",
                        reference.destination(),
                    ));
                };
                let (artifact_requirements, artifact_stable_requirements) =
                    artifact_requirements(artifact, requirements, stable_requirements)
                        .map_err(|value| invalid(value, reference.destination()))?;
                replacement = desired_revision(artifact.content())
                    .map_err(|value| invalid(value, reference.destination()))?;
                let mut disposition = old.map_or(Disposition::Created, OwnedClaim::disposition);
                let mut baseline = old.and_then(|owned| owned.baseline().cloned());
                if old.is_none() && !actual.nodes().is_empty() {
                    match acquisition {
                        Acquisition::Conflict => {
                            error = Some(TargetError::new(
                                ErrorCode::Conflict,
                                "unowned content already exists",
                                Some(reference.destination().to_path_buf()),
                                None,
                            ));
                            break;
                        }
                        Acquisition::Adopt if !intact(&actual, &replacement) => {
                            error = Some(TargetError::new(
                                ErrorCode::Conflict,
                                "adoption requires matching content",
                                Some(reference.destination().to_path_buf()),
                                None,
                            ));
                            break;
                        }
                        Acquisition::Adopt => disposition = Disposition::Adopted,
                        Acquisition::Takeover => {
                            disposition = Disposition::TakenOver;
                            baseline = Some(actual.clone());
                        }
                    }
                }
                result.push(
                    owned_claim(
                        artifact,
                        bundle,
                        rendered,
                        reference,
                        artifact_requirements,
                        artifact_stable_requirements,
                        None,
                        Some(replacement.clone()),
                        disposition,
                        baseline,
                    )
                    .map_err(|value| invalid(value, reference.destination()))?,
                );
            } else {
                replacement = old
                    .and_then(|owned| owned.baseline().cloned())
                    .unwrap_or_else(Revision::empty);
            }
            if !intact(&actual, &replacement) {
                after = replace_selection(&after, &claim, &replacement)
                    .map_err(|value| invalid(value, reference.destination()))?;
            }
        }
    }
    let mut created_directories = previous
        .as_ref()
        .map_or_else(Vec::new, |receipt| receipt.created_directories().to_vec());
    if reference.kind() == ResourceKind::SkillContainer && error.is_none() {
        (after, created_directories) =
            container_provenance(&before, &after, previous.as_ref(), &result)
                .map_err(|value| invalid(value, reference.destination()))?;
    }
    result.sort_by(|left, right| left.claim_id().cmp(right.claim_id()));
    let mut receipt = if error.is_none() {
        Receipt::new(
            reference.clone(),
            transaction_id(),
            result,
            provenance,
            created_directories,
            2,
        )
        .map_err(|value| invalid(value, reference.destination()))?
    } else {
        previous.clone().unwrap_or(
            Receipt::new(
                reference.clone(),
                transaction_id(),
                Vec::new(),
                DocumentProvenance::default(),
                Vec::new(),
                2,
            )
            .map_err(|value| invalid(value, reference.destination()))?,
        )
    };
    if previous
        .as_ref()
        .is_some_and(|old| receipt.equivalent_except_transaction(old))
    {
        receipt = previous.clone().unwrap_or(receipt);
    }
    ResourcePlan::new(
        reference.clone(),
        before.clone(),
        if error.is_none() { after } else { before },
        previous,
        receipt,
        ancestors(&reference.state_root().join("transaction.json"))?,
        state_revision,
        recovery_paths,
        required,
        error,
    )
    .map_err(|value| invalid(value, reference.destination()))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn state_root_order(resources: &mut [ResourceRef]) {
    resources.sort_by(|left, right| {
        crate::codec::posix_path(left.state_root())
            .cmp(&crate::codec::posix_path(right.state_root()))
    });
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn is_source_free_removal(receipt: &Receipt, bundle_id: &str) -> bool {
    receipt
        .claims()
        .iter()
        .all(|claim| claim.bundle_id() == bundle_id)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn edit_planned_document(
    data: Option<Vec<u8>>,
    requests: &[DocumentRequest],
    ownership: &[OwnedSelection],
    provenance: &DocumentProvenance,
    active_schemas: &[DocumentSchema],
    validate: bool,
) -> Result<DocumentEdit, ValueError> {
    crate::schema::edit_schema_document(
        data,
        requests,
        ownership,
        provenance,
        active_schemas,
        validate,
    )
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn requirements_match(
    data: Option<&[u8]>,
    requirements: &[DocumentSchema],
) -> Result<bool, ValueError> {
    requirements
        .iter()
        .map(|requirement| crate::schema::matches(data, requirement))
        .try_fold(true, |all, matches| matches.map(|matches| all && matches))
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn completes_pending_index(current: Option<&Index>, prepared: Option<&Index>) -> bool {
    current.is_some_and(|current| {
        (!current.residual().is_empty()
            || current.pending().is_some()
            || current.previous().is_some())
            && prepared
                == Index::new(
                    current.bundle_id().to_owned(),
                    current.context_digest().to_owned(),
                    current.pending().cloned(),
                    None,
                    None,
                    Vec::new(),
                    2,
                )
                .ok()
                .as_ref()
    })
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepared_index_promotes(
    current: Option<&Index>,
    prepared: &Index,
) -> Result<bool, ValueError> {
    if Some(prepared) == current || completes_pending_index(current, Some(prepared)) {
        return Ok(false);
    }
    let previous = current.and_then(Index::current);
    let previous_refs = current
        .into_iter()
        .flat_map(Index::resources)
        .cloned()
        .collect::<HashSet<_>>();
    let desired_refs = prepared
        .pending()
        .into_iter()
        .flat_map(Generation::membership)
        .map(|(reference, _)| reference.clone())
        .collect::<HashSet<_>>();
    let residual = prepared.residual().iter().cloned().collect::<HashSet<_>>();
    if prepared.current() == previous
        && prepared.previous() == previous
        && residual == previous_refs.union(&desired_refs).cloned().collect()
    {
        return Ok(true);
    }
    Err(ValueError::new(
        "unrecognized installation index preparation retained",
    ))
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn index_with_prepared_resources(
    current: Option<&Index>,
    prepared: Option<&Index>,
    identifier: &str,
    context_digest: &str,
) -> Result<Option<Index>, ValueError> {
    let Some(prepared) = prepared else {
        return Ok(current.cloned());
    };
    let mut resources = current
        .into_iter()
        .flat_map(Index::resources)
        .chain(prepared.resources())
        .cloned()
        .collect::<Vec<_>>();
    state_root_order(&mut resources);
    resources.dedup();
    Index::new(
        identifier.to_owned(),
        context_digest.to_owned(),
        current.and_then(Index::current).cloned(),
        current.and_then(Index::pending).cloned(),
        current.and_then(Index::previous).cloned(),
        resources,
        2,
    )
    .map(Some)
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn expects_receipt(
    index: Option<&Index>,
    prepared: Option<&Index>,
    reference: &ResourceRef,
) -> bool {
    let expected = index
        .into_iter()
        .flat_map(|index| [index.current(), index.previous()].into_iter().flatten());
    let completed = prepared
        .filter(|prepared| completes_pending_index(index, Some(prepared)))
        .and_then(Index::current);
    expected.chain(completed).any(|generation| {
        generation
            .membership()
            .iter()
            .any(|(member, _)| member == reference)
    })
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn pending_index(
    identifier: &str,
    context_digest: &str,
    previous: Option<&Index>,
    generation: Option<Generation>,
    resources: Vec<ResourceRef>,
) -> Result<Index, ValueError> {
    Index::new(
        identifier.to_owned(),
        context_digest.to_owned(),
        previous.and_then(Index::current).cloned(),
        generation,
        previous.and_then(Index::current).cloned(),
        resources,
        2,
    )
}

#[allow(dead_code, clippy::redundant_pub_crate)]
pub(crate) fn completed_index(
    identifier: &str,
    context_digest: &str,
    generation: Option<Generation>,
) -> Result<Index, ValueError> {
    Index::new(
        identifier.to_owned(),
        context_digest.to_owned(),
        generation,
        None,
        None,
        Vec::new(),
        2,
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use crate::artifacts::Family;
    use crate::content::{FileContent, SectionContent};
    use crate::models::BundleEntry;
    use crate::resource_io::{admit, reserve};
    use crate::resource_transaction::{ResourceLockGuard, cleanup, prepare, publish};
    use crate::test_support::checkout_temp_dir;
    use std::time::Duration;

    fn bundle() -> Bundle {
        Bundle::from_memory(
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}"#,
            [BundleEntry::file(
                "review/SKILL.md",
                b"# Review\n".to_vec(),
            )
            .expect("entry")],
        )
        .expect("bundle")
    }

    fn rendered_file(path: PathBuf, data: &[u8]) -> (RenderedBundle, RenderedArtifact) {
        let artifact = RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path,
            Content::File(FileContent::new(data, FileMode::Private)),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact");
        let rendered = RenderedBundle::new([artifact.clone()], [], []).expect("rendered bundle");
        (rendered, artifact)
    }

    fn requirements(
        artifact: &RenderedArtifact,
    ) -> (RequirementsByArtifact, RequirementsByArtifact) {
        (
            HashMap::from([(artifact.id().to_owned(), Vec::new())]),
            HashMap::from([(artifact.id().to_owned(), Vec::new())]),
        )
    }

    #[test]
    fn plans_creation_observation_and_source_free_removal() {
        let tmp = checkout_temp_dir("resource-plan-file");
        let (rendered, artifact) = rendered_file(tmp.join("guide"), b"generated\n");
        let reference = resource(&artifact).expect("resource");
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let source = bundle();
        let (requirements, stable_requirements) = requirements(&artifact);
        let missing = plan_resource(
            &reference,
            "team",
            Some(&source),
            &rendered,
            std::slice::from_ref(&artifact),
            Acquisition::Conflict,
            false,
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect_err("dependency maps are required");
        assert_eq!(
            missing.message(),
            "planned artifact requirements are missing"
        );
        let plan = plan_resource(
            &reference,
            "team",
            Some(&source),
            &rendered,
            &[artifact],
            Acquisition::Conflict,
            false,
            &requirements,
            &stable_requirements,
        )
        .expect("plan");
        assert!(plan.error().is_none());
        assert_eq!(plan.after().data(), Some(b"generated\n".as_slice()));
        assert_eq!(
            plan.receipt().claims()[0].disposition(),
            Disposition::Created
        );
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&plan, &guard).expect("prepare");
        publish(&journal).expect("publish");
        cleanup(&journal).expect("cleanup");
        let observation = summary(&reference).expect("summary");
        assert_eq!(observation.claims[0].status, EditStatus::Current);
        assert!(observation.recovery_paths.is_empty());

        let empty = RenderedBundle::empty();
        let removal = plan_resource(
            &reference,
            "team",
            None,
            &empty,
            &[],
            Acquisition::Conflict,
            false,
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("removal");
        assert!(is_source_free_removal(
            removal.previous().expect("previous"),
            "team"
        ));
        assert!(removal.after().nodes().is_empty());
        assert!(removal.receipt().claims().is_empty());
    }

    #[test]
    fn plans_shared_document_edits_and_detects_local_modification() {
        let tmp = checkout_temp_dir("resource-plan-section");
        let destination = tmp.join("AGENTS.md");
        std::fs::write(&destination, b"foreign\n").expect("foreign");
        let content =
            Content::Section(SectionContent::new("guide", "generated", None).expect("section"));
        let artifact = RenderedArtifact::new(
            "guide",
            Family::Instructions,
            destination,
            content,
            None,
            false,
            Vec::new(),
        )
        .expect("artifact");
        let rendered = RenderedBundle::new([artifact.clone()], [], []).expect("rendered bundle");
        let reference = resource(&artifact).expect("resource");
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let source = bundle();
        let (requirements, stable_requirements) = requirements(&artifact);
        let first = plan_resource(
            &reference,
            "team",
            Some(&source),
            &rendered,
            std::slice::from_ref(&artifact),
            Acquisition::Conflict,
            false,
            &requirements,
            &stable_requirements,
        )
        .expect("plan");
        assert!(first.error().is_none());
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&first, &guard).expect("prepare");
        publish(&journal).expect("publish");
        cleanup(&journal).expect("cleanup");
        std::fs::write(
            reference.destination(),
            b"foreign\n\n<!-- flyrail:guide:start -->\nchanged\n<!-- flyrail:guide:end -->\n",
        )
        .expect("local edit");
        let modified = plan_resource(
            &reference,
            "team",
            Some(&source),
            &rendered,
            &[artifact],
            Acquisition::Conflict,
            false,
            &requirements,
            &stable_requirements,
        )
        .expect("modified plan");
        assert_eq!(
            modified.error().map(TargetError::code),
            Some(ErrorCode::Modified)
        );
    }

    #[test]
    fn index_transition_kernel_preserves_membership_and_receipt_requirements() {
        let tmp = checkout_temp_dir("resource-plan-index");
        let (rendered, artifact) = rendered_file(tmp.join("guide"), b"x");
        let reference = resource(&artifact).expect("resource");
        let claim = artifact_claim(&artifact).expect("claim");
        let generation = Generation::new(
            "1".to_owned(),
            "a".repeat(64),
            rendered.content_digest().to_owned(),
            vec![(
                reference.clone(),
                vec![
                    claim
                        .identity(&reference.recorded_authority())
                        .expect("identity"),
                ],
            )],
        )
        .expect("generation");
        let current =
            completed_index("team", &"b".repeat(64), Some(generation.clone())).expect("current");
        let pending = pending_index(
            "team",
            &"b".repeat(64),
            Some(&current),
            Some(generation.clone()),
            vec![reference.clone()],
        )
        .expect("pending");
        assert!(prepared_index_promotes(Some(&current), &pending).expect("transition"));
        let effective =
            index_with_prepared_resources(Some(&current), Some(&pending), "team", &"b".repeat(64))
                .expect("effective")
                .expect("index");
        assert_eq!(effective.resources(), vec![&reference]);
        assert!(expects_receipt(Some(&current), Some(&pending), &reference));
        let completed =
            completed_index("team", &"b".repeat(64), Some(generation)).expect("completed");
        assert!(completes_pending_index(Some(&pending), Some(&completed)));
    }

    #[test]
    fn prepared_directory_markers_are_not_written_as_logical_placeholders() {
        let tmp = checkout_temp_dir("resource-plan-marker");
        let destination = tmp.join("skills");
        let artifact = RenderedArtifact::new(
            "skill",
            Family::Skills,
            destination,
            Content::Tree(
                crate::content::TreeContent::new([BundleEntry::file(
                    "SKILL.md",
                    b"# Review\n".to_vec(),
                )
                .expect("file")])
                .expect("tree"),
            ),
            Some("review".to_owned()),
            false,
            Vec::new(),
        )
        .expect("artifact");
        let rendered = RenderedBundle::new([artifact.clone()], [], []).expect("rendered bundle");
        let reference = resource(&artifact).expect("resource");
        reserve(&reference).expect("reserve");
        admit(&reference).expect("admit");
        let (requirements, stable_requirements) = requirements(&artifact);
        let plan = plan_resource(
            &reference,
            "team",
            Some(&bundle()),
            &rendered,
            &[artifact],
            Acquisition::Conflict,
            false,
            &requirements,
            &stable_requirements,
        )
        .expect("plan");
        assert!(
            plan.receipt()
                .created_directories()
                .iter()
                .all(|node| node.inode().as_u64() == Some(0))
        );
        let guard = ResourceLockGuard::acquire(&reference, Duration::ZERO).expect("lock");
        let journal = prepare(&plan, &guard).expect("prepare");
        assert!(
            journal
                .receipt()
                .created_directories()
                .iter()
                .all(|node| node.inode().as_u64().is_some_and(|inode| inode != 0))
        );
    }
}
