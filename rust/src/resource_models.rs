use crate::authority::{AuthorityError, Ownership, ResourceAuthority};
use crate::content::{Content, DocumentSchema};
use crate::editors::{Disposition, DocumentProvenance, OwnedSelection, content_claim};
use crate::json::Json;
use crate::validation::{validate_identifier, validate_relative_path, validate_version};
use crate::values::ValueError;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Count {
    Small(i64),
    Big(String),
}

impl From<i64> for Count {
    fn from(value: i64) -> Self {
        Self::Small(value)
    }
}

impl Count {
    pub(crate) fn is_negative(&self) -> bool {
        match self {
            Self::Small(value) => *value < 0,
            Self::Big(text) => text.starts_with('-'),
        }
    }

    pub(crate) const fn exceeds(&self, cap: i64) -> bool {
        match self {
            Self::Small(value) => *value > cap,
            Self::Big(_) => true,
        }
    }

    pub(crate) fn to_json(&self) -> Json {
        match self {
            Self::Small(value) => Json::Int(*value),
            Self::Big(text) => Json::BigInt(text.clone()),
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Small(value) => u32::try_from(*value).ok(),
            Self::Big(text) => text.parse().ok(),
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Small(value) => u64::try_from(*value).ok(),
            Self::Big(text) => text.parse().ok(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResourceKind {
    Document,
    Tree,
    SkillContainer,
}

impl ResourceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Tree => "tree",
            Self::SkillContainer => "skill-container",
        }
    }
}

impl FromStr for ResourceKind {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "document" => Ok(Self::Document),
            "tree" => Ok(Self::Tree),
            "skill-container" => Ok(Self::SkillContainer),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid ResourceKind"
            ))),
        }
    }
}

pub fn hexadecimal(value: &str, length: usize) -> Result<(), ValueError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValueError::new("invalid hexadecimal identity"));
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<(), ValueError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(ValueError::new("expected an absolute concrete path"));
    }
    if path.parent().is_none() {
        return Err(ValueError::new("filesystem roots are not resources"));
    }
    Ok(())
}

pub fn digest(record: &Json) -> String {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(crate::codec::encode_state(record));
    crate::authority::sha256_hex(&hasher.finalize())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ancestor {
    path: PathBuf,
    device: Count,
    inode: Count,
    mode: Count,
    uid: Count,
    gid: Count,
    security: Vec<u8>,
}

impl Ancestor {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        path: PathBuf,
        device: Count,
        inode: Count,
        mode: Count,
        uid: Count,
        gid: Count,
        security: impl Into<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        if !path.is_absolute() {
            return Err(ValueError::new("ancestor path must be absolute"));
        }
        if device.is_negative()
            || inode.is_negative()
            || mode.is_negative()
            || uid.is_negative()
            || gid.is_negative()
            || mode.exceeds(0o7777)
        {
            return Err(ValueError::new("invalid ancestor metadata"));
        }
        Ok(Self {
            path,
            device,
            inode,
            mode,
            uid,
            gid,
            security: security.into(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn device(&self) -> &Count {
        &self.device
    }

    pub const fn inode(&self) -> &Count {
        &self.inode
    }

    pub const fn mode(&self) -> &Count {
        &self.mode
    }

    pub const fn uid(&self) -> &Count {
        &self.uid
    }

    pub const fn gid(&self) -> &Count {
        &self.gid
    }

    pub fn security(&self) -> &[u8] {
        &self.security
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    path: String,
    data: Option<Vec<u8>>,
    mode: Count,
    uid: Count,
    gid: Count,
    device: Count,
    inode: Count,
    security: Vec<u8>,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        path: String,
        data: Option<Vec<u8>>,
        mode: Count,
        uid: Count,
        gid: Count,
        device: Count,
        inode: Count,
        security: impl Into<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        if !path.is_empty() {
            validate_relative_path(&path, "revision path")?;
        }
        if mode.is_negative()
            || uid.is_negative()
            || gid.is_negative()
            || device.is_negative()
            || inode.is_negative()
            || mode.exceeds(0o777)
        {
            return Err(ValueError::new("invalid revision metadata"));
        }
        Ok(Self {
            path,
            data,
            mode,
            uid,
            gid,
            device,
            inode,
            security: security.into(),
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn data(&self) -> Option<&[u8]> {
        self.data.as_deref()
    }

    pub const fn mode(&self) -> &Count {
        &self.mode
    }

    pub const fn uid(&self) -> &Count {
        &self.uid
    }

    pub const fn gid(&self) -> &Count {
        &self.gid
    }

    pub const fn device(&self) -> &Count {
        &self.device
    }

    pub const fn inode(&self) -> &Count {
        &self.inode
    }

    pub fn security(&self) -> &[u8] {
        &self.security
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Revision {
    nodes: Vec<Node>,
}

impl Revision {
    pub(crate) const fn empty() -> Self {
        Self { nodes: Vec::new() }
    }

    pub(crate) fn new(nodes: Vec<Node>) -> Result<Self, ValueError> {
        let mut by_path: HashMap<&str, &Node> = HashMap::with_capacity(nodes.len());
        for node in &nodes {
            if by_path.insert(node.path(), node).is_some() {
                return Err(ValueError::new("revision must have exactly one root"));
            }
        }
        if nodes.first().is_some_and(|node| !node.path().is_empty()) {
            return Err(ValueError::new("revision must have exactly one root"));
        }
        for node in nodes.iter().skip(1) {
            let parent_path = node.path().rsplit_once('/').map_or("", |(head, _)| head);
            if by_path
                .get(parent_path)
                .copied()
                .is_none_or(|parent| parent.data().is_some())
            {
                return Err(ValueError::new("revision parents must be directories"));
            }
        }
        let paths: Vec<&str> = nodes.iter().map(Node::path).collect();
        let mut ordered = paths.clone();
        ordered.sort_unstable();
        if ordered != paths {
            return Err(ValueError::new("revision entries must be ordered"));
        }
        Ok(Self { nodes })
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn data(&self) -> Option<&[u8]> {
        self.nodes.first().and_then(Node::data)
    }

    pub fn content_digest(&self) -> String {
        let record = Json::Array(
            self.nodes
                .iter()
                .map(|node| {
                    Json::Array(vec![
                        Json::Str(node.path().to_owned()),
                        node.data().map_or(Json::Null, crate::codec::encoded_bytes),
                        node.mode().to_json(),
                    ])
                })
                .collect(),
        );
        digest(&record)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceRef {
    destination: PathBuf,
    kind: ResourceKind,
    schema_version: i64,
}

impl ResourceRef {
    pub fn new(destination: PathBuf, kind: ResourceKind) -> Result<Self, ValueError> {
        Self::with_schema(destination, kind, 2)
    }

    pub(crate) fn with_schema(
        destination: PathBuf,
        kind: ResourceKind,
        schema_version: i64,
    ) -> Result<Self, ValueError> {
        if schema_version != 2 {
            return Err(ValueError::new("authority schema must be 2"));
        }
        absolute(&destination)?;
        Ok(Self {
            destination,
            kind,
            schema_version,
        })
    }

    pub(crate) fn authority_checked(&self) -> Result<ResourceAuthority, AuthorityError> {
        let result = ResourceAuthority::new(&self.destination)?;
        if result.destination() != self.destination {
            return Err(ValueError::new("resource canonical destination changed").into());
        }
        Ok(result)
    }

    #[allow(dead_code)]
    pub(crate) fn authority(&self) -> Result<ResourceAuthority, ValueError> {
        self.authority_checked()
            .map_err(|error| ValueError::new(error.message()))
    }

    pub(crate) fn recorded_authority(&self) -> ResourceAuthority {
        ResourceAuthority::unchecked(self.destination.clone())
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }

    pub const fn schema_version(&self) -> i64 {
        self.schema_version
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preparation {
    resource: ResourceRef,
    transaction_id: String,
    directory: PathBuf,
    schema_version: i64,
}

impl Preparation {
    pub fn new(
        resource: ResourceRef,
        transaction_id: String,
        directory: PathBuf,
    ) -> Result<Self, ValueError> {
        Self::with_schema(resource, transaction_id, directory, 2)
    }

    pub(crate) fn with_schema(
        resource: ResourceRef,
        transaction_id: String,
        directory: PathBuf,
        schema_version: i64,
    ) -> Result<Self, ValueError> {
        hexadecimal(&transaction_id, 32)?;
        absolute(&directory)?;
        if schema_version != 2 {
            return Err(ValueError::new("preparation schema must be 2"));
        }
        Ok(Self {
            resource,
            transaction_id,
            directory,
            schema_version,
        })
    }

    pub const fn resource(&self) -> &ResourceRef {
        &self.resource
    }

    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub const fn schema_version(&self) -> i64 {
        self.schema_version
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedClaim {
    claim: crate::authority::Claim,
    claim_id: String,
    bundle_id: String,
    artifact_id: String,
    version: String,
    bundle_digest: String,
    render_digest: String,
    selection: Option<OwnedSelection>,
    installed: Option<Revision>,
    disposition: Disposition,
    baseline: Option<Revision>,
    requirements: Vec<(PathBuf, String)>,
    stable_requirements: Vec<(PathBuf, String)>,
    schema_requirements: Vec<DocumentSchema>,
}

impl OwnedClaim {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        claim: crate::authority::Claim,
        claim_id: String,
        bundle_id: String,
        artifact_id: String,
        version: String,
        bundle_digest: String,
        render_digest: String,
        selection: Option<OwnedSelection>,
        installed: Option<Revision>,
        disposition: Disposition,
        baseline: Option<Revision>,
        requirements: Vec<(PathBuf, String)>,
        stable_requirements: Vec<(PathBuf, String)>,
        schema_requirements: Vec<DocumentSchema>,
    ) -> Result<Self, ValueError> {
        hexadecimal(&claim_id, 64)?;
        validate_identifier(&bundle_id, "owner")?;
        validate_identifier(&artifact_id, "artifact")?;
        validate_version(&version)?;
        hexadecimal(&bundle_digest, 64)?;
        hexadecimal(&render_digest, 64)?;
        check_requirements(
            &requirements,
            &stable_requirements,
            &schema_requirements,
            selection.as_ref(),
        )?;
        check_ownership_shape(
            &claim,
            selection.as_ref(),
            installed.as_ref(),
            baseline.as_ref(),
            disposition,
        )?;
        Ok(Self {
            claim,
            claim_id,
            bundle_id,
            artifact_id,
            version,
            bundle_digest,
            render_digest,
            selection,
            installed,
            disposition,
            baseline,
            requirements,
            stable_requirements,
            schema_requirements,
        })
    }
}

fn check_requirements(
    requirements: &[(PathBuf, String)],
    stable_requirements: &[(PathBuf, String)],
    schema_requirements: &[DocumentSchema],
    selection: Option<&OwnedSelection>,
) -> Result<(), ValueError> {
    for (path, identifier) in requirements {
        absolute(path)?;
        hexadecimal(identifier, 64)?;
    }
    let mut seen_requirements: HashSet<(&Path, &str)> = HashSet::with_capacity(requirements.len());
    if !requirements
        .iter()
        .all(|(path, identifier)| seen_requirements.insert((path.as_path(), identifier.as_str())))
    {
        return Err(ValueError::new("duplicate claim requirement"));
    }
    let mut seen_stable: HashSet<(&Path, &str)> = HashSet::with_capacity(stable_requirements.len());
    if !stable_requirements.iter().all(|(path, identifier)| {
        seen_stable.insert((path.as_path(), identifier.as_str()))
            && seen_requirements.contains(&(path.as_path(), identifier.as_str()))
    }) {
        return Err(ValueError::new(
            "stable requirements must be a unique subset of requirements",
        ));
    }
    let mut schema_keys: HashSet<&str> = HashSet::with_capacity(schema_requirements.len());
    if !schema_requirements
        .iter()
        .all(|schema| schema_keys.insert(schema.key()))
    {
        return Err(ValueError::new("duplicate claim schema requirement"));
    }
    if !schema_requirements.is_empty() {
        let Some(selection) = selection else {
            return Err(ValueError::new(
                "schema requirements need matching document selections",
            ));
        };
        if schema_requirements
            .iter()
            .any(|schema| Some(schema.format()) != selection.format())
        {
            return Err(ValueError::new(
                "schema requirements need matching document selections",
            ));
        }
    }
    Ok(())
}

fn check_ownership_shape(
    claim: &crate::authority::Claim,
    selection: Option<&OwnedSelection>,
    installed: Option<&Revision>,
    baseline: Option<&Revision>,
    disposition: Disposition,
) -> Result<(), ValueError> {
    if matches!(claim.kind(), Ownership::Section | Ownership::Structured) {
        let Some(selection) = selection else {
            return Err(ValueError::new(
                "shared claims require matching selection ownership",
            ));
        };
        if selection.claim() != claim {
            return Err(ValueError::new(
                "shared claims require matching selection ownership",
            ));
        }
        if installed.is_some() || baseline.is_some() {
            return Err(ValueError::new(
                "shared claims cannot own complete revisions",
            ));
        }
        if disposition != selection.disposition() {
            return Err(ValueError::new("selection disposition disagrees"));
        }
    } else {
        if selection.is_some() {
            return Err(ValueError::new(
                "whole claims require an installed revision",
            ));
        }
        let Some(installed) = installed else {
            return Err(ValueError::new(
                "whole claims require an installed revision",
            ));
        };
        if installed.nodes().is_empty() {
            return Err(ValueError::new("owned revisions cannot be absent"));
        }
        if (disposition == Disposition::TakenOver) != baseline.is_some() {
            return Err(ValueError::new("only takeover has a baseline"));
        }
        if let Some(baseline) = baseline
            && baseline.nodes().is_empty()
        {
            return Err(ValueError::new("takeover needs an original revision"));
        }
    }
    Ok(())
}

impl OwnedClaim {
    pub const fn claim(&self) -> &crate::authority::Claim {
        &self.claim
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

    pub const fn selection(&self) -> Option<&OwnedSelection> {
        self.selection.as_ref()
    }

    pub const fn installed(&self) -> Option<&Revision> {
        self.installed.as_ref()
    }

    pub const fn disposition(&self) -> Disposition {
        self.disposition
    }

    pub const fn baseline(&self) -> Option<&Revision> {
        self.baseline.as_ref()
    }

    pub fn requirements(&self) -> &[(PathBuf, String)] {
        &self.requirements
    }

    pub fn stable_requirements(&self) -> &[(PathBuf, String)] {
        &self.stable_requirements
    }

    pub fn schema_requirements(&self) -> &[DocumentSchema] {
        &self.schema_requirements
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    resource: ResourceRef,
    transaction_id: String,
    claims: Vec<OwnedClaim>,
    provenance: DocumentProvenance,
    created_directories: Vec<Node>,
    schema_version: i64,
}

impl Receipt {
    pub(crate) fn new(
        resource: ResourceRef,
        transaction_id: String,
        claims: Vec<OwnedClaim>,
        provenance: DocumentProvenance,
        created_directories: Vec<Node>,
        schema_version: i64,
    ) -> Result<Self, ValueError> {
        if schema_version != 2 {
            return Err(ValueError::new("receipt schema must be 2"));
        }
        hexadecimal(&transaction_id, 32)?;
        if !created_directories.is_empty() && resource.kind() != ResourceKind::SkillContainer {
            return Err(ValueError::new(
                "directory provenance requires a skill container",
            ));
        }
        if created_directories.iter().any(|node| node.data().is_some()) {
            return Err(ValueError::new(
                "created directory evidence must contain distinct directories",
            ));
        }
        let mut seen_directories: HashSet<&str> = HashSet::with_capacity(created_directories.len());
        if !created_directories
            .iter()
            .all(|node| seen_directories.insert(node.path()))
        {
            return Err(ValueError::new(
                "created directory evidence must contain distinct directories",
            ));
        }
        let authority = resource.recorded_authority();
        let allowed: &[Ownership] = match resource.kind() {
            ResourceKind::Document => &[Ownership::File, Ownership::Section, Ownership::Structured],
            ResourceKind::Tree => &[Ownership::Tree],
            ResourceKind::SkillContainer => &[Ownership::Subtree],
        };
        for (index, owned) in claims.iter().enumerate() {
            if owned.claim_id() != owned.claim().identity(&authority)? {
                return Err(ValueError::new(
                    "claim identity disagrees with physical destination",
                ));
            }
            if claims
                .iter()
                .take(index)
                .any(|other| owned.claim().overlaps(other.claim()))
            {
                return Err(ValueError::new("receipt claims overlap"));
            }
            if !allowed.contains(&owned.claim().kind()) {
                return Err(ValueError::new("claim kind disagrees with resource kind"));
            }
        }
        let mut schemas: HashSet<&DocumentSchema> = HashSet::new();
        for owned in &claims {
            for schema in owned.schema_requirements() {
                schemas.insert(schema);
            }
        }
        let mut schema_keys: HashSet<&str> = HashSet::with_capacity(schemas.len());
        if !schemas
            .iter()
            .all(|schema| schema_keys.insert(schema.key()))
        {
            return Err(ValueError::new(
                "receipt has incompatible schema requirements",
            ));
        }
        for schema in &schemas {
            let claim = content_claim(&Content::Structured(schema.content()?))?;
            if claims.iter().any(|owned| claim.overlaps(owned.claim())) {
                return Err(ValueError::new("schema requirement overlaps app ownership"));
            }
        }
        Ok(Self {
            resource,
            transaction_id,
            claims,
            provenance,
            created_directories,
            schema_version,
        })
    }

    pub const fn resource(&self) -> &ResourceRef {
        &self.resource
    }

    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    pub fn claims(&self) -> &[OwnedClaim] {
        &self.claims
    }

    pub const fn provenance(&self) -> &DocumentProvenance {
        &self.provenance
    }

    pub fn created_directories(&self) -> &[Node] {
        &self.created_directories
    }

    pub const fn schema_version(&self) -> i64 {
        self.schema_version
    }

    pub(crate) fn with_created_directories(
        &self,
        created_directories: Vec<Node>,
    ) -> Result<Self, ValueError> {
        Self::new(
            self.resource.clone(),
            self.transaction_id.clone(),
            self.claims.clone(),
            self.provenance.clone(),
            created_directories,
            self.schema_version,
        )
    }

    pub(crate) fn equivalent_except_transaction(&self, other: &Self) -> bool {
        self.resource == other.resource
            && self.claims == other.claims
            && self.provenance == other.provenance
            && self.created_directories == other.created_directories
            && self.schema_version == other.schema_version
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generation {
    version: String,
    bundle_digest: String,
    render_digest: String,
    membership: Vec<(ResourceRef, Vec<String>)>,
}

impl Generation {
    pub(crate) fn new(
        version: String,
        bundle_digest: String,
        render_digest: String,
        membership: Vec<(ResourceRef, Vec<String>)>,
    ) -> Result<Self, ValueError> {
        validate_version(&version)?;
        hexadecimal(&bundle_digest, 64)?;
        hexadecimal(&render_digest, 64)?;
        for (_reference, identifiers) in &membership {
            for identifier in identifiers {
                hexadecimal(identifier, 64)?;
            }
            let mut seen_identifiers: HashSet<&str> = HashSet::with_capacity(identifiers.len());
            if !identifiers
                .iter()
                .all(|identifier| seen_identifiers.insert(identifier.as_str()))
            {
                return Err(ValueError::new("duplicate membership claim"));
            }
        }
        let mut destinations: HashSet<&Path> = HashSet::with_capacity(membership.len());
        if !membership
            .iter()
            .all(|(reference, _)| destinations.insert(reference.destination()))
        {
            return Err(ValueError::new("duplicate generation resource"));
        }
        Ok(Self {
            version,
            bundle_digest,
            render_digest,
            membership,
        })
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

    pub fn membership(&self) -> &[(ResourceRef, Vec<String>)] {
        &self.membership
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Index {
    bundle_id: String,
    context_digest: String,
    current: Option<Generation>,
    pending: Option<Generation>,
    previous: Option<Generation>,
    residual: Vec<ResourceRef>,
    schema_version: i64,
}

impl Index {
    pub(crate) fn new(
        bundle_id: String,
        context_digest: String,
        current: Option<Generation>,
        pending: Option<Generation>,
        previous: Option<Generation>,
        residual: Vec<ResourceRef>,
        schema_version: i64,
    ) -> Result<Self, ValueError> {
        validate_identifier(&bundle_id, "index owner")?;
        hexadecimal(&context_digest, 64)?;
        if schema_version != 2 {
            return Err(ValueError::new("index schema must be 2"));
        }
        let index = Self {
            bundle_id,
            context_digest,
            current,
            pending,
            previous,
            residual,
            schema_version,
        };
        let resources = index.resources();
        let mut destinations: HashSet<&Path> = HashSet::with_capacity(resources.len());
        if !resources
            .iter()
            .all(|reference| destinations.insert(reference.destination()))
        {
            return Err(ValueError::new("index has conflicting resource kinds"));
        }
        Ok(index)
    }

    pub fn resources(&self) -> Vec<&ResourceRef> {
        let mut seen: HashSet<&ResourceRef> = self.residual.iter().collect();
        let mut references: Vec<&ResourceRef> = self.residual.iter().collect();
        for generation in [&self.current, &self.pending, &self.previous]
            .into_iter()
            .flatten()
        {
            for (reference, _) in &generation.membership {
                if seen.insert(reference) {
                    references.push(reference);
                }
            }
        }
        references.sort_by(|left, right| {
            crate::codec::posix_path(left.destination())
                .cmp(&crate::codec::posix_path(right.destination()))
        });
        references
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub fn context_digest(&self) -> &str {
        &self.context_digest
    }

    pub const fn current(&self) -> Option<&Generation> {
        self.current.as_ref()
    }

    pub const fn pending(&self) -> Option<&Generation> {
        self.pending.as_ref()
    }

    pub const fn previous(&self) -> Option<&Generation> {
        self.previous.as_ref()
    }

    pub fn residual(&self) -> &[ResourceRef] {
        &self.residual
    }

    pub const fn schema_version(&self) -> i64 {
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
    use crate::authority::ClaimSelector;
    use crate::content::{DocumentFormat, DocumentSchema, Key, Selector, SelectorPart};
    use crate::editors::{Disposition, OwnedSelection, SelectionSnapshot, SnapshotValue};
    use crate::values::{Scalar, Value};

    fn node(path: &str, data: Option<&[u8]>, mode: i64) -> Node {
        Node::new(
            path.to_owned(),
            data.map(<[u8]>::to_vec),
            mode.into(),
            0.into(),
            0.into(),
            0.into(),
            0.into(),
            Vec::new(),
        )
        .expect("test node")
    }

    #[test]
    fn revision_content_digests_match_python() {
        let foreign =
            Revision::new(vec![node("", Some(b"Foreign\n"), 0o600)]).expect("foreign revision");
        assert_eq!(
            foreign.content_digest(),
            "cc391149a8acd4875b9350fa3a00bd7ab74c6f22a51bedf0ee3cecab2b598af2"
        );
        let empty = Revision::new(vec![node("", Some(b""), 0o600)]).expect("empty revision");
        assert_eq!(
            empty.content_digest(),
            "0bd98d2117cf8a3e6724a4619042c40342c3403f88b6eb856b060fc034b8a349"
        );
    }

    #[test]
    fn revision_validation_messages_match_python() {
        let duplicate = Revision::new(vec![
            node("", Some(b"root"), 0o600),
            node("", Some(b"dupe"), 0o600),
        ]);
        assert_eq!(
            duplicate.unwrap_err().message(),
            "revision must have exactly one root"
        );
        let missing_parent = Revision::new(vec![
            node("", Some(b"root"), 0o600),
            node("a/b", Some(b"orphan"), 0o600),
        ]);
        assert_eq!(
            missing_parent.unwrap_err().message(),
            "revision parents must be directories"
        );
        let unordered = Revision::new(vec![
            node("", None, 0o755),
            node("b", None, 0o755),
            node("a", Some(b"file"), 0o600),
            node("b/c", Some(b"file"), 0o600),
        ]);
        assert_eq!(
            unordered.unwrap_err().message(),
            "revision entries must be ordered"
        );
    }

    #[test]
    fn metadata_validation_messages_match_python() {
        let bad_mode = Node::new(
            String::new(),
            Some(b"x".to_vec()),
            0o1000.into(),
            0.into(),
            0.into(),
            0.into(),
            0.into(),
            Vec::new(),
        );
        assert_eq!(bad_mode.unwrap_err().message(), "invalid revision metadata");
        let bad_ancestor = Ancestor::new(
            PathBuf::from("/tmp/ancestor"),
            0.into(),
            0.into(),
            0o100_000.into(),
            0.into(),
            0.into(),
            Vec::new(),
        );
        assert_eq!(
            bad_ancestor.unwrap_err().message(),
            "invalid ancestor metadata"
        );
        let relative = Ancestor::new(
            PathBuf::from("relative"),
            0.into(),
            0.into(),
            0o755.into(),
            0.into(),
            0.into(),
            Vec::new(),
        );
        assert_eq!(
            relative.unwrap_err().message(),
            "ancestor path must be absolute"
        );
    }

    #[test]
    fn resource_ref_validation_messages_match_python() {
        let destination = PathBuf::from("/fixture/project/AGENTS.md");
        let wrong_schema = ResourceRef::with_schema(destination, ResourceKind::Document, 1);
        assert_eq!(
            wrong_schema.unwrap_err().message(),
            "authority schema must be 2"
        );
        let relative = ResourceRef::new(PathBuf::from("relative"), ResourceKind::Document);
        assert_eq!(
            relative.unwrap_err().message(),
            "expected an absolute concrete path"
        );
    }

    #[test]
    fn resource_authorities_agree_with_the_recorded_destination() {
        let destination = PathBuf::from("/fixture/project/AGENTS.md");
        let resource =
            ResourceRef::new(destination.clone(), ResourceKind::Document).expect("resource");
        let authority = resource.authority().expect("authority");
        assert_eq!(authority.destination(), destination);
        assert_eq!(resource.recorded_authority(), authority);
    }

    fn whole_claim() -> crate::authority::Claim {
        crate::authority::Claim::new(crate::authority::Ownership::File, ClaimSelector::Whole)
            .expect("whole claim")
    }

    fn structured_claim(key: &str) -> crate::authority::Claim {
        crate::authority::Claim::new(
            crate::authority::Ownership::Structured,
            ClaimSelector::Structured(
                Selector::new([SelectorPart::Key(Key::new(key).expect("key"))]).expect("selector"),
            ),
        )
        .expect("structured claim")
    }

    fn selection_claim() -> crate::authority::Claim {
        structured_claim("mcpServers")
    }

    fn structured_selection(key: &str) -> OwnedSelection {
        OwnedSelection::new(
            structured_claim(key),
            Some(DocumentFormat::Json),
            SelectionSnapshot::new(
                SnapshotValue::Typed(Value::Scalar(Scalar::text("value"))),
                b"\"value\"",
                b"\"value\"",
                None,
                b"",
            )
            .expect("snapshot"),
            Disposition::Created,
            None,
            b"",
        )
        .expect("owned selection")
    }

    fn owned_selection() -> OwnedSelection {
        structured_selection("mcpServers")
    }

    fn revision() -> Revision {
        Revision::new(vec![node("", Some(b"x"), 0o600)]).expect("revision")
    }

    fn absent_revision() -> Revision {
        Revision::new(Vec::new()).expect("absent revision")
    }

    #[allow(clippy::too_many_arguments)]
    fn owned_claim_parts(
        claim: crate::authority::Claim,
        claim_id: String,
        selection: Option<OwnedSelection>,
        installed: Option<Revision>,
        disposition: Disposition,
        baseline: Option<Revision>,
        requirements: Vec<(PathBuf, String)>,
        stable_requirements: Vec<(PathBuf, String)>,
        schema_requirements: Vec<DocumentSchema>,
    ) -> Result<OwnedClaim, crate::values::ValueError> {
        OwnedClaim::new(
            claim,
            claim_id,
            "owner".to_owned(),
            "artifact".to_owned(),
            "1".to_owned(),
            "a".repeat(64),
            "b".repeat(64),
            selection,
            installed,
            disposition,
            baseline,
            requirements,
            stable_requirements,
            schema_requirements,
        )
    }

    fn owned_claim(
        claim: crate::authority::Claim,
        selection: Option<OwnedSelection>,
        installed: Option<Revision>,
        disposition: Disposition,
        baseline: Option<Revision>,
    ) -> Result<OwnedClaim, crate::values::ValueError> {
        owned_claim_parts(
            claim,
            "1".repeat(64),
            selection,
            installed,
            disposition,
            baseline,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    #[test]
    fn owned_claim_validation_messages_match_python() {
        let missing = owned_claim(whole_claim(), None, None, Disposition::Created, None);
        assert_eq!(
            missing.unwrap_err().message(),
            "whole claims require an installed revision"
        );
        let takeover = owned_claim(
            whole_claim(),
            None,
            Some(revision()),
            Disposition::TakenOver,
            None,
        );
        assert_eq!(
            takeover.unwrap_err().message(),
            "only takeover has a baseline"
        );
        let shared_owns = owned_claim(
            selection_claim(),
            Some(owned_selection()),
            Some(revision()),
            Disposition::Created,
            None,
        );
        assert_eq!(
            shared_owns.unwrap_err().message(),
            "shared claims cannot own complete revisions"
        );
        let ok = owned_claim(
            whole_claim(),
            None,
            Some(revision()),
            Disposition::Created,
            None,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn receipt_validation_messages_match_python() {
        let destination = PathBuf::from("/fixture/project/AGENTS.md");
        let resource = ResourceRef::new(destination, ResourceKind::Document).expect("resource");
        let ok = Receipt::new(
            resource.clone(),
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert!(ok.is_ok());
        let bad_transaction = Receipt::new(
            resource.clone(),
            "1".repeat(20),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert_eq!(
            bad_transaction.unwrap_err().message(),
            "invalid hexadecimal identity"
        );
        let bad_schema = Receipt::new(
            resource,
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            1,
        );
        assert_eq!(
            bad_schema.unwrap_err().message(),
            "receipt schema must be 2"
        );
    }

    #[test]
    fn generation_and_index_validation_messages_match_python() {
        let destination = PathBuf::from("/fixture/project/AGENTS.md");
        let resource = ResourceRef::new(destination, ResourceKind::Document).expect("resource");
        let generation = Generation::new(
            "1".to_owned(),
            "a".repeat(64),
            "b".repeat(64),
            vec![(resource.clone(), vec!["c".repeat(64)])],
        );
        assert!(generation.is_ok());
        let dup_ids = Generation::new(
            "1".to_owned(),
            "a".repeat(64),
            "b".repeat(64),
            vec![(resource.clone(), vec!["c".repeat(64), "c".repeat(64)])],
        );
        assert_eq!(dup_ids.unwrap_err().message(), "duplicate membership claim");
        let index_ok = Index::new(
            "owner".to_owned(),
            "d".repeat(64),
            generation.ok(),
            None,
            None,
            Vec::new(),
            2,
        );
        assert!(index_ok.is_ok());
        let dup_resources = Index::new(
            "owner".to_owned(),
            "d".repeat(64),
            None,
            None,
            None,
            vec![resource.clone(), resource],
            2,
        );
        assert_eq!(
            dup_resources.unwrap_err().message(),
            "index has conflicting resource kinds"
        );
    }

    #[test]
    fn journal_validation_messages_match_python() {
        let destination = PathBuf::from("/fixture/project/AGENTS.md");
        let resource = ResourceRef::new(destination, ResourceKind::Document).expect("resource");
        let receipt = Receipt::new(
            resource.clone(),
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        )
        .expect("receipt");
        let revision = Revision::new(vec![node("", Some(b"x"), 0o600)]).expect("revision");
        let ok = crate::resource_transaction::Journal::new(
            resource.clone(),
            None,
            receipt.clone(),
            revision.clone(),
            revision.clone(),
            revision.clone(),
            revision.clone(),
            Vec::new(),
            2,
        );
        assert!(ok.is_ok());

        let mismatched = crate::resource_transaction::Journal::new(
            resource.clone(),
            None,
            receipt.clone(),
            Revision::new(vec![node("", Some(b"before"), 0o600)]).expect("before"),
            revision.clone(),
            revision.clone(),
            Revision::new(vec![node("", Some(b"backup"), 0o600)]).expect("backup"),
            Vec::new(),
            2,
        );
        assert_eq!(
            mismatched.unwrap_err().message(),
            "journal private revisions must retain complete content"
        );

        let wrong_schema = crate::resource_transaction::Journal::new(
            resource,
            None,
            receipt,
            revision.clone(),
            revision.clone(),
            revision.clone(),
            revision,
            Vec::new(),
            1,
        );
        assert_eq!(
            wrong_schema.unwrap_err().message(),
            "journal schema must be 2"
        );
    }

    #[test]
    fn journal_receipt_resources_must_agree() {
        let destination = PathBuf::from("/fixture/project/AGENTS.md");
        let resource = ResourceRef::new(destination, ResourceKind::Document).expect("resource");
        let other_destination = PathBuf::from("/fixture/project/OTHER.md");
        let other =
            ResourceRef::new(other_destination, ResourceKind::Document).expect("other resource");
        let receipt = Receipt::new(
            other,
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        )
        .expect("other receipt");
        let disagree = crate::resource_transaction::Journal::new(
            resource,
            None,
            receipt,
            revision(),
            revision(),
            revision(),
            revision(),
            Vec::new(),
            2,
        );
        assert_eq!(
            disagree.unwrap_err().message(),
            "journal receipt resources disagree"
        );
    }

    #[test]
    fn index_resources_are_deduplicated_and_destination_sorted() {
        let resource = |name: &str| {
            let destination = PathBuf::from(format!("/fixture/project/{name}.md"));
            ResourceRef::new(destination, ResourceKind::Document).expect("resource")
        };
        let a = resource("a");
        let m = resource("m");
        let z = resource("z");
        let generation = Generation::new(
            "1".to_owned(),
            "a".repeat(64),
            "b".repeat(64),
            vec![
                (m.clone(), vec!["c".repeat(64)]),
                (a.clone(), vec!["c".repeat(64)]),
            ],
        )
        .expect("generation");
        let index = Index::new(
            "owner".to_owned(),
            "d".repeat(64),
            Some(generation),
            None,
            None,
            vec![z.clone(), m.clone()],
            2,
        )
        .expect("index");
        let order: Vec<&Path> = index.resources().iter().map(|r| r.destination()).collect();
        assert_eq!(
            order,
            vec![a.destination(), m.destination(), z.destination()]
        );
    }

    #[test]
    fn node_paths_use_the_revision_path_label() {
        let absolute = Node::new(
            "/absolute".to_owned(),
            None,
            0o600.into(),
            0.into(),
            0.into(),
            0.into(),
            0.into(),
            Vec::new(),
        );
        assert_eq!(
            absolute.unwrap_err().message(),
            "revision path must be an unambiguous relative path"
        );
        let nonportable = Node::new(
            "a<b".to_owned(),
            None,
            0o600.into(),
            0.into(),
            0.into(),
            0.into(),
            0.into(),
            Vec::new(),
        );
        assert_eq!(
            nonportable.unwrap_err().message(),
            "revision path contains a nonportable character"
        );
    }

    #[test]
    fn metadata_boundaries_match_python() {
        let node = Node::new(
            "a".to_owned(),
            Some(b"x".to_vec()),
            0o777.into(),
            0.into(),
            0.into(),
            0.into(),
            0.into(),
            Vec::new(),
        )
        .expect("node at the mode cap");
        assert_eq!(node.mode(), &Count::Small(0o777));
        let ancestor = Ancestor::new(
            PathBuf::from("/a"),
            0.into(),
            0.into(),
            0o7777.into(),
            0.into(),
            0.into(),
            Vec::new(),
        )
        .expect("ancestor at the mode cap");
        assert_eq!(ancestor.mode(), &Count::Small(0o7777));
    }

    #[test]
    fn revision_data_returns_the_root_node_payload() {
        assert_eq!(revision().data(), Some(b"x".as_slice()));
        assert_eq!(absent_revision().data(), None);
    }

    #[test]
    fn owned_claim_requirement_messages_match_python() {
        let hex = "c".repeat(64);
        let duplicate = owned_claim_parts(
            whole_claim(),
            "1".repeat(64),
            None,
            None,
            Disposition::Created,
            None,
            vec![
                (PathBuf::from("/a"), hex.clone()),
                (PathBuf::from("/a"), hex.clone()),
            ],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            duplicate.unwrap_err().message(),
            "duplicate claim requirement"
        );
        let stale = owned_claim_parts(
            whole_claim(),
            "1".repeat(64),
            None,
            None,
            Disposition::Created,
            None,
            vec![(PathBuf::from("/a"), hex.clone())],
            vec![(PathBuf::from("/b"), hex)],
            Vec::new(),
        );
        assert_eq!(
            stale.unwrap_err().message(),
            "stable requirements must be a unique subset of requirements"
        );
        let schema_a =
            DocumentSchema::new(DocumentFormat::Json, "a", Scalar::text("1")).expect("schema");
        let schema_a2 =
            DocumentSchema::new(DocumentFormat::Json, "a", Scalar::text("2")).expect("schema");
        let duplicate_schema = owned_claim_parts(
            whole_claim(),
            "1".repeat(64),
            None,
            None,
            Disposition::Created,
            None,
            Vec::new(),
            Vec::new(),
            vec![schema_a.clone(), schema_a2],
        );
        assert_eq!(
            duplicate_schema.unwrap_err().message(),
            "duplicate claim schema requirement"
        );
        let without_selection = owned_claim_parts(
            structured_claim("b"),
            "1".repeat(64),
            None,
            None,
            Disposition::Created,
            None,
            Vec::new(),
            Vec::new(),
            vec![schema_a],
        );
        assert_eq!(
            without_selection.unwrap_err().message(),
            "schema requirements need matching document selections"
        );
    }

    #[test]
    fn owned_claim_selection_messages_match_python() {
        let disposition = owned_claim_parts(
            structured_claim("b"),
            "1".repeat(64),
            Some(structured_selection("b")),
            None,
            Disposition::Adopted,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            disposition.unwrap_err().message(),
            "selection disposition disagrees"
        );
        let shared = owned_claim_parts(
            structured_claim("c"),
            "1".repeat(64),
            Some(structured_selection("b")),
            None,
            Disposition::Created,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            shared.unwrap_err().message(),
            "shared claims require matching selection ownership"
        );
        let absent = owned_claim(
            whole_claim(),
            None,
            Some(absent_revision()),
            Disposition::Created,
            None,
        );
        assert_eq!(
            absent.unwrap_err().message(),
            "owned revisions cannot be absent"
        );
        let takeover = owned_claim(
            whole_claim(),
            None,
            Some(revision()),
            Disposition::TakenOver,
            Some(absent_revision()),
        );
        assert_eq!(
            takeover.unwrap_err().message(),
            "takeover needs an original revision"
        );
    }

    fn document_resource(path: &str) -> ResourceRef {
        let destination = PathBuf::from(path);
        ResourceRef::new(destination, ResourceKind::Document).expect("resource")
    }

    #[test]
    fn receipt_claim_validations_match_python() {
        let resource = document_resource("/fixture/project/AGENTS.md");
        let authority = resource.recorded_authority();
        let whole_owned = |claim: crate::authority::Claim, claim_id: String| {
            owned_claim_parts(
                claim,
                claim_id,
                None,
                Some(revision()),
                Disposition::Created,
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .expect("owned claim")
        };
        let wrong_identity = Receipt::new(
            resource.clone(),
            "1".repeat(32),
            vec![whole_owned(whole_claim(), "0".repeat(64))],
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert_eq!(
            wrong_identity.unwrap_err().message(),
            "claim identity disagrees with physical destination"
        );
        let tree =
            crate::authority::Claim::new(crate::authority::Ownership::Tree, ClaimSelector::Whole)
                .expect("tree claim");
        let tree_id = tree.identity(&authority).expect("identity");
        let wrong_kind = Receipt::new(
            resource.clone(),
            "1".repeat(32),
            vec![whole_owned(tree, tree_id)],
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert_eq!(
            wrong_kind.unwrap_err().message(),
            "claim kind disagrees with resource kind"
        );
        let file_id = whole_claim().identity(&authority).expect("identity");
        let overlap = Receipt::new(
            resource,
            "1".repeat(32),
            vec![
                whole_owned(whole_claim(), file_id.clone()),
                whole_owned(whole_claim(), file_id),
            ],
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert_eq!(overlap.unwrap_err().message(), "receipt claims overlap");
    }

    #[test]
    fn receipt_schema_requirement_messages_match_python() {
        let resource = document_resource("/fixture/project/AGENTS.md");
        let authority = resource.recorded_authority();
        let structured_owned = |key: &str, schema: DocumentSchema| {
            let claim = structured_claim(key);
            let claim_id = claim.identity(&authority).expect("identity");
            owned_claim_parts(
                claim,
                claim_id,
                Some(structured_selection(key)),
                None,
                Disposition::Created,
                None,
                Vec::new(),
                Vec::new(),
                vec![schema],
            )
            .expect("structured claim")
        };
        let schema_a =
            DocumentSchema::new(DocumentFormat::Json, "a", Scalar::text("1")).expect("schema");
        let schema_a2 =
            DocumentSchema::new(DocumentFormat::Json, "a", Scalar::text("2")).expect("schema");
        let incompatible = Receipt::new(
            resource.clone(),
            "1".repeat(32),
            vec![
                structured_owned("b", schema_a),
                structured_owned("c", schema_a2),
            ],
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert_eq!(
            incompatible.unwrap_err().message(),
            "receipt has incompatible schema requirements"
        );
        let schema_overlap = Receipt::new(
            resource,
            "1".repeat(32),
            vec![structured_owned(
                "a",
                DocumentSchema::new(DocumentFormat::Json, "a", Scalar::text("1")).expect("schema"),
            )],
            crate::editors::DocumentProvenance::default(),
            Vec::new(),
            2,
        );
        assert_eq!(
            schema_overlap.unwrap_err().message(),
            "schema requirement overlaps app ownership"
        );
    }

    #[test]
    fn receipt_directory_evidence_messages_match_python() {
        let directories = Receipt::new(
            document_resource("/fixture/project/AGENTS.md"),
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            vec![node("d", None, 0o755)],
            2,
        );
        assert_eq!(
            directories.unwrap_err().message(),
            "directory provenance requires a skill container"
        );
        let skills = ResourceRef::new(
            PathBuf::from("/fixture/project/skills"),
            ResourceKind::SkillContainer,
        )
        .expect("skill resource");
        let with_data = Receipt::new(
            skills.clone(),
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            vec![node("d", Some(b"x"), 0o755)],
            2,
        );
        assert_eq!(
            with_data.unwrap_err().message(),
            "created directory evidence must contain distinct directories"
        );
        let duplicate_directories = Receipt::new(
            skills,
            "1".repeat(32),
            Vec::new(),
            crate::editors::DocumentProvenance::default(),
            vec![node("d", None, 0o755), node("d", None, 0o755)],
            2,
        );
        assert_eq!(
            duplicate_directories.unwrap_err().message(),
            "created directory evidence must contain distinct directories"
        );
    }
}
