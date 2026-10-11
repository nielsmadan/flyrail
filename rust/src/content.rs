use crate::models::BundleEntry;
use crate::rendered::{ModelValue, typed_record};
use crate::validation::{ancestors, is_python_blank, portable_path_key, validate_identifier};
use crate::values::{ArrayValue, Scalar, Value, ValueError};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DocumentFormat {
    Json,
    Jsonc,
    Toml,
}

impl DocumentFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Toml => "toml",
        }
    }
}

impl FromStr for DocumentFormat {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "json" => Ok(Self::Json),
            "jsonc" => Ok(Self::Jsonc),
            "toml" => Ok(Self::Toml),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid DocumentFormat"
            ))),
        }
    }
}

impl ModelValue for DocumentFormat {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FileMode {
    Private,
    Readable,
    Executable,
}

impl FileMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Readable => "readable",
            Self::Executable => "executable",
        }
    }
}

impl FromStr for FileMode {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "private" => Ok(Self::Private),
            "readable" => Ok(Self::Readable),
            "executable" => Ok(Self::Executable),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid FileMode"
            ))),
        }
    }
}

impl ModelValue for FileMode {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Key {
    name: String,
}

impl Key {
    pub fn new(name: impl Into<String>) -> Result<Self, ValueError> {
        let name = name.into();
        if name.is_empty() {
            return Err(ValueError::new("selector keys must be nonempty"));
        }
        Ok(Self { name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl ModelValue for Key {
    fn model_value(&self) -> Value {
        typed_record(
            "Key",
            [("name", Value::Scalar(Scalar::text(self.name.as_str())))],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Member {
    identity: Value,
    key: Vec<String>,
}

impl Member {
    pub fn new(
        identity: impl Into<Value>,
        key: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ValueError> {
        let identity = identity.into();
        let key: Vec<String> = key.into_iter().map(Into::into).collect();
        for name in &key {
            Key::new(name.as_str())?;
        }
        if !key.is_empty() && !matches!(identity, Value::Scalar(_)) {
            return Err(ValueError::new("native member identity must be a scalar"));
        }
        Ok(Self { identity, key })
    }

    pub const fn identity(&self) -> &Value {
        &self.identity
    }

    pub fn key(&self) -> &[String] {
        &self.key
    }
}

impl ModelValue for Member {
    fn model_value(&self) -> Value {
        typed_record(
            "Member",
            [
                ("identity", self.identity.model_value()),
                (
                    "key",
                    Value::Array(ArrayValue::new(
                        self.key
                            .iter()
                            .map(|name| Value::Scalar(Scalar::text(name.as_str()))),
                    )),
                ),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SelectorPart {
    Key(Key),
    Member(Member),
}

impl ModelValue for SelectorPart {
    fn model_value(&self) -> Value {
        match self {
            Self::Key(key) => key.model_value(),
            Self::Member(member) => member.model_value(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Selector {
    parts: Vec<SelectorPart>,
}

impl Selector {
    pub fn new(parts: impl IntoIterator<Item = SelectorPart>) -> Result<Self, ValueError> {
        let parts: Vec<SelectorPart> = parts.into_iter().collect();
        if parts.is_empty() {
            return Err(ValueError::new("a selector must select a key or member"));
        }
        Ok(Self { parts })
    }

    pub fn parts(&self) -> &[SelectorPart] {
        &self.parts
    }
}

impl ModelValue for Selector {
    fn model_value(&self) -> Value {
        typed_record(
            "Selector",
            [(
                "parts",
                Value::Array(ArrayValue::new(
                    self.parts.iter().map(ModelValue::model_value),
                )),
            )],
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Position {
    First,
    Last,
    Before,
    After,
}

impl Position {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Last => "last",
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

impl FromStr for Position {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "first" => Ok(Self::First),
            "last" => Ok(Self::Last),
            "before" => Ok(Self::Before),
            "after" => Ok(Self::After),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid Position"
            ))),
        }
    }
}

impl ModelValue for Position {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Placement {
    position: Position,
    anchor: Option<Member>,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            position: Position::Last,
            anchor: None,
        }
    }
}

impl Placement {
    pub fn new(position: Position, anchor: Option<Member>) -> Result<Self, ValueError> {
        let anchored = matches!(position, Position::Before | Position::After);
        if anchored != anchor.is_some() {
            return Err(ValueError::new(
                "before/after placement requires exactly one member anchor",
            ));
        }
        Ok(Self { position, anchor })
    }

    pub const fn position(&self) -> Position {
        self.position
    }

    pub const fn anchor(&self) -> Option<&Member> {
        self.anchor.as_ref()
    }
}

impl ModelValue for Placement {
    fn model_value(&self) -> Value {
        typed_record(
            "Placement",
            [
                ("anchor", self.anchor.as_ref().model_value()),
                ("position", self.position.model_value()),
            ],
        )
    }
}

pub fn snapshot_entries(
    entries: impl IntoIterator<Item = BundleEntry>,
) -> Result<Vec<BundleEntry>, ValueError> {
    let mut selected: Vec<BundleEntry> = Vec::new();
    let mut spellings: HashMap<String, String> = HashMap::new();
    for entry in entries {
        if selected
            .iter()
            .any(|existing| existing.path() == entry.path())
        {
            return Err(ValueError::new("duplicate entry path"));
        }
        for path in ancestors(entry.path()) {
            let key = portable_path_key(&path);
            if spellings
                .get(&key)
                .is_some_and(|previous| *previous != path)
            {
                return Err(ValueError::new("portable entry path collision"));
            }
            spellings.insert(key, path);
        }
        selected.push(entry);
    }
    let snapshot = selected.clone();
    for entry in snapshot {
        for path in ancestors(entry.path()) {
            if path == entry.path() {
                continue;
            }
            if let Some(existing) = selected.iter().find(|candidate| candidate.path() == path) {
                if !existing.is_directory() {
                    return Err(ValueError::new("entry file/directory collision"));
                }
            } else {
                selected.push(BundleEntry::directory(path)?);
            }
        }
    }
    selected.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));
    Ok(selected)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileContent {
    data: Vec<u8>,
    mode: FileMode,
}

impl FileContent {
    pub fn new(data: impl Into<Vec<u8>>, mode: FileMode) -> Self {
        Self {
            data: data.into(),
            mode,
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub const fn mode(&self) -> FileMode {
        self.mode
    }
}

impl ModelValue for FileContent {
    fn model_value(&self) -> Value {
        typed_record(
            "FileContent",
            [
                ("data", self.data.model_value()),
                ("mode", self.mode.model_value()),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TreeContent {
    entries: Vec<BundleEntry>,
}

impl TreeContent {
    pub fn new(entries: impl IntoIterator<Item = BundleEntry>) -> Result<Self, ValueError> {
        Ok(Self {
            entries: snapshot_entries(entries)?,
        })
    }

    pub fn entries(&self) -> &[BundleEntry] {
        &self.entries
    }
}

impl ModelValue for TreeContent {
    fn model_value(&self) -> Value {
        typed_record(
            "TreeContent",
            [(
                "entries",
                Value::Array(ArrayValue::new(
                    self.entries.iter().map(ModelValue::model_value),
                )),
            )],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SectionBoundaries {
    start: String,
    end: String,
}

impl SectionBoundaries {
    pub fn new(start: impl Into<String>, end: impl Into<String>) -> Result<Self, ValueError> {
        let start = start.into();
        let end = end.into();
        for line in [&start, &end] {
            if is_python_blank(line) || line.contains(['\r', '\n', '\0']) {
                return Err(ValueError::new(
                    "section boundaries must be nonblank single lines without NUL",
                ));
            }
        }
        if start == end {
            return Err(ValueError::new("section boundaries must be distinct"));
        }
        Ok(Self { start, end })
    }

    pub fn from_marker(marker: &str) -> Result<Self, ValueError> {
        validate_identifier(marker, "section marker")?;
        Self::new(
            format!("<!-- flyrail:{marker}:start -->"),
            format!("<!-- flyrail:{marker}:end -->"),
        )
    }

    pub fn start(&self) -> &str {
        &self.start
    }

    pub fn end(&self) -> &str {
        &self.end
    }
}

impl ModelValue for SectionBoundaries {
    fn model_value(&self) -> Value {
        typed_record(
            "SectionBoundaries",
            [
                ("end", Value::Scalar(Scalar::text(self.end.as_str()))),
                ("start", Value::Scalar(Scalar::text(self.start.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SectionContent {
    marker: String,
    text: String,
    boundaries: Option<SectionBoundaries>,
}

impl SectionContent {
    pub fn new(
        marker: impl Into<String>,
        text: impl Into<String>,
        boundaries: Option<SectionBoundaries>,
    ) -> Result<Self, ValueError> {
        let marker = marker.into();
        let text = text.into();
        validate_identifier(&marker, "section marker")?;
        if text.contains('\0') || text.contains("<!-- flyrail:") {
            return Err(ValueError::new(
                "section text cannot contain NUL or Flyrail markers",
            ));
        }
        if let Some(boundaries) = &boundaries
            && python_lines(&text)
                .any(|line| line == boundaries.start() || line == boundaries.end())
        {
            return Err(ValueError::new(
                "section text cannot contain its boundary lines",
            ));
        }
        Ok(Self {
            marker,
            text,
            boundaries,
        })
    }

    pub fn marker(&self) -> &str {
        &self.marker
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub const fn boundaries(&self) -> Option<&SectionBoundaries> {
        self.boundaries.as_ref()
    }

    pub fn selector(&self) -> Result<SectionBoundaries, ValueError> {
        self.boundaries.as_ref().map_or_else(
            || SectionBoundaries::from_marker(&self.marker),
            |boundaries| Ok(boundaries.clone()),
        )
    }
}

impl ModelValue for SectionContent {
    fn model_value(&self) -> Value {
        typed_record(
            "SectionContent",
            [
                ("boundaries", self.boundaries.as_ref().model_value()),
                ("marker", Value::Scalar(Scalar::text(self.marker.as_str()))),
                ("text", Value::Scalar(Scalar::text(self.text.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StructuredContent {
    format: DocumentFormat,
    selector: Selector,
    value: Value,
    placement: Option<Placement>,
}

impl StructuredContent {
    pub fn new(
        format: DocumentFormat,
        selector: Selector,
        value: Value,
        placement: Option<Placement>,
    ) -> Result<Self, ValueError> {
        if let Some(placement) = &placement {
            let Some(last) = selector.parts().last() else {
                return Err(ValueError::new("a selector must select a key or member"));
            };
            if !matches!(last, SelectorPart::Member(_)) {
                return Err(ValueError::new(
                    "placement is only valid for an array member",
                ));
            }
            if let SelectorPart::Member(member) = last
                && placement.anchor() == Some(member)
            {
                return Err(ValueError::new(
                    "a member cannot be its own insertion anchor",
                ));
            }
        }
        Ok(Self {
            format,
            selector,
            value,
            placement,
        })
    }

    pub const fn format(&self) -> DocumentFormat {
        self.format
    }

    pub const fn selector(&self) -> &Selector {
        &self.selector
    }

    pub const fn value(&self) -> &Value {
        &self.value
    }

    pub const fn placement(&self) -> Option<&Placement> {
        self.placement.as_ref()
    }
}

impl ModelValue for StructuredContent {
    fn model_value(&self) -> Value {
        typed_record(
            "StructuredContent",
            [
                ("format", self.format.model_value()),
                ("placement", self.placement.as_ref().model_value()),
                ("selector", self.selector.model_value()),
                ("value", self.value.model_value()),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DocumentSchema {
    format: DocumentFormat,
    key: String,
    value: Scalar,
}

impl DocumentSchema {
    pub fn new(
        format: DocumentFormat,
        key: impl Into<String>,
        value: Scalar,
    ) -> Result<Self, ValueError> {
        let key = key.into();
        if !matches!(format, DocumentFormat::Json | DocumentFormat::Jsonc) {
            return Err(ValueError::new(
                "document schema scaffolds require JSON or JSONC",
            ));
        }
        Key::new(key.as_str())?;
        Ok(Self { format, key, value })
    }

    pub const fn format(&self) -> DocumentFormat {
        self.format
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub const fn value(&self) -> &Scalar {
        &self.value
    }

    pub fn content(&self) -> Result<StructuredContent, ValueError> {
        StructuredContent::new(
            self.format,
            Selector::new([SelectorPart::Key(Key::new(self.key.as_str())?)])?,
            Value::Scalar(self.value.clone()),
            None,
        )
    }
}

impl ModelValue for DocumentSchema {
    fn model_value(&self) -> Value {
        typed_record(
            "DocumentSchema",
            [
                ("format", self.format.model_value()),
                ("key", Value::Scalar(Scalar::text(self.key.as_str()))),
                ("value", self.value.model_value()),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Content {
    File(FileContent),
    Tree(TreeContent),
    Section(SectionContent),
    Structured(StructuredContent),
}

impl ModelValue for Content {
    fn model_value(&self) -> Value {
        match self {
            Self::File(file) => file.model_value(),
            Self::Tree(tree) => tree.model_value(),
            Self::Section(section) => section.model_value(),
            Self::Structured(structured) => structured.model_value(),
        }
    }
}

pub(crate) const fn python_lines(text: &str) -> PythonLines<'_> {
    PythonLines { rest: text }
}

pub(crate) struct PythonLines<'a> {
    rest: &'a str,
}

impl<'a> Iterator for PythonLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let rest = self.rest;
        for (index, character) in rest.char_indices() {
            if !is_python_line_break(character) {
                continue;
            }
            let (line, after) = rest.split_at_checked(index)?;
            let width = if character == '\r'
                && after.as_bytes().get(1).is_some_and(|byte| *byte == b'\n')
            {
                2
            } else {
                character.len_utf8()
            };
            self.rest = after.split_at_checked(width).map(|(_, tail)| tail)?;
            return Some(line);
        }
        self.rest = "";
        Some(rest)
    }
}

const fn is_python_line_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
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
        Content, DocumentFormat, DocumentSchema, FileContent, FileMode, Key, Member, Placement,
        Position, PythonLines, SectionBoundaries, SectionContent, Selector, SelectorPart,
        StructuredContent, TreeContent, python_lines, snapshot_entries,
    };
    use crate::models::BundleEntry;
    use crate::temporal::Date;
    use crate::values::Scalar;

    fn paths(entries: &[BundleEntry]) -> Vec<&str> {
        entries.iter().map(BundleEntry::path).collect()
    }

    #[test]
    fn tree_snapshots_infer_parents_and_preserve_empty_directories() {
        let mut entries = vec![
            BundleEntry::executable_file("scripts/run", b"run".to_vec()).unwrap(),
            BundleEntry::directory("empty").unwrap(),
        ];
        let tree = TreeContent::new(entries.clone()).unwrap();
        entries.clear();
        assert_eq!(paths(tree.entries()), ["empty", "scripts", "scripts/run"]);
        assert!(tree.entries()[1].is_directory());
        assert!(tree.entries()[2].executable());
        assert_eq!(tree.entries()[2].data(), Some(b"run".as_slice()));
    }

    #[test]
    fn trees_reject_duplicate_colliding_and_shadowed_entries() {
        let duplicate = snapshot_entries([
            BundleEntry::directory("a").unwrap(),
            BundleEntry::directory("a").unwrap(),
        ]);
        assert_eq!(duplicate.unwrap_err().message(), "duplicate entry path");
        let colliding = snapshot_entries([
            BundleEntry::file("a/x", b"x".to_vec()).unwrap(),
            BundleEntry::file("A/y", b"y".to_vec()).unwrap(),
        ]);
        assert_eq!(
            colliding.unwrap_err().message(),
            "portable entry path collision"
        );
        let shadowed = snapshot_entries([
            BundleEntry::file("a", b"file".to_vec()).unwrap(),
            BundleEntry::file("a/b", b"child".to_vec()).unwrap(),
        ]);
        assert_eq!(
            shadowed.unwrap_err().message(),
            "entry file/directory collision"
        );
        assert!(TreeContent::new([]).is_ok());
    }

    #[test]
    fn sections_derive_selectors_from_markers_or_custom_boundaries() {
        let default = SectionContent::new("guide", "text", None).unwrap();
        assert_eq!(
            default.selector().unwrap(),
            SectionBoundaries::new("<!-- flyrail:guide:start -->", "<!-- flyrail:guide:end -->")
                .unwrap()
        );
        let custom =
            SectionBoundaries::new("<!-- >>> owned >>> -->", "<!-- <<< owned <<< -->").unwrap();
        let section = SectionContent::new("first", "text", Some(custom.clone())).unwrap();
        let other = SectionContent::new("second", "text", Some(custom.clone())).unwrap();
        assert_eq!(section.selector().unwrap(), other.selector().unwrap());
        assert_eq!(section.boundaries(), Some(&custom));
    }

    #[test]
    fn sections_reject_blank_identical_and_leaky_boundaries() {
        assert_eq!(
            SectionBoundaries::new("same", "same")
                .unwrap_err()
                .message(),
            "section boundaries must be distinct"
        );
        for (start, end) in [("", "end"), ("\n", "end"), ("a\0b", "end"), ("a", "b\r")] {
            assert_eq!(
                SectionBoundaries::new(start, end).unwrap_err().message(),
                "section boundaries must be nonblank single lines without NUL"
            );
        }
        assert!(SectionBoundaries::new(" start ", "end").is_ok());
    }

    #[test]
    fn sections_reject_nul_marker_lines_and_contained_boundaries() {
        assert_eq!(
            SectionContent::new("x", "text\0", None)
                .unwrap_err()
                .message(),
            "section text cannot contain NUL or Flyrail markers"
        );
        assert_eq!(
            SectionContent::new("x", "<!-- flyrail:x:start -->", None)
                .unwrap_err()
                .message(),
            "section text cannot contain NUL or Flyrail markers"
        );
        let boundaries = SectionBoundaries::new("start", "end").unwrap();
        assert_eq!(
            SectionContent::new("x", "start\n", Some(boundaries.clone()))
                .unwrap_err()
                .message(),
            "section text cannot contain its boundary lines"
        );
        assert!(SectionContent::new("x", "start here\nend here", Some(boundaries)).is_ok());
        assert_eq!(
            SectionContent::new("Bad", "text", None)
                .unwrap_err()
                .message(),
            "section marker must be 1-64 lowercase ASCII letters/digits with internal hyphens"
        );
    }

    #[test]
    fn sections_split_text_like_python_splitlines() {
        assert_eq!(
            python_lines("a\r\nb\rc\u{b}d\u{c}e\u{1c}g\u{1d}h\u{1e}i\u{85}j\u{2028}k\u{2029}l")
                .collect::<Vec<_>>(),
            ["a", "b", "c", "d", "e", "g", "h", "i", "j", "k", "l"]
        );
        assert_eq!(python_lines("").collect::<Vec<_>>(), Vec::<&str>::new());
        assert_eq!(python_lines("plain").collect::<Vec<_>>(), ["plain"]);
        assert_eq!(python_lines("a\r\n").collect::<Vec<_>>(), ["a"]);
        let mut lines = PythonLines { rest: "x\u{2028}y" };
        assert_eq!(lines.next(), Some("x"));
        assert_eq!(lines.next(), Some("y"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn selectors_accept_keys_and_identified_members() {
        let selector = Selector::new([
            SelectorPart::Key(Key::new("hooks").unwrap()),
            SelectorPart::Member(Member::new(Scalar::text("review"), ["name"]).unwrap()),
        ])
        .unwrap();
        assert_eq!(selector.parts().len(), 2);
        assert_eq!(
            selector.parts()[1].clone(),
            SelectorPart::Member(Member::new(Scalar::text("review"), ["name"]).unwrap(),)
        );
        assert_eq!(
            Selector::new([]).unwrap_err().message(),
            "a selector must select a key or member"
        );
        assert_eq!(
            Key::new("").unwrap_err().message(),
            "selector keys must be nonempty"
        );
        let member = Member::new(Scalar::integer(1), Vec::<String>::new()).unwrap();
        assert_eq!(member.key(), [] as [String; 0]);
        let object = crate::values::ObjectValue::new([("a", Scalar::integer(1).into())]).unwrap();
        assert_eq!(
            Member::new(object.clone(), ["name"]).unwrap_err().message(),
            "native member identity must be a scalar"
        );
        assert!(Member::new(object, Vec::<String>::new()).is_ok());
    }

    #[test]
    fn placement_anchors_only_before_and_after_positions() {
        let member = || Member::new(Scalar::text("anchor"), Vec::<String>::new()).unwrap();
        assert!(Placement::new(Position::First, None).is_ok());
        assert!(Placement::new(Position::Last, None).is_ok());
        assert_eq!(Placement::default().position(), Position::Last);
        assert!(Placement::new(Position::Before, Some(member())).is_ok());
        assert!(Placement::new(Position::After, Some(member())).is_ok());
        for position in [Position::First, Position::Last] {
            assert_eq!(
                Placement::new(position, Some(member()))
                    .unwrap_err()
                    .message(),
                "before/after placement requires exactly one member anchor"
            );
        }
        for position in [Position::Before, Position::After] {
            assert_eq!(
                Placement::new(position, None).unwrap_err().message(),
                "before/after placement requires exactly one member anchor"
            );
        }
    }

    #[test]
    fn structured_content_places_only_array_members() {
        let later = Member::new(Scalar::text("later"), ["name"]).unwrap();
        let selector = Selector::new([
            SelectorPart::Key(Key::new("hooks").unwrap()),
            SelectorPart::Member(Member::new(Scalar::text("review"), ["name"]).unwrap()),
        ])
        .unwrap();
        let value = crate::values::ObjectValue::new([
            ("name", Scalar::text("review").into()),
            ("command", Scalar::text("check").into()),
        ])
        .unwrap()
        .into();
        let content = StructuredContent::new(
            DocumentFormat::Jsonc,
            selector,
            value,
            Some(Placement::new(Position::Before, Some(later)).unwrap()),
        )
        .unwrap();
        assert_eq!(content.format(), DocumentFormat::Jsonc);
        assert_eq!(
            content.placement().map(Placement::position),
            Some(Position::Before)
        );

        let key_selector = Selector::new([SelectorPart::Key(Key::new("hooks").unwrap())]).unwrap();
        assert_eq!(
            StructuredContent::new(
                DocumentFormat::Json,
                key_selector,
                Scalar::integer(1).into(),
                Some(Placement::default())
            )
            .unwrap_err()
            .message(),
            "placement is only valid for an array member"
        );

        let member = Member::new(Scalar::text("review"), ["name"]).unwrap();
        let member_selector = Selector::new([
            SelectorPart::Key(Key::new("hooks").unwrap()),
            SelectorPart::Member(member.clone()),
        ])
        .unwrap();
        assert_eq!(
            StructuredContent::new(
                DocumentFormat::Json,
                member_selector,
                Scalar::integer(1).into(),
                Some(Placement::new(Position::Before, Some(member)).unwrap())
            )
            .unwrap_err()
            .message(),
            "a member cannot be its own insertion anchor"
        );
        assert!(
            StructuredContent::new(
                DocumentFormat::Json,
                Selector::new([SelectorPart::Key(Key::new("hooks").unwrap())]).unwrap(),
                Scalar::date(Date::new(2026, 1, 2).unwrap()).into(),
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn document_schemas_scaffold_structured_content() {
        let schema =
            DocumentSchema::new(DocumentFormat::Jsonc, "version", Scalar::integer(1)).unwrap();
        let content = schema.content().unwrap();
        assert_eq!(content.format(), DocumentFormat::Jsonc);
        assert_eq!(
            content.selector(),
            &Selector::new([SelectorPart::Key(Key::new("version").unwrap())]).unwrap()
        );
        assert_eq!(content.value(), &Scalar::integer(1).into());
        assert_eq!(
            DocumentSchema::new(DocumentFormat::Toml, "version", Scalar::integer(1))
                .unwrap_err()
                .message(),
            "document schema scaffolds require JSON or JSONC"
        );
        assert!(DocumentSchema::new(DocumentFormat::Json, "", Scalar::integer(1)).is_err());
    }

    #[test]
    fn file_content_carries_bytes_and_mode() {
        let file = FileContent::new(b"hi".to_vec(), FileMode::Readable);
        assert_eq!(file.data(), b"hi".as_slice());
        assert_eq!(file.mode(), FileMode::Readable);
        let content = Content::File(file);
        assert!(matches!(content, Content::File(_)));
    }
}
