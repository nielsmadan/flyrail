use crate::artifacts::Family;
use crate::content::{Content, DocumentSchema};
use crate::models::BundleEntry;
use crate::validation::validate_identifier;
use crate::values::{ArrayValue, ObjectValue, Scalar, Value, ValueError};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

pub trait ModelValue {
    fn model_value(&self) -> Value;
}

impl ModelValue for Value {
    fn model_value(&self) -> Value {
        match self {
            Self::Scalar(scalar) => {
                typed_record("Scalar", [("value", Self::Scalar(scalar.clone()))])
            }
            Self::Array(array) => typed_record(
                "ArrayValue",
                [(
                    "items",
                    Self::Array(ArrayValue::new(
                        array.items().iter().map(ModelValue::model_value),
                    )),
                )],
            ),
            Self::Object(object) => typed_record(
                "ObjectValue",
                [(
                    "items",
                    Self::Array(ArrayValue::new(object.items().iter().map(|(key, item)| {
                        Self::Array(ArrayValue::new([
                            Self::Scalar(Scalar::text(key.as_str())),
                            item.model_value(),
                        ]))
                    }))),
                )],
            ),
        }
    }
}

impl ModelValue for Scalar {
    fn model_value(&self) -> Value {
        typed_record("Scalar", [("value", Value::Scalar(self.clone()))])
    }
}

impl ModelValue for BundleEntry {
    fn model_value(&self) -> Value {
        let data = self.data().map_or_else(
            || Value::Scalar(Scalar::null()),
            |data| data.to_vec().model_value(),
        );
        typed_record(
            "BundleEntry",
            [
                ("data", data),
                (
                    "executable",
                    Value::Scalar(Scalar::boolean(self.executable())),
                ),
                ("path", Value::Scalar(Scalar::text(self.path()))),
            ],
        )
    }
}

impl ModelValue for str {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self))
    }
}

impl ModelValue for String {
    fn model_value(&self) -> Value {
        self.as_str().model_value()
    }
}

impl ModelValue for bool {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::boolean(*self))
    }
}

impl ModelValue for i32 {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::integer(i64::from(*self)))
    }
}

impl ModelValue for Vec<u8> {
    fn model_value(&self) -> Value {
        let mut hex = String::with_capacity(self.len() * 2);
        for byte in self {
            let _ = write!(hex, "{byte:02x}");
        }
        Value::Object(ObjectValue::from_pairs([(
            "bytes",
            Value::Scalar(Scalar::text(hex)),
        )]))
    }
}

impl<T: ModelValue + ?Sized> ModelValue for Option<&T> {
    fn model_value(&self) -> Value {
        self.as_ref().map_or_else(
            || Value::Scalar(Scalar::null()),
            |inner| inner.model_value(),
        )
    }
}

pub(crate) fn typed_record(
    type_name: &str,
    fields: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    Value::Object(ObjectValue::from_pairs([
        ("fields", Value::Object(ObjectValue::from_pairs(fields))),
        ("type", Value::Scalar(Scalar::text(type_name))),
    ]))
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DependencyMode {
    Revision,
    StableReference,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NoticeKind {
    Prerequisite,
    Activation,
    Unsupported,
}

impl NoticeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prerequisite => "prerequisite",
            Self::Activation => "activation",
            Self::Unsupported => "unsupported",
        }
    }
}

impl FromStr for NoticeKind {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "prerequisite" => Ok(Self::Prerequisite),
            "activation" => Ok(Self::Activation),
            "unsupported" => Ok(Self::Unsupported),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid NoticeKind"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Notice {
    artifact_id: String,
    kind: NoticeKind,
    message: String,
    code: String,
}

impl Notice {
    pub fn new(
        artifact_id: impl Into<String>,
        kind: NoticeKind,
        message: impl Into<String>,
    ) -> Result<Self, ValueError> {
        Self::with_code(artifact_id, kind, message, "")
    }

    pub fn with_code(
        artifact_id: impl Into<String>,
        kind: NoticeKind,
        message: impl Into<String>,
        code: impl Into<String>,
    ) -> Result<Self, ValueError> {
        let artifact_id = artifact_id.into();
        let message = message.into();
        let code = code.into();
        validate_identifier(&artifact_id, "notice artifact id")?;
        if message.trim().is_empty() {
            return Err(ValueError::new("notice message must be nonblank"));
        }
        if !code.is_empty() {
            validate_identifier(&code, "notice code")?;
        }
        Ok(Self {
            artifact_id,
            kind,
            message,
            code,
        })
    }

    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    pub const fn kind(&self) -> NoticeKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl DependencyMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Revision => "revision",
            Self::StableReference => "stable-reference",
        }
    }
}

impl FromStr for DependencyMode {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "revision" => Ok(Self::Revision),
            "stable-reference" => Ok(Self::StableReference),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid DependencyMode"
            ))),
        }
    }
}

impl ModelValue for DependencyMode {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Dependency {
    dependent: String,
    required: String,
    mode: DependencyMode,
}

impl Dependency {
    pub fn new(
        dependent: impl Into<String>,
        required: impl Into<String>,
    ) -> Result<Self, ValueError> {
        Self::with_mode(dependent, required, DependencyMode::Revision)
    }

    pub fn with_mode(
        dependent: impl Into<String>,
        required: impl Into<String>,
        mode: DependencyMode,
    ) -> Result<Self, ValueError> {
        let dependent = dependent.into();
        let required = required.into();
        validate_identifier(&dependent, "dependent artifact")?;
        validate_identifier(&required, "required artifact")?;
        if dependent == required {
            return Err(ValueError::new("an artifact cannot depend on itself"));
        }
        Ok(Self {
            dependent,
            required,
            mode,
        })
    }

    pub fn dependent(&self) -> &str {
        &self.dependent
    }

    pub fn required(&self) -> &str {
        &self.required
    }

    pub const fn mode(&self) -> DependencyMode {
        self.mode
    }
}

impl ModelValue for Dependency {
    fn model_value(&self) -> Value {
        typed_record(
            "Dependency",
            [
                (
                    "dependent",
                    Value::Scalar(Scalar::text(self.dependent.as_str())),
                ),
                ("mode", self.mode.model_value()),
                (
                    "required",
                    Value::Scalar(Scalar::text(self.required.as_str())),
                ),
            ],
        )
    }
}

pub fn dependency_order(
    names: &[String],
    dependencies: &[Dependency],
) -> Result<Vec<String>, ValueError> {
    let mut pending: HashSet<String> = names.iter().cloned().collect();
    let mut pairs: HashSet<(&str, &str)> = HashSet::with_capacity(dependencies.len());
    for edge in dependencies {
        if !pairs.insert((edge.dependent(), edge.required())) {
            return Err(ValueError::new("duplicate dependency"));
        }
    }
    for edge in dependencies {
        if !pending.contains(edge.dependent()) || !pending.contains(edge.required()) {
            return Err(ValueError::new(
                "dependency references an unknown artifact or asset",
            ));
        }
    }
    let mut result = Vec::with_capacity(names.len());
    while !pending.is_empty() {
        let mut ready: Vec<String> = pending
            .iter()
            .filter(|name| {
                !dependencies.iter().any(|edge| {
                    edge.dependent() == name.as_str() && pending.contains(edge.required())
                })
            })
            .cloned()
            .collect();
        if ready.is_empty() {
            return Err(ValueError::new("cyclic artifact dependencies"));
        }
        ready.sort();
        for name in &ready {
            pending.remove(name);
        }
        result.append(&mut ready);
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedArtifact {
    id: String,
    family: Family,
    destination: PathBuf,
    content: Content,
    subtree: Option<String>,
    require_existing: bool,
    schema_requirements: Vec<DocumentSchema>,
}

impl RenderedArtifact {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        family: Family,
        destination: PathBuf,
        content: Content,
        subtree: Option<String>,
        require_existing: bool,
        schema_requirements: Vec<DocumentSchema>,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        validate_identifier(&id, "rendered artifact id")?;
        if !destination.is_absolute()
            || destination.parent().is_none()
            || destination
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(ValueError::new(
                "rendered destination must be an absolute Path",
            ));
        }
        let mut keys = HashSet::with_capacity(schema_requirements.len());
        if !schema_requirements
            .iter()
            .all(|schema| keys.insert(schema.key()))
        {
            return Err(ValueError::new("duplicate document schema requirement"));
        }
        if !schema_requirements.is_empty()
            && (!matches!(content, Content::Structured(_))
                || schema_requirements.iter().any(|schema| {
                    !matches!(
                        &content,
                        Content::Structured(structured)
                            if schema.format() == structured.format()
                    )
                }))
        {
            return Err(ValueError::new(
                "document schema requirements need matching structured content",
            ));
        }
        if let Some(selector) = &subtree {
            crate::validation::validate_relative_path(selector, "rendered subtree")?;
            if !matches!(content, Content::Tree(_)) {
                return Err(ValueError::new("subtree claims require tree content"));
            }
        }
        Ok(Self {
            id,
            family,
            destination,
            content,
            subtree,
            require_existing,
            schema_requirements,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn family(&self) -> Family {
        self.family
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    pub const fn content(&self) -> &Content {
        &self.content
    }

    pub fn subtree(&self) -> Option<&str> {
        self.subtree.as_deref()
    }

    pub const fn require_existing(&self) -> bool {
        self.require_existing
    }

    pub fn schema_requirements(&self) -> &[DocumentSchema] {
        &self.schema_requirements
    }
}

impl ModelValue for RenderedArtifact {
    fn model_value(&self) -> Value {
        typed_record(
            "RenderedArtifact",
            [
                ("content", self.content.model_value()),
                (
                    "destination",
                    Value::Scalar(Scalar::text(
                        crate::codec::posix_path(&self.destination).as_str(),
                    )),
                ),
                ("family", self.family.model_value()),
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
                (
                    "require_existing",
                    Value::Scalar(Scalar::boolean(self.require_existing)),
                ),
                (
                    "schema_requirements",
                    Value::Array(ArrayValue::new(
                        self.schema_requirements.iter().map(ModelValue::model_value),
                    )),
                ),
                (
                    "subtree",
                    self.subtree
                        .as_ref()
                        .map_or_else(|| Value::Scalar(Scalar::null()), ModelValue::model_value),
                ),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedBundle {
    artifacts: Vec<RenderedArtifact>,
    dependencies: Vec<Dependency>,
    notices: Vec<Notice>,
    content_digest: String,
    routing_context: Vec<(String, String)>,
}

impl RenderedBundle {
    pub fn new(
        artifacts: impl IntoIterator<Item = RenderedArtifact>,
        dependencies: impl IntoIterator<Item = Dependency>,
        routing_context: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ValueError> {
        Self::with_notices(artifacts, dependencies, std::iter::empty(), routing_context)
    }

    pub fn with_notices(
        artifacts: impl IntoIterator<Item = RenderedArtifact>,
        dependencies: impl IntoIterator<Item = Dependency>,
        notices: impl IntoIterator<Item = Notice>,
        routing_context: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ValueError> {
        let mut artifacts: Vec<_> = artifacts.into_iter().collect();
        let mut dependencies: Vec<_> = dependencies.into_iter().collect();
        let notices = notices.into_iter().collect();
        let mut routing_context: Vec<_> = routing_context.into_iter().collect();
        let names: Vec<_> = artifacts
            .iter()
            .map(|artifact| artifact.id().to_owned())
            .collect();
        if names.iter().collect::<HashSet<_>>().len() != names.len() {
            return Err(ValueError::new("rendered artifact ids must be unique"));
        }
        dependency_order(&names, &dependencies)?;
        let by_id: HashMap<_, _> = artifacts
            .iter()
            .map(|artifact| (artifact.id(), artifact))
            .collect();
        for edge in &dependencies {
            if edge.mode() == DependencyMode::StableReference {
                if !matches!(
                    by_id.get(edge.required()).map(|item| item.content()),
                    Some(Content::File(_))
                ) {
                    return Err(ValueError::new(
                        "stable references require a whole-file routing entry",
                    ));
                }
                if dependencies.iter().any(|child| {
                    child.dependent() == edge.required() && child.mode() != DependencyMode::Revision
                }) {
                    return Err(ValueError::new(
                        "stable routing entries require revision-bound dependencies",
                    ));
                }
            }
        }
        let mut context_keys = HashSet::with_capacity(routing_context.len());
        if !routing_context
            .iter()
            .all(|(key, _)| context_keys.insert(key.as_str()))
        {
            return Err(ValueError::new(
                "render routing context keys must be unique",
            ));
        }
        artifacts.sort_by(|left, right| left.id().cmp(right.id()));
        dependencies.sort_by(|left, right| {
            (left.dependent(), left.required()).cmp(&(right.dependent(), right.required()))
        });
        routing_context.sort();
        let content_digest = render_digest(&artifacts, &dependencies);
        Ok(Self {
            artifacts,
            dependencies,
            notices,
            content_digest,
            routing_context,
        })
    }

    pub fn empty() -> Self {
        Self {
            artifacts: Vec::new(),
            dependencies: Vec::new(),
            notices: Vec::new(),
            content_digest: render_digest(&[], &[]),
            routing_context: Vec::new(),
        }
    }

    pub fn artifacts(&self) -> &[RenderedArtifact] {
        &self.artifacts
    }

    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    pub fn supported(&self) -> bool {
        !self
            .notices
            .iter()
            .any(|notice| notice.kind() == NoticeKind::Unsupported)
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    pub fn routing_context(&self) -> &[(String, String)] {
        &self.routing_context
    }
}

fn render_digest(artifacts: &[RenderedArtifact], dependencies: &[Dependency]) -> String {
    let model = Value::Array(ArrayValue::new([
        Value::Array(ArrayValue::new(
            artifacts.iter().map(ModelValue::model_value),
        )),
        Value::Array(ArrayValue::new(
            dependencies.iter().map(ModelValue::model_value),
        )),
    ]));
    let mut digest = Sha256::new();
    digest.update(b"flyrail-render-v1\0");
    digest.update(model.semantic_bytes());
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]
mod tests {
    use super::{
        Dependency, DependencyMode, ModelValue, RenderedArtifact, RenderedBundle, dependency_order,
    };
    use crate::artifacts::Family;
    use crate::content::{Content, SectionContent};
    use crate::values::{Scalar, Value};
    use std::path::PathBuf;

    fn encoded(value: &Value) -> String {
        String::from_utf8(value.semantic_bytes()).unwrap()
    }

    #[test]
    fn encodes_scalars_objects_arrays_and_bytes_like_python_models() {
        let scalar = Value::Scalar(Scalar::text("check"));
        assert_eq!(
            encoded(&scalar.model_value()),
            r#"["object",[["fields",["object",[["value",["string","check"]]]]],["type",["string","Scalar"]]]]"#
        );
        let from_bytes = vec![0xCA_u8, 0xFE].model_value();
        assert_eq!(
            encoded(&from_bytes),
            r#"["object",[["bytes",["string","cafe"]]]]"#
        );
        let text = "hello".to_owned();
        assert_eq!(encoded(&text.model_value()), r#"["string","hello"]"#);
        assert_eq!(encoded(&true.model_value()), r#"["boolean",true]"#);
        assert_eq!(encoded(&7_i32.model_value()), r#"["integer","7"]"#);
        let nothing: Option<&Dependency> = None;
        assert_eq!(encoded(&nothing.model_value()), r#"["null"]"#);
        let empty_bytes: Vec<u8> = Vec::new();
        assert_eq!(
            encoded(&empty_bytes.model_value()),
            r#"["object",[["bytes",["string",""]]]]"#
        );
    }

    #[test]
    fn orders_dependencies_topologically_and_detects_cycles() {
        let names = vec!["hook".to_owned(), "support".to_owned()];
        let edges = [Dependency::new("hook", "support").unwrap()];
        assert_eq!(
            dependency_order(&names, &edges).unwrap(),
            ["support".to_owned(), "hook".to_owned()]
        );
        let empty: Vec<Dependency> = Vec::new();
        assert_eq!(
            dependency_order(&names, &empty).unwrap(),
            ["hook".to_owned(), "support".to_owned()]
        );
    }

    #[test]
    fn rejects_duplicate_unknown_and_cyclic_dependencies() {
        let names = vec!["guide".to_owned()];
        let duplicated = vec![
            Dependency::new("guide", "guide-a").unwrap(),
            Dependency::new("guide", "guide-a").unwrap(),
        ];
        assert_eq!(
            dependency_order(&names, &duplicated).unwrap_err().message(),
            "duplicate dependency"
        );
        let unknown = vec![Dependency::new("guide", "missing").unwrap()];
        assert_eq!(
            dependency_order(&names, &unknown).unwrap_err().message(),
            "dependency references an unknown artifact or asset"
        );
        let cyclic = vec![
            Dependency::new("guide", "helper").unwrap(),
            Dependency::new("helper", "guide").unwrap(),
        ];
        let both = vec!["guide".to_owned(), "helper".to_owned()];
        assert_eq!(
            dependency_order(&both, &cyclic).unwrap_err().message(),
            "cyclic artifact dependencies"
        );
    }

    #[test]
    fn dependencies_validate_their_members_and_modes() {
        let edge = Dependency::new("guide", "helper").unwrap();
        assert_eq!(edge.mode(), DependencyMode::Revision);
        assert_eq!(
            Dependency::with_mode("guide", "helper", DependencyMode::StableReference)
                .unwrap()
                .mode(),
            DependencyMode::StableReference
        );
        assert_eq!(
            Dependency::new("guide", "guide").unwrap_err().message(),
            "an artifact cannot depend on itself"
        );
        assert!(Dependency::new("Bad", "guide").is_err());
        assert_eq!(
            encoded(&edge.model_value()),
            r#"["object",[["fields",["object",[["dependent",["string","guide"]],["mode",["string","revision"]],["required",["string","helper"]]]]],["type",["string","Dependency"]]]]"#
        );
    }

    #[test]
    fn rendered_instruction_digest_matches_the_shared_fixture() {
        let artifact = RenderedArtifact::new(
            "guide",
            Family::Instructions,
            PathBuf::from("/fixture/project/AGENTS.md"),
            Content::Section(SectionContent::new("guide", "Hello", None).unwrap()),
            None,
            false,
            Vec::new(),
        )
        .unwrap();
        let rendered = RenderedBundle::new([artifact], [], []).unwrap();
        assert_eq!(
            rendered.content_digest(),
            "249a091ac4a021ebbd33997ba8e3b49d34765516a1d2fdfab2f28f195329838b"
        );
    }
}
