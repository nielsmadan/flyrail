use crate::authority::{AuthorityError, canonical_destination};
use crate::bundle::Bundle;
use crate::editors::{Acquisition, EditStatus};
use crate::json::Json;
use crate::models::OperationStatus;
use crate::observations::TargetError;
use crate::rendered::{Notice, RenderedBundle};
use crate::resource_models::{Ancestor, Generation, Index, ResourceRef, Revision, digest};
use crate::validation::validate_identifier;
use crate::values::ValueError;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub use crate::resource_plan::ResourcePlan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationTarget {
    index_root: PathBuf,
    context: Vec<(String, String)>,
    routing_context: Vec<(String, String)>,
}

impl InstallationTarget {
    pub fn new(
        index_root: impl AsRef<Path>,
        context: impl IntoIterator<Item = (String, String)>,
        routing_context: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, AuthorityError> {
        let index_root = canonical_destination(index_root)?;
        let mut context = context.into_iter().collect::<Vec<_>>();
        let mut routing_context = routing_context.into_iter().collect::<Vec<_>>();
        check_context(&context)?;
        check_context(&routing_context)?;
        context.sort();
        routing_context.sort();
        Ok(Self {
            index_root,
            context,
            routing_context,
        })
    }

    pub fn without_context(index_root: impl AsRef<Path>) -> Result<Self, AuthorityError> {
        Self::new(index_root, std::iter::empty(), std::iter::empty())
    }

    pub fn index_root(&self) -> &Path {
        &self.index_root
    }

    pub fn context(&self) -> &[(String, String)] {
        &self.context
    }

    pub fn routing_context(&self) -> &[(String, String)] {
        &self.routing_context
    }

    pub fn context_digest(&self) -> String {
        digest(&Json::Array(vec![
            crate::codec::encoded_path(&self.index_root),
            Json::Array(
                self.context
                    .iter()
                    .map(|(key, value)| {
                        Json::Array(vec![Json::Str(key.clone()), Json::Str(value.clone())])
                    })
                    .collect(),
            ),
        ]))
    }
}

fn check_context(context: &[(String, String)]) -> Result<(), AuthorityError> {
    let mut keys = HashSet::with_capacity(context.len());
    if !context.iter().all(|(key, _)| keys.insert(key.as_str())) {
        return Err(ValueError::new("target context keys must be unique").into());
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimObservation {
    claim_id: String,
    bundle_id: String,
    artifact_id: String,
    version: String,
    bundle_digest: String,
    render_digest: String,
    status: EditStatus,
}

impl ClaimObservation {
    pub(crate) const fn new(
        claim_id: String,
        bundle_id: String,
        artifact_id: String,
        version: String,
        bundle_digest: String,
        render_digest: String,
        status: EditStatus,
    ) -> Self {
        Self {
            claim_id,
            bundle_id,
            artifact_id,
            version,
            bundle_digest,
            render_digest,
            status,
        }
    }

    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }
    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn bundle_digest(&self) -> &str {
        &self.bundle_digest
    }
    pub fn render_digest(&self) -> &str {
        &self.render_digest
    }
    pub const fn status(&self) -> EditStatus {
        self.status
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceObservation {
    destination: PathBuf,
    state_root: PathBuf,
    claims: Vec<ClaimObservation>,
    revision_digest: Option<String>,
    error: Option<TargetError>,
    recovery_paths: Vec<PathBuf>,
}

impl ResourceObservation {
    pub(crate) const fn observed(
        destination: PathBuf,
        state_root: PathBuf,
        claims: Vec<ClaimObservation>,
        revision_digest: String,
        recovery_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            destination,
            state_root,
            claims,
            revision_digest: Some(revision_digest),
            error: None,
            recovery_paths,
        }
    }

    pub(crate) fn failed(reference: &ResourceRef, error: TargetError) -> Self {
        Self {
            destination: reference.destination().to_path_buf(),
            state_root: reference.state_root().to_path_buf(),
            claims: Vec::new(),
            revision_digest: None,
            error: Some(error),
            recovery_paths: Vec::new(),
        }
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }
    pub fn claims(&self) -> &[ClaimObservation] {
        &self.claims
    }
    pub fn revision_digest(&self) -> Option<&str> {
        self.revision_digest.as_deref()
    }
    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }
    pub fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationObservation {
    bundle_id: String,
    target: InstallationTarget,
    resources: Vec<ResourceObservation>,
    is_current: bool,
    pending: bool,
    version: Option<String>,
    bundle_digest: Option<String>,
    render_digest: Option<String>,
    error: Option<TargetError>,
}

impl InstallationObservation {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        bundle_id: String,
        target: InstallationTarget,
        resources: Vec<ResourceObservation>,
        is_current: bool,
        pending: bool,
        version: Option<String>,
        bundle_digest: Option<String>,
        render_digest: Option<String>,
        error: Option<TargetError>,
    ) -> Self {
        Self {
            bundle_id,
            target,
            resources,
            is_current,
            pending,
            version,
            bundle_digest,
            render_digest,
            error,
        }
    }

    pub fn matches(&self, bundle: &Bundle, rendered: &RenderedBundle) -> bool {
        self.is_current
            && !self.pending
            && self.error.is_none()
            && rendered.supported()
            && self.bundle_id == bundle.id()
            && self.version.as_deref() == Some(bundle.version())
            && self.bundle_digest.as_deref() == Some(bundle.content_digest())
            && self.render_digest.as_deref() == Some(rendered.content_digest())
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }
    pub const fn target(&self) -> &InstallationTarget {
        &self.target
    }
    pub fn resources(&self) -> &[ResourceObservation] {
        &self.resources
    }
    pub const fn is_current(&self) -> bool {
        self.is_current
    }
    pub const fn pending(&self) -> bool {
        self.pending
    }
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
    pub fn bundle_digest(&self) -> Option<&str> {
        self.bundle_digest.as_deref()
    }
    pub fn render_digest(&self) -> Option<&str> {
        self.render_digest.as_deref()
    }
    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceResult {
    resource: ResourceRef,
    status: OperationStatus,
    error: Option<TargetError>,
    recovery_paths: Vec<PathBuf>,
}

impl ResourceResult {
    pub(crate) const fn new(
        resource: ResourceRef,
        status: OperationStatus,
        error: Option<TargetError>,
        recovery_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            resource,
            status,
            error,
            recovery_paths,
        }
    }

    pub const fn resource(&self) -> &ResourceRef {
        &self.resource
    }
    pub const fn status(&self) -> OperationStatus {
        self.status
    }
    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }
    pub fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationResult {
    status: OperationStatus,
    observation: InstallationObservation,
    resources: Vec<ResourceResult>,
    error: Option<TargetError>,
    notices: Vec<Notice>,
}

impl InstallationResult {
    pub(crate) const fn new(
        status: OperationStatus,
        observation: InstallationObservation,
        resources: Vec<ResourceResult>,
        error: Option<TargetError>,
        notices: Vec<Notice>,
    ) -> Self {
        Self {
            status,
            observation,
            resources,
            error,
            notices,
        }
    }

    pub const fn status(&self) -> OperationStatus {
        self.status
    }
    pub const fn observation(&self) -> &InstallationObservation {
        &self.observation
    }
    pub fn resources(&self) -> &[ResourceResult] {
        &self.resources
    }
    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecyclePreview {
    bundle_id: String,
    bundle: Option<Bundle>,
    rendered: RenderedBundle,
    target: InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
    index: Option<Index>,
    index_revision: Option<Revision>,
    index_ancestors: Vec<Ancestor>,
    generation: Option<Generation>,
    resources: Vec<ResourcePlan>,
    error: Option<TargetError>,
}

impl LifecyclePreview {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        bundle_id: String,
        bundle: Option<Bundle>,
        rendered: RenderedBundle,
        target: InstallationTarget,
        acquisition: Acquisition,
        replace_modified: bool,
        index: Option<Index>,
        index_revision: Option<Revision>,
        index_ancestors: Vec<Ancestor>,
        generation: Option<Generation>,
        resources: Vec<ResourcePlan>,
        error: Option<TargetError>,
    ) -> Self {
        Self {
            bundle_id,
            bundle,
            rendered,
            target,
            acquisition,
            replace_modified,
            index,
            index_revision,
            index_ancestors,
            generation,
            resources,
            error,
        }
    }

    pub fn applicable(&self) -> bool {
        self.error.is_none()
            && self
                .resources
                .iter()
                .all(|item| item.error().is_none() && item.recovery_paths().is_empty())
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }
    pub const fn bundle(&self) -> Option<&Bundle> {
        self.bundle.as_ref()
    }
    pub const fn rendered(&self) -> &RenderedBundle {
        &self.rendered
    }
    pub const fn target(&self) -> &InstallationTarget {
        &self.target
    }
    pub const fn acquisition(&self) -> Acquisition {
        self.acquisition
    }
    pub const fn replace_modified(&self) -> bool {
        self.replace_modified
    }
    pub const fn index(&self) -> Option<&Index> {
        self.index.as_ref()
    }
    pub const fn index_revision(&self) -> Option<&Revision> {
        self.index_revision.as_ref()
    }
    pub fn index_ancestors(&self) -> &[Ancestor] {
        &self.index_ancestors
    }
    pub const fn generation(&self) -> Option<&Generation> {
        self.generation.as_ref()
    }
    pub fn resources(&self) -> &[ResourcePlan] {
        &self.resources
    }
    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }

    pub(crate) fn with_observations(
        &self,
        index_revision: Revision,
        index_ancestors: Vec<Ancestor>,
        resources: Vec<ResourcePlan>,
    ) -> Self {
        let mut result = self.clone();
        result.index_revision = Some(index_revision);
        result.index_ancestors = index_ancestors;
        result.resources = resources;
        result
    }
}

pub(crate) fn validate_bundle_id(bundle_id: &str) -> Result<(), ValueError> {
    validate_identifier(bundle_id, "bundle id")
}
