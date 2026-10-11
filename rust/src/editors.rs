use crate::authority::{Claim, ClaimSelector, Ownership, sha256_hex};
use crate::content::{
    Content, DocumentFormat, Member, Placement, Position, SectionBoundaries, SectionContent,
    Selector, SelectorPart, StructuredContent,
};
use crate::json_document::{JsonDocument, check_identity_frames, check_snapshot_frames};
use crate::toml_layout::{LayoutPart, header_path, layout_headers};
use crate::values::{ArrayValue, ObjectValue, Scalar, Value, ValueError};
use sha2::{Digest as _, Sha256};
use std::str::FromStr;

/// Content accepted by document editing requests.
pub type SharedContent = Content;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Disposition {
    Created,
    TakenOver,
    Adopted,
}

impl Disposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::TakenOver => "taken-over",
            Self::Adopted => "adopted",
        }
    }
}

impl FromStr for Disposition {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "created" => Ok(Self::Created),
            "taken-over" => Ok(Self::TakenOver),
            "adopted" => Ok(Self::Adopted),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid Disposition"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Acquisition {
    Conflict,
    Takeover,
    Adopt,
}

impl Acquisition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::Takeover => "takeover",
            Self::Adopt => "adopt",
        }
    }
}

impl FromStr for Acquisition {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "conflict" => Ok(Self::Conflict),
            "takeover" => Ok(Self::Takeover),
            "adopt" => Ok(Self::Adopt),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid Acquisition"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EditStatus {
    Created,
    TakenOver,
    Adopted,
    Current,
    Updated,
    Removed,
    Modified,
    Conflict,
}

impl EditStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::TakenOver => "taken-over",
            Self::Adopted => "adopted",
            Self::Current => "current",
            Self::Updated => "updated",
            Self::Removed => "removed",
            Self::Modified => "modified",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapshotValue {
    Bytes(Vec<u8>),
    Typed(Value),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrayPosition {
    previous: Option<Member>,
    following: Option<Member>,
    prefix: Vec<u8>,
}

impl ArrayPosition {
    pub fn new(
        previous: Option<Member>,
        following: Option<Member>,
        prefix: impl Into<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        if previous.is_some() && previous == following {
            return Err(ValueError::new("array position anchors must be distinct"));
        }
        Ok(Self {
            previous,
            following,
            prefix: prefix.into(),
        })
    }

    pub const fn previous(&self) -> Option<&Member> {
        self.previous.as_ref()
    }

    pub const fn following(&self) -> Option<&Member> {
        self.following.as_ref()
    }

    pub fn prefix(&self) -> &[u8] {
        &self.prefix
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionSnapshot {
    value: SnapshotValue,
    syntax: Vec<u8>,
    owned_syntax: Vec<u8>,
    position: Option<ArrayPosition>,
    layout: Vec<u8>,
}

impl SelectionSnapshot {
    pub fn new(
        value: SnapshotValue,
        syntax: impl Into<Vec<u8>>,
        owned_syntax: impl Into<Vec<u8>>,
        position: Option<ArrayPosition>,
        layout: impl Into<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        let snapshot = Self {
            value,
            syntax: syntax.into(),
            owned_syntax: owned_syntax.into(),
            position,
            layout: layout.into(),
        };
        snapshot.validate_layouts()?;
        Ok(snapshot)
    }

    fn validate_layouts(&self) -> Result<(), ValueError> {
        for (relative, _) in layout_headers(&self.layout)? {
            let mut current = match &self.value {
                SnapshotValue::Typed(value) => Some(value),
                SnapshotValue::Bytes(_) => None,
            };
            for part in &relative {
                current = match (part, current) {
                    (LayoutPart::Field(name), Some(Value::Object(object))) => object.get(name),
                    (LayoutPart::Index(index), Some(Value::Array(array))) => {
                        match usize::try_from(*index) {
                            Ok(index) if index < array.items().len() => array.items().get(index),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if current.is_none() {
                    return Err(ValueError::new("invalid TOML restoration path"));
                }
            }
            if !matches!(current, Some(Value::Object(_))) {
                return Err(ValueError::new(
                    "TOML restoration header requires an object",
                ));
            }
        }
        Ok(())
    }

    pub const fn value(&self) -> &SnapshotValue {
        &self.value
    }

    pub fn syntax(&self) -> &[u8] {
        &self.syntax
    }

    pub fn owned_syntax(&self) -> &[u8] {
        &self.owned_syntax
    }

    pub const fn position(&self) -> Option<&ArrayPosition> {
        self.position.as_ref()
    }

    pub fn layout(&self) -> &[u8] {
        &self.layout
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedSelection {
    claim: Claim,
    format: Option<DocumentFormat>,
    installed: SelectionSnapshot,
    disposition: Disposition,
    baseline: Option<SelectionSnapshot>,
    separator: Vec<u8>,
}

impl OwnedSelection {
    pub fn new(
        claim: Claim,
        format: Option<DocumentFormat>,
        installed: SelectionSnapshot,
        disposition: Disposition,
        baseline: Option<SelectionSnapshot>,
        separator: impl Into<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        validate_claim(&claim, format)?;
        if (disposition == Disposition::TakenOver) != baseline.is_some() {
            return Err(ValueError::new("only takeover ownership has a baseline"));
        }
        let separator = separator.into();
        for snapshot in std::iter::once(&installed).chain(baseline.iter()) {
            if matches!(snapshot.value(), SnapshotValue::Bytes(_)) != format.is_none() {
                return Err(ValueError::new(
                    "selection snapshot and document format disagree",
                ));
            }
            if !snapshot.layout().is_empty() {
                if format != Some(DocumentFormat::Toml)
                    || !matches!(claim.selector(), ClaimSelector::Structured(_))
                {
                    return Err(ValueError::new("restoration layout requires a TOML claim"));
                }
                let path = selector_key_path(&claim);
                for (relative, name) in layout_headers(snapshot.layout())? {
                    let mut expected = path.clone();
                    expected.extend(relative.iter().filter_map(|part| match part {
                        LayoutPart::Field(name) => Some(name.clone()),
                        LayoutPart::Index(_) => None,
                    }));
                    if header_path(&name)? != expected {
                        return Err(ValueError::new(
                            "TOML restoration header does not match its destination",
                        ));
                    }
                }
            }
        }
        if format.is_some() && !separator.is_empty() {
            return Err(ValueError::new("section separators require a text claim"));
        }
        Ok(Self {
            claim,
            format,
            installed,
            disposition,
            baseline,
            separator,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn with_installed(&self, installed: SelectionSnapshot) -> Result<Self, ValueError> {
        Self::new(
            self.claim.clone(),
            self.format,
            installed,
            self.disposition,
            self.baseline.clone(),
            self.separator.clone(),
        )
    }

    pub const fn claim(&self) -> &Claim {
        &self.claim
    }

    pub const fn format(&self) -> Option<DocumentFormat> {
        self.format
    }

    pub const fn installed(&self) -> &SelectionSnapshot {
        &self.installed
    }

    pub const fn disposition(&self) -> Disposition {
        self.disposition
    }

    pub const fn baseline(&self) -> Option<&SelectionSnapshot> {
        self.baseline.as_ref()
    }

    pub fn separator(&self) -> &[u8] {
        &self.separator
    }
}

fn selector_key_path(claim: &Claim) -> Vec<String> {
    match claim.selector() {
        ClaimSelector::Structured(selector) => selector
            .parts()
            .iter()
            .filter_map(|part| match part {
                SelectorPart::Key(key) => Some(key.name().to_owned()),
                SelectorPart::Member(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn validate_claim(claim: &Claim, format: Option<DocumentFormat>) -> Result<(), ValueError> {
    match (claim.kind(), format) {
        (Ownership::Section, None) | (Ownership::Structured, Some(_)) => Ok(()),
        _ => Err(ValueError::new("shared claim and document format disagree")),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedContainer {
    selector: Selector,
    empty_syntax: Vec<u8>,
}

impl CreatedContainer {
    pub fn new(selector: Selector, empty_syntax: impl Into<Vec<u8>>) -> Self {
        Self {
            selector,
            empty_syntax: empty_syntax.into(),
        }
    }

    pub const fn selector(&self) -> &Selector {
        &self.selector
    }

    pub fn empty_syntax(&self) -> &[u8] {
        &self.empty_syntax
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentProvenance {
    empty_document: Option<Vec<u8>>,
    containers: Vec<CreatedContainer>,
    trailing_separators: Vec<String>,
    schema_fields: Vec<OwnedSelection>,
    schema_empty_document: Option<Vec<u8>>,
}

impl DocumentProvenance {
    pub fn new(
        empty_document: Option<Vec<u8>>,
        containers: Vec<CreatedContainer>,
        trailing_separators: Vec<String>,
        schema_fields: Vec<OwnedSelection>,
        schema_empty_document: Option<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        for item in &schema_fields {
            if item.disposition() != Disposition::Created
                || !matches!(
                    item.claim().selector(),
                    ClaimSelector::Structured(selector)
                        if selector.parts().len() == 1
                            && matches!(selector.parts().first(), Some(SelectorPart::Key(_)))
                )
                || !matches!(
                    item.format(),
                    Some(DocumentFormat::Json | DocumentFormat::Jsonc)
                )
                || !matches!(
                    item.installed().value(),
                    SnapshotValue::Typed(Value::Scalar(_))
                )
            {
                return Err(ValueError::new(
                    "schema provenance requires created top-level scalar fields",
                ));
            }
        }
        for (index, item) in schema_fields.iter().enumerate() {
            if schema_fields
                .iter()
                .take(index)
                .any(|other| other.claim() == item.claim())
            {
                return Err(ValueError::new("duplicate schema provenance"));
            }
        }
        if let Some(bytes) = &schema_empty_document
            && !bytes
                .iter()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        {
            return Err(ValueError::new(
                "schema empty-document evidence must be whitespace bytes",
            ));
        }
        for (index, container) in containers.iter().enumerate() {
            if containers
                .iter()
                .take(index)
                .any(|other| other.selector() == container.selector())
            {
                return Err(ValueError::new("duplicate container provenance"));
            }
        }
        for digest in &trailing_separators {
            if !is_sha256_text(digest) {
                return Err(ValueError::new(
                    "separator evidence must be SHA-256 digests",
                ));
            }
        }
        Ok(Self {
            empty_document,
            containers,
            trailing_separators,
            schema_fields,
            schema_empty_document,
        })
    }

    pub fn empty_document(&self) -> Option<&[u8]> {
        self.empty_document.as_deref()
    }

    pub fn containers(&self) -> &[CreatedContainer] {
        &self.containers
    }

    pub fn trailing_separators(&self) -> &[String] {
        &self.trailing_separators
    }

    pub fn schema_fields(&self) -> &[OwnedSelection] {
        &self.schema_fields
    }

    pub fn schema_empty_document(&self) -> Option<&[u8]> {
        self.schema_empty_document.as_deref()
    }
}

fn is_sha256_text(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PathPart {
    Key(String),
    Index(usize),
}

pub(crate) trait DocumentBackend {
    fn text(&self) -> &str;

    fn native(&self, path: &[PathPart]) -> Result<Value, ValueError>;

    fn syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError>;

    fn owned_syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError>;

    fn layout(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError>;

    fn position_prefix(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError>;

    fn empty_syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError>;

    fn put(
        &mut self,
        path: &[PathPart],
        value: &Value,
        syntax: Option<&[u8]>,
        layout: &[u8],
    ) -> Result<(), ValueError>;

    fn insert(
        &mut self,
        path: &[PathPart],
        index: usize,
        value: &Value,
        syntax: Option<&[u8]>,
        prefix: Option<&[u8]>,
        layout: &[u8],
    ) -> Result<(), ValueError>;

    fn delete(&mut self, path: &[PathPart]) -> Result<(), ValueError>;

    fn relocate(
        &mut self,
        path: &[PathPart],
        index: usize,
        value: &Value,
        syntax: Option<&[u8]>,
        prefix: Option<&[u8]>,
        layout: &[u8],
    ) -> Result<(), ValueError>;
}

enum Backend {
    Section,
    Structured(DocumentFormat),
}

impl Backend {
    const fn for_format(format: Option<DocumentFormat>) -> Self {
        match format {
            None => Self::Section,
            Some(format) => Self::Structured(format),
        }
    }
}

fn validate_inputs(
    requests: &[DocumentRequest],
    ownership: &[OwnedSelection],
) -> Result<Option<DocumentFormat>, ValueError> {
    for (index, item) in requests.iter().enumerate() {
        if requests
            .get(..index)
            .is_some_and(|rest| rest.iter().any(|other| other.claim() == item.claim()))
        {
            return Err(ValueError::new("duplicate document claim"));
        }
    }
    for (index, item) in ownership.iter().enumerate() {
        if ownership
            .get(..index)
            .is_some_and(|rest| rest.iter().any(|other| other.claim() == item.claim()))
        {
            return Err(ValueError::new("duplicate document claim"));
        }
    }
    let mut claims: Vec<&Claim> = Vec::new();
    for claim in requests
        .iter()
        .map(DocumentRequest::claim)
        .chain(ownership.iter().map(OwnedSelection::claim))
    {
        if !claims.contains(&claim) {
            claims.push(claim);
        }
    }
    for (index, claim) in claims.iter().enumerate() {
        if claims
            .get(index + 1..)
            .is_some_and(|rest| rest.iter().any(|other| claim.overlaps(other)))
        {
            return Err(ValueError::new("document claims overlap"));
        }
    }
    let mut formats: Vec<Option<DocumentFormat>> =
        ownership.iter().map(OwnedSelection::format).collect();
    for item in requests {
        if let Some(content) = item.content() {
            formats.push(match content {
                Content::Structured(structured) => Some(structured.format()),
                _ => None,
            });
        }
    }
    let mut distinct: Vec<Option<DocumentFormat>> = Vec::new();
    for format in formats {
        if !distinct.contains(&format) {
            distinct.push(format);
        }
    }
    if distinct.len() > 1 {
        return Err(ValueError::new("one document must use one editor format"));
    }
    Ok(distinct.first().copied().flatten())
}

pub(crate) fn decode_utf8(bytes: &[u8]) -> Result<String, ValueError> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(error) => {
            let start = error.valid_up_to();
            let end = error
                .error_len()
                .map_or(bytes.len(), |length| start.saturating_add(length));
            let length = end.saturating_sub(start);
            let description = if length == 1 {
                format!(
                    "byte 0x{:02x}",
                    bytes.get(start).copied().unwrap_or_default()
                )
            } else {
                "bytes".to_owned()
            };
            let position = if length == 1 {
                format!("position {start}")
            } else {
                format!("position {start}-{}", end.saturating_sub(1))
            };
            let reason = if error.error_len().is_none() {
                "unexpected end of data"
            } else if bytes
                .get(start)
                .is_some_and(|byte| !matches!(*byte, 0xC2..=0xF4))
            {
                "invalid start byte"
            } else {
                "invalid continuation byte"
            };
            Err(ValueError::new(format!(
                "'utf-8' codec can't decode {description} in {position}: {reason}"
            )))
        }
    }
}

fn decode_document(data: Option<&[u8]>) -> Result<(String, bool), ValueError> {
    let bytes = data.unwrap_or(b"");
    let bom: &[u8] = &[0xEF, 0xBB, 0xBF];
    let body = if bytes.starts_with(bom) {
        bytes.get(3..).unwrap_or(bytes)
    } else {
        bytes
    };
    let text = decode_utf8(body)?;
    if text.contains('\0') {
        return Err(ValueError::new("documents cannot contain NUL"));
    }
    Ok((text, body.len() != bytes.len()))
}

fn detect_newline(text: &str) -> &'static str {
    let Some(index) = text.find(['\r', '\n']) else {
        return "\n";
    };
    let bytes = text.as_bytes();
    if bytes.get(index) == Some(&b'\r') {
        if bytes.get(index + 1) == Some(&b'\n') {
            "\r\n"
        } else {
            "\r"
        }
    } else {
        "\n"
    }
}

fn normalize_newlines(text: &str, newline: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(['\r', '\n']) {
        out.push_str(rest.get(..index).unwrap_or_default());
        out.push_str(newline);
        let width = match rest.as_bytes().get(index) {
            Some(b'\r') if rest.as_bytes().get(index + 1) == Some(&b'\n') => 2,
            _ => 1,
        };
        rest = rest.get(index + width..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

fn generated_section(content: &SectionContent, newline: &str) -> Result<String, ValueError> {
    let selector = content.selector()?;
    let mut body = normalize_newlines(content.text(), newline);
    if !body.is_empty() && !body.ends_with(newline) {
        body.push_str(newline);
    }
    let mut section = String::with_capacity(
        selector.start().len() + selector.end().len() + body.len() + 2 * newline.len(),
    );
    section.push_str(selector.start());
    section.push_str(newline);
    section.push_str(&body);
    section.push_str(selector.end());
    section.push_str(newline);
    Ok(section)
}

struct SectionLines<'a> {
    offset: usize,
    rest: &'a str,
}

impl<'a> Iterator for SectionLines<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let start = self.offset;
        let (line, consumed) = match self.rest.find(['\r', '\n']) {
            Some(index) => {
                let width = match self.rest.as_bytes().get(index) {
                    Some(b'\r') if self.rest.as_bytes().get(index + 1) == Some(&b'\n') => 2,
                    _ => 1,
                };
                (
                    self.rest.get(..index + width).unwrap_or_default(),
                    index + width,
                )
            }
            None => (self.rest, self.rest.len()),
        };
        self.rest = self.rest.get(consumed..).unwrap_or_default();
        self.offset += consumed;
        Some((start, line))
    }
}

fn strip_line_newline(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .or_else(|| line.strip_suffix('\r'))
        .unwrap_or(line)
}

fn scan_markers(text: &str, pairs: &mut Vec<SectionBoundaries>) -> Result<(), ValueError> {
    const PREFIX: &str = "<!-- flyrail:";
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while let Some(offset) = text.get(cursor..).and_then(|rest| rest.find(PREFIX)) {
        let marker_start = offset + cursor + PREFIX.len();
        let region_end = bytes
            .get(marker_start..)
            .and_then(|rest| {
                rest.iter()
                    .position(|byte| matches!(byte, b'\r' | b'\n' | b'<' | b'>'))
            })
            .map_or(bytes.len(), |position| marker_start + position);
        let mut matched = false;
        for colon in (marker_start..region_end).rev() {
            if matched {
                break;
            }
            if bytes.get(colon) != Some(&b':') || colon == marker_start {
                continue;
            }
            let after = text.get(colon + 1..).unwrap_or_default();
            for keyword in ["start", "end"] {
                let Some(rest) = after.strip_prefix(keyword) else {
                    continue;
                };
                if rest.starts_with(" -->") {
                    let marker = text.get(marker_start..colon).unwrap_or_default();
                    let pair = SectionBoundaries::from_marker(marker)?;
                    if !pairs.contains(&pair) {
                        pairs.push(pair);
                    }
                    matched = true;
                    break;
                }
            }
        }
        cursor = marker_start;
    }
    Ok(())
}

type SectionSpan = (SectionBoundaries, (usize, usize));

fn section_spans<'a>(
    text: &str,
    boundaries: impl IntoIterator<Item = &'a SectionBoundaries>,
) -> Result<Vec<SectionSpan>, ValueError> {
    let mut pairs: Vec<SectionBoundaries> = Vec::new();
    for boundary in boundaries {
        if !pairs.contains(boundary) {
            pairs.push(boundary.clone());
        }
    }
    scan_markers(text, &mut pairs)?;
    let mut marker_lines: Vec<(&str, &SectionBoundaries, bool)> = Vec::new();
    for pair in &pairs {
        for (line, opening) in [(pair.start(), true), (pair.end(), false)] {
            if marker_lines
                .iter()
                .any(|(existing, _, _)| *existing == line)
            {
                return Err(ValueError::new("section boundaries overlap"));
            }
            marker_lines.push((line, pair, opening));
        }
    }
    let mut spans: Vec<(SectionBoundaries, (usize, usize))> = Vec::new();
    let mut opened: Option<(&SectionBoundaries, usize)> = None;
    let lines = SectionLines {
        offset: 0,
        rest: text,
    };
    for (offset, line) in lines {
        let bare = strip_line_newline(line);
        if let Some((_, pair, opening)) = marker_lines
            .iter()
            .find(|(existing, _, _)| *existing == bare)
        {
            if *opening {
                if opened.is_some() || spans.iter().any(|(existing, _)| existing == *pair) {
                    return Err(ValueError::new("nested or repeated section boundaries"));
                }
                opened = Some((pair, offset));
            } else {
                let Some((open_pair, open_offset)) = opened else {
                    return Err(ValueError::new(
                        "malformed or intersecting section boundaries",
                    ));
                };
                if open_pair != *pair {
                    return Err(ValueError::new(
                        "malformed or intersecting section boundaries",
                    ));
                }
                spans.push(((*pair).clone(), (open_offset, offset + line.len())));
                opened = None;
            }
        } else if bare.contains("<!-- flyrail:")
            || marker_lines
                .iter()
                .any(|(existing, _, _)| bare.contains(existing))
        {
            return Err(ValueError::new(
                "section boundaries must occupy complete lines",
            ));
        }
    }
    if opened.is_some() {
        return Err(ValueError::new("section has no closing boundary"));
    }
    Ok(spans)
}

fn section_snapshot_of(text: &str) -> Result<SelectionSnapshot, ValueError> {
    let raw = text.as_bytes().to_vec();
    SelectionSnapshot::new(
        SnapshotValue::Bytes(raw.clone()),
        raw.clone(),
        raw,
        None,
        Vec::new(),
    )
}

fn section_snapshot(
    text: &str,
    span: Option<(usize, usize)>,
) -> Result<Option<SelectionSnapshot>, ValueError> {
    span.map_or(Ok(None), |(start, end)| {
        section_snapshot_of(text.get(start..end).unwrap_or_default()).map(Some)
    })
}

enum Desired<'a> {
    Retired,
    Section(&'a [u8]),
    Structured(&'a Value),
}

fn desired_matches(value: &SnapshotValue, desired: Desired<'_>) -> bool {
    match (value, desired) {
        (SnapshotValue::Bytes(bytes), Desired::Section(want)) => bytes.as_slice() == want,
        (SnapshotValue::Typed(item), Desired::Structured(want)) => item == want,
        _ => false,
    }
}

fn selection_status(
    current: Option<&SelectionSnapshot>,
    desired: Desired<'_>,
    owned: Option<&OwnedSelection>,
    acquisition: Acquisition,
    format: Option<DocumentFormat>,
) -> EditStatus {
    if let Some(old) = owned {
        let installed = old.installed();
        let Some(snapshot) = current else {
            return EditStatus::Modified;
        };
        if snapshot.value() != installed.value()
            || (format != Some(DocumentFormat::Json)
                && snapshot.owned_syntax() != installed.owned_syntax())
        {
            return EditStatus::Modified;
        }
        if matches!(desired, Desired::Retired) {
            return EditStatus::Removed;
        }
        return if desired_matches(snapshot.value(), desired) {
            EditStatus::Current
        } else {
            EditStatus::Updated
        };
    }
    if matches!(desired, Desired::Retired) {
        return EditStatus::Conflict;
    }
    let Some(snapshot) = current else {
        return EditStatus::Created;
    };
    if acquisition == Acquisition::Takeover {
        return EditStatus::TakenOver;
    }
    if acquisition == Acquisition::Adopt && desired_matches(snapshot.value(), desired) {
        return EditStatus::Adopted;
    }
    EditStatus::Conflict
}

pub(crate) fn observe_selections(
    data: Option<&[u8]>,
    ownership: &[OwnedSelection],
) -> Result<Vec<EditObservation>, ValueError> {
    let format = validate_inputs(&[], ownership)?;
    let (text, _) = decode_document(data)?;
    let snapshots = match Backend::for_format(format) {
        Backend::Section => {
            let boundaries = ownership
                .iter()
                .filter_map(|item| match item.claim().selector() {
                    ClaimSelector::Section(boundaries) => Some(boundaries),
                    _ => None,
                });
            let spans = section_spans(&text, boundaries)?;
            ownership
                .iter()
                .map(|item| {
                    let current = match item.claim().selector() {
                        ClaimSelector::Section(boundaries) => section_snapshot(
                            &text,
                            spans
                                .iter()
                                .find(|(candidate, _)| candidate == boundaries)
                                .map(|(_, span)| *span),
                        )?,
                        _ => None,
                    };
                    Ok(current)
                })
                .collect::<Result<Vec<_>, ValueError>>()?
        }
        Backend::Structured(format) => {
            let backend = StructuredBackend::parse(&text, format)?;
            ownership
                .iter()
                .map(|item| match item.claim().selector() {
                    ClaimSelector::Structured(selector) => {
                        structured_snapshot(backend.as_ref(), selector)
                    }
                    _ => Ok(None),
                })
                .collect::<Result<Vec<_>, ValueError>>()?
        }
    };
    Ok(ownership
        .iter()
        .zip(snapshots)
        .map(|(item, current)| {
            let status = selection_status(
                current.as_ref(),
                match item.installed().value() {
                    SnapshotValue::Bytes(bytes) => Desired::Section(bytes),
                    SnapshotValue::Typed(value) => Desired::Structured(value),
                },
                Some(item),
                Acquisition::Conflict,
                format,
            );
            EditObservation::new(item.claim().clone(), status, current, String::new())
        })
        .collect())
}

fn disposition_from_status(status: EditStatus) -> Result<Disposition, ValueError> {
    match status {
        EditStatus::Created => Ok(Disposition::Created),
        EditStatus::TakenOver => Ok(Disposition::TakenOver),
        EditStatus::Adopted => Ok(Disposition::Adopted),
        _ => Err(ValueError::new(format!(
            "'{}' is not a valid Disposition",
            status.as_str()
        ))),
    }
}

fn ownership_for(
    claim: &Claim,
    format: Option<DocumentFormat>,
    installed: SelectionSnapshot,
    current: Option<SelectionSnapshot>,
    previous: Option<OwnedSelection>,
    status: EditStatus,
    separator: &[u8],
) -> Result<OwnedSelection, ValueError> {
    if let Some(previous) = previous {
        return OwnedSelection::new(
            previous.claim().clone(),
            previous.format(),
            installed,
            previous.disposition(),
            previous.baseline().cloned(),
            previous.separator().to_vec(),
        );
    }
    let disposition = disposition_from_status(status)?;
    let baseline = if disposition == Disposition::TakenOver {
        current
    } else {
        None
    };
    OwnedSelection::new(
        claim.clone(),
        format,
        installed,
        disposition,
        baseline,
        separator.to_vec(),
    )
}

fn upsert_owned(
    owned: &mut Vec<(Claim, OwnedSelection)>,
    claim: &Claim,
    selection: OwnedSelection,
) {
    match owned.iter_mut().find(|(existing, _)| *existing == *claim) {
        Some((_, value)) => *value = selection,
        None => owned.push((claim.clone(), selection)),
    }
}

fn with_empty_document(provenance: &DocumentProvenance) -> DocumentProvenance {
    DocumentProvenance {
        empty_document: Some(Vec::new()),
        containers: provenance.containers().to_vec(),
        trailing_separators: provenance.trailing_separators().to_vec(),
        schema_fields: provenance.schema_fields().to_vec(),
        schema_empty_document: provenance.schema_empty_document().map(<[u8]>::to_vec),
    }
}

#[allow(clippy::too_many_lines)]
fn edit_sections(
    data: Option<Vec<u8>>,
    text: &str,
    has_bom: bool,
    requests: &[DocumentRequest],
    previous: &[OwnedSelection],
    provenance: &DocumentProvenance,
) -> Result<DocumentEdit, ValueError> {
    let mut boundaries: Vec<&SectionBoundaries> = Vec::new();
    for claim in requests
        .iter()
        .map(DocumentRequest::claim)
        .chain(previous.iter().map(OwnedSelection::claim))
    {
        match claim.selector() {
            ClaimSelector::Section(pair) => boundaries.push(pair),
            _ => {
                return Err(ValueError::new(
                    "retirement requires existing ownership and its document format",
                ));
            }
        }
    }
    let spans = section_spans(text, boundaries.iter().copied())?;
    let mut owned: Vec<(Claim, OwnedSelection)> = previous
        .iter()
        .map(|item| (item.claim().clone(), item.clone()))
        .collect();
    let mut observations: Vec<EditObservation> = Vec::new();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut additions: Vec<(Claim, String, EditStatus)> = Vec::new();
    let newline = detect_newline(text);
    let mut body = String::from(text);
    for request in requests {
        let ClaimSelector::Section(pair) = request.claim().selector() else {
            return Err(ValueError::new("text editor requires section boundaries"));
        };
        let span = spans
            .iter()
            .find(|(candidate, _)| candidate == pair)
            .map(|(_, span)| *span);
        let current = section_snapshot(text, span)?;
        let section_newline = span.map_or(newline, |(start, end)| {
            text.get(start..end).map_or("\n", detect_newline)
        });
        let desired = match request.content() {
            Some(Content::Section(content)) => Some(generated_section(content, section_newline)?),
            _ => None,
        };
        let old = owned
            .iter()
            .find(|(claim, _)| claim == request.claim())
            .map(|(_, selection)| selection.clone());
        let status = selection_status(
            current.as_ref(),
            desired
                .as_deref()
                .map_or(Desired::Retired, |text| Desired::Section(text.as_bytes())),
            old.as_ref(),
            request.acquisition(),
            None,
        );
        observations.push(EditObservation::new(
            request.claim().clone(),
            status,
            current.clone(),
            String::new(),
        ));
        if matches!(status, EditStatus::Conflict | EditStatus::Modified) {
            continue;
        }
        if status == EditStatus::Removed {
            let Some(old) = old.as_ref() else {
                return Err(ValueError::new("retirement requires existing ownership"));
            };
            let Some((start, end)) = span else {
                return Err(ValueError::new("retirement requires existing ownership"));
            };
            let replacement = match old.baseline() {
                Some(baseline) => decode_utf8(baseline.syntax())?,
                None => String::new(),
            };
            let mut start = start;
            if replacement.is_empty() && !old.separator().is_empty() {
                let separator = decode_utf8(old.separator())?;
                if let Some(separator_start) = start.checked_sub(separator.len())
                    && text
                        .get(..start)
                        .is_some_and(|prefix| prefix.ends_with(separator.as_str()))
                    && spans.iter().all(|(_, (other_start, other_end))| {
                        !(separator_start < *other_end && *other_start < start)
                    })
                {
                    start = separator_start;
                }
            }
            edits.push((start, end, replacement));
            owned.retain(|(claim, _)| claim != request.claim());
        } else if let Some(desired_text) = desired {
            match span {
                None => additions.push((request.claim().clone(), desired_text, status)),
                Some((start, end)) => {
                    if matches!(status, EditStatus::Updated | EditStatus::TakenOver) {
                        edits.push((start, end, desired_text.clone()));
                    }
                    let installed = match status {
                        EditStatus::Updated | EditStatus::TakenOver => {
                            Some(section_snapshot_of(&desired_text)?)
                        }
                        _ => current.clone(),
                    };
                    let Some(installed) = installed else {
                        return Err(ValueError::new("selection is missing"));
                    };
                    let selection = ownership_for(
                        request.claim(),
                        None,
                        installed,
                        current.clone(),
                        old.clone(),
                        status,
                        b"",
                    )?;
                    upsert_owned(&mut owned, request.claim(), selection);
                }
            }
        }
    }
    edits.sort();
    if edits
        .iter()
        .zip(edits.iter().skip(1))
        .any(|(first, second)| first.1 > second.0)
    {
        return Err(ValueError::new("section edits overlap"));
    }
    for (start, end, replacement) in edits.iter().rev() {
        body.replace_range(*start..*end, replacement.as_str());
    }
    let doubled = format!("{newline}{newline}");
    for (claim, desired_text, status) in additions {
        let separator = if body.is_empty() || body.ends_with(doubled.as_str()) {
            ""
        } else if body.ends_with(newline) {
            newline
        } else {
            doubled.as_str()
        };
        body.push_str(separator);
        body.push_str(&desired_text);
        let installed = section_snapshot_of(&desired_text)?;
        let selection = ownership_for(
            &claim,
            None,
            installed,
            None,
            None,
            status,
            separator.as_bytes(),
        )?;
        upsert_owned(&mut owned, &claim, selection);
    }
    section_spans(&body, boundaries.iter().copied())?;
    let owned: Vec<OwnedSelection> = owned.into_iter().map(|(_, selection)| selection).collect();
    let creation = if data.is_none() && !owned.is_empty() {
        with_empty_document(provenance)
    } else {
        provenance.clone()
    };
    let mut after = Vec::with_capacity(body.len() + 3);
    if has_bom {
        after.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    after.extend_from_slice(body.as_bytes());
    Ok(edit_result(
        data,
        after,
        previous.to_vec(),
        owned,
        provenance,
        &creation,
        observations,
    ))
}

fn identity_at(value: &Value, member: &Member) -> Result<Option<Value>, ValueError> {
    let mut current = value;
    for name in member.key() {
        let Value::Object(object) = current else {
            return Ok(None);
        };
        let Some(next) = object.get(name) else {
            return Ok(None);
        };
        current = next;
    }
    check_identity_frames(current)?;
    Ok(
        if member.key().is_empty() || matches!(current, Value::Scalar(_)) {
            Some(current.clone())
        } else {
            None
        },
    )
}

fn member_index(array: &[Value], member: &Member) -> Result<Option<usize>, ValueError> {
    let mut seen: Vec<Value> = Vec::new();
    let mut found = None;
    for (index, item) in array.iter().enumerate() {
        let Some(identity) = identity_at(item, member)? else {
            continue;
        };
        if seen.contains(&identity) {
            return Err(ValueError::new(
                "duplicate or ambiguous array member identities",
            ));
        }
        if identity == *member.identity() {
            found = Some(index);
        }
        seen.push(identity);
    }
    Ok(found)
}

fn resolve(
    backend: &dyn DocumentBackend,
    selector: &Selector,
) -> Result<Option<Vec<PathPart>>, ValueError> {
    let mut path: Vec<PathPart> = Vec::new();
    for part in selector.parts() {
        let node = backend.native(&path)?;
        match part {
            SelectorPart::Key(key) => {
                let Value::Object(object) = &node else {
                    return Err(ValueError::new("object key selector crosses a non-object"));
                };
                if object.get(key.name()).is_none() {
                    return Ok(None);
                }
                path.push(PathPart::Key(key.name().to_owned()));
            }
            SelectorPart::Member(member) => {
                let Value::Array(array) = &node else {
                    return Err(ValueError::new("member selector requires an array"));
                };
                let Some(index) = member_index(array.items(), member)? else {
                    return Ok(None);
                };
                path.push(PathPart::Index(index));
            }
        }
    }
    Ok(Some(path))
}

fn anchor(array: &[Value], index: usize, key: &[String]) -> Result<Member, ValueError> {
    let item = array
        .get(index)
        .cloned()
        .ok_or_else(|| ValueError::new("array position has an ambiguous anchor"))?;
    let probe = Member::new(Scalar::null(), key.to_vec())?;
    let result = if let Some(identity) = identity_at(&item, &probe)? {
        Member::new(identity, key.to_vec())?
    } else {
        check_identity_frames(&item)?;
        Member::new(item, Vec::<String>::new())?
    };
    if member_index(array, &result)? != Some(index) {
        return Err(ValueError::new("array position has an ambiguous anchor"));
    }
    Ok(result)
}

fn array_position(
    backend: &dyn DocumentBackend,
    path: &[PathPart],
    member: &Member,
) -> Result<ArrayPosition, ValueError> {
    let Some((last, parent_path)) = path.split_last() else {
        return Err(ValueError::new("array position requires an array member"));
    };
    let PathPart::Index(index) = last else {
        return Err(ValueError::new("array position requires an array member"));
    };
    let parent = backend.native(parent_path)?;
    let Value::Array(array) = &parent else {
        return Err(ValueError::new("array position requires an array member"));
    };
    let previous = if *index > 0 {
        Some(anchor(array.items(), index - 1, member.key())?)
    } else {
        None
    };
    let following = if index + 1 < array.items().len() {
        Some(anchor(array.items(), index + 1, member.key())?)
    } else {
        None
    };
    ArrayPosition::new(previous, following, backend.position_prefix(path)?)
}

pub(crate) fn structured_snapshot(
    backend: &dyn DocumentBackend,
    selector: &Selector,
) -> Result<Option<SelectionSnapshot>, ValueError> {
    let Some(path) = resolve(backend, selector)? else {
        return Ok(None);
    };
    let value = backend.native(&path)?;
    check_snapshot_frames(&value)?;
    let position = match selector.parts().last() {
        Some(SelectorPart::Member(member)) => Some(array_position(backend, &path, member)?),
        _ => None,
    };
    Ok(Some(SelectionSnapshot::new(
        SnapshotValue::Typed(value),
        backend.syntax(&path)?,
        backend.owned_syntax(&path)?,
        position,
        backend.layout(&path)?,
    )?))
}

fn insertion_index(array: &[Value], placement: Option<&Placement>) -> Result<usize, ValueError> {
    let Some(placement) = placement else {
        return Ok(array.len());
    };
    match placement.position() {
        Position::First => Ok(0),
        Position::Last => Ok(array.len()),
        Position::Before | Position::After => {
            let Some(item) = placement.anchor() else {
                return Err(ValueError::new("array placement requires an anchor"));
            };
            let Some(index) = member_index(array, item)? else {
                return Err(ValueError::new("array placement anchor is missing"));
            };
            Ok(index + usize::from(placement.position() == Position::After))
        }
    }
}

fn restoration_index(array: &[Value], position: &ArrayPosition) -> Result<usize, ValueError> {
    let previous = match position.previous() {
        Some(item) => member_index(array, item)?,
        None => None,
    };
    let following = match position.following() {
        Some(item) => member_index(array, item)?,
        None => None,
    };
    if previous.is_some_and(|left| following.is_some_and(|right| left >= right)) {
        return Err(ValueError::new(
            "foreign array order prevents safe restoration",
        ));
    }
    if let Some(previous) = previous {
        return Ok(previous + 1);
    }
    if let Some(following) = following {
        return Ok(following);
    }
    if !array.is_empty() {
        return Err(ValueError::new(
            "original array position has no surviving anchors",
        ));
    }
    Ok(0)
}

fn without_index(array: &[Value], skip: usize) -> Vec<Value> {
    array
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != skip)
        .map(|(_, item)| item.clone())
        .collect()
}

fn parents(
    backend: &mut dyn DocumentBackend,
    selector: &Selector,
    containers: &mut Vec<CreatedContainer>,
) -> Result<Vec<PathPart>, ValueError> {
    let parts = selector.parts();
    let mut path: Vec<PathPart> = Vec::new();
    for (index, part) in parts.iter().enumerate().take(parts.len().saturating_sub(1)) {
        let node = backend.native(&path)?;
        match part {
            SelectorPart::Key(key) => {
                let Value::Object(object) = &node else {
                    return Err(ValueError::new("object key selector crosses a non-object"));
                };
                path.push(PathPart::Key(key.name().to_owned()));
                if object.get(key.name()).is_none() {
                    let next_is_key = parts
                        .get(index + 1)
                        .is_some_and(|next| matches!(next, SelectorPart::Key(_)));
                    let value = if next_is_key {
                        Value::Object(ObjectValue::new::<String>([])?)
                    } else {
                        Value::Array(ArrayValue::new([]))
                    };
                    backend.put(&path, &value, None, b"")?;
                    containers.push(CreatedContainer::new(
                        Selector::new(parts.iter().take(index + 1).cloned())?,
                        backend.empty_syntax(&path)?,
                    ));
                }
            }
            SelectorPart::Member(member) => {
                let Value::Array(array) = &node else {
                    return Err(ValueError::new("member selector requires an array"));
                };
                let Some(found) = member_index(array.items(), member)? else {
                    return Err(ValueError::new("intermediate array member is missing"));
                };
                path.push(PathPart::Index(found));
            }
        }
    }
    Ok(path)
}

fn put_structured(
    backend: &mut dyn DocumentBackend,
    content: &StructuredContent,
    containers: &mut Vec<CreatedContainer>,
) -> Result<(), ValueError> {
    let selector = content.selector();
    let parts = selector.parts();
    let Some(part) = parts.last() else {
        return Err(ValueError::new("a selector must select a key or member"));
    };
    let value = content.value();
    let parent = parents(backend, selector, containers)?;
    let current = backend.native(&parent)?;
    match part {
        SelectorPart::Key(key) => {
            if !matches!(current, Value::Object(_)) {
                return Err(ValueError::new("object key selector crosses a non-object"));
            }
            let mut path = parent;
            path.push(PathPart::Key(key.name().to_owned()));
            backend.put(&path, value, None, b"")
        }
        SelectorPart::Member(member) => {
            let Value::Array(array) = &current else {
                return Err(ValueError::new("member selector requires an array"));
            };
            if identity_at(value, member)?.as_ref() != Some(member.identity()) {
                return Err(ValueError::new(
                    "generated array member does not match its selector",
                ));
            }
            if let Some(index) = member_index(array.items(), member)? {
                let remaining = without_index(array.items(), index);
                let insertion = match content.placement() {
                    None => index,
                    Some(placement) => insertion_index(&remaining, Some(placement))?,
                };
                let mut path = parent;
                path.push(PathPart::Index(index));
                if insertion == index {
                    backend.put(&path, value, None, b"")?;
                } else {
                    backend.relocate(&path, insertion, value, None, None, b"")?;
                }
                Ok(())
            } else {
                let insertion = insertion_index(array.items(), content.placement())?;
                backend.insert(&parent, insertion, value, None, None, b"")
            }
        }
    }
}

fn restore_structured(
    backend: &mut dyn DocumentBackend,
    selector: &Selector,
    baseline: Option<&SelectionSnapshot>,
) -> Result<(), ValueError> {
    let Some(path) = resolve(backend, selector)? else {
        return Err(ValueError::new("owned selection is missing"));
    };
    let Some(baseline) = baseline else {
        return backend.delete(&path);
    };
    if let Some(SelectorPart::Key(_)) = selector.parts().last() {
        let SnapshotValue::Typed(value) = baseline.value() else {
            return Err(ValueError::new(
                "structured baseline requires a typed value",
            ));
        };
        backend.put(&path, value, Some(baseline.syntax()), baseline.layout())
    } else {
        let Some(position) = baseline.position() else {
            return Err(ValueError::new(
                "array baseline requires its original position and typed value",
            ));
        };
        let SnapshotValue::Typed(value) = baseline.value() else {
            return Err(ValueError::new(
                "array baseline requires its original position and typed value",
            ));
        };
        let Some((last, parent_path)) = path.split_last() else {
            return Err(ValueError::new("member selector requires an array"));
        };
        let PathPart::Index(index) = last else {
            return Err(ValueError::new("member selector requires an array"));
        };
        let native = backend.native(parent_path)?;
        let Value::Array(array) = &native else {
            return Err(ValueError::new("member selector requires an array"));
        };
        let remaining = without_index(array.items(), *index);
        let insertion = restoration_index(&remaining, position)?;
        if insertion == *index {
            backend.put(&path, value, Some(baseline.syntax()), baseline.layout())
        } else {
            backend.relocate(
                &path,
                insertion,
                value,
                Some(baseline.syntax()),
                Some(position.prefix()),
                baseline.layout(),
            )
        }
    }
}

fn add_edge(edges: &mut [(usize, Vec<usize>)], index: usize, dependency: usize) {
    if let Some((_, dependencies)) = edges.iter_mut().find(|(existing, _)| *existing == index)
        && !dependencies.contains(&dependency)
    {
        dependencies.push(dependency);
    }
}

#[allow(clippy::too_many_lines)]
fn restore_batch(
    backend: &mut dyn DocumentBackend,
    selections: &[OwnedSelection],
) -> Result<(), ValueError> {
    let mut snapshots: Vec<(usize, SelectionSnapshot)> = Vec::new();
    let mut array_path: Vec<PathPart> = Vec::new();
    let mut array: Vec<Value> = Vec::new();
    for selection in selections {
        let ClaimSelector::Structured(selector) = selection.claim().selector() else {
            return Err(ValueError::new(
                "array restoration requires a baseline and selector",
            ));
        };
        let Some(baseline) = selection.baseline() else {
            return Err(ValueError::new(
                "array restoration requires a baseline and selector",
            ));
        };
        let Some(path) = resolve(backend, selector)? else {
            return Err(ValueError::new("owned array member is missing"));
        };
        let Some((last, parent)) = path.split_last() else {
            return Err(ValueError::new("owned array member is missing"));
        };
        let PathPart::Index(index) = last else {
            return Err(ValueError::new("owned array member is missing"));
        };
        array_path = parent.to_vec();
        let native = backend.native(&array_path)?;
        let Value::Array(items) = &native else {
            return Err(ValueError::new("member selector requires an array"));
        };
        array = items.items().to_vec();
        match snapshots.iter_mut().find(|(existing, _)| existing == index) {
            Some((_, snapshot)) => *snapshot = baseline.clone(),
            None => snapshots.push((*index, baseline.clone())),
        }
    }
    let length = array.len();
    let tracked = |index: usize| snapshots.iter().any(|(existing, _)| *existing == index);
    let mut edges: Vec<(usize, Vec<usize>)> =
        (0..length).map(|index| (index, Vec::new())).collect();
    let foreign: Vec<usize> = (0..length).filter(|index| !tracked(*index)).collect();
    for pair in foreign.windows(2) {
        let (Some(left), Some(right)) = (pair.first().copied(), pair.get(1).copied()) else {
            continue;
        };
        add_edge(&mut edges, right, left);
    }
    for (index, baseline) in &snapshots {
        let Some(position) = baseline.position() else {
            return Err(ValueError::new(
                "array baseline requires its original position",
            ));
        };
        let previous = match position.previous() {
            Some(item) => member_index(&array, item)?,
            None => None,
        };
        let following = match position.following() {
            Some(item) => member_index(&array, item)?,
            None => None,
        };
        if previous.is_none() && following.is_none() && length > 1 {
            return Err(ValueError::new(
                "original array position has no surviving anchors",
            ));
        }
        if let Some(previous) = previous {
            add_edge(&mut edges, *index, previous);
        }
        if let Some(following) = following {
            add_edge(&mut edges, following, *index);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    while !edges.is_empty() {
        let ready = edges
            .iter()
            .filter(|(_, dependencies)| dependencies.is_empty())
            .map(|(index, _)| *index)
            .min_by_key(|index| (!tracked(*index), *index))
            .ok_or_else(|| ValueError::new("foreign array order prevents safe restoration"))?;
        order.push(ready);
        edges.retain(|(index, _)| *index != ready);
        for (_, dependencies) in &mut edges {
            dependencies.retain(|dependency| *dependency != ready);
        }
    }
    let mut current: Vec<usize> = (0..length).collect();
    for (target, identity) in order.iter().enumerate() {
        let tracked_identity = tracked(*identity);
        while !tracked_identity && current.get(target).copied() != Some(*identity) {
            let mut path = array_path.clone();
            path.push(PathPart::Index(target));
            let value = backend.native(&path)?;
            let syntax = backend.syntax(&path)?;
            let layout = backend.layout(&path)?;
            let last = current.len().saturating_sub(1);
            backend.relocate(&path, last, &value, Some(&syntax), None, &layout)?;
            let moved = current.get(target).copied().unwrap_or_default();
            current.remove(target);
            current.push(moved);
        }
        if !tracked_identity {
            continue;
        }
        let Some(baseline) = snapshots
            .iter()
            .find(|(existing, _)| existing == identity)
            .map(|(_, snapshot)| snapshot)
        else {
            continue;
        };
        let SnapshotValue::Typed(value) = baseline.value() else {
            return Err(ValueError::new(
                "array baseline requires a typed value and position",
            ));
        };
        let Some(position) = baseline.position() else {
            return Err(ValueError::new(
                "array baseline requires a typed value and position",
            ));
        };
        let Some(index) = current.iter().position(|item| item == identity) else {
            return Err(ValueError::new("owned array member is missing"));
        };
        let mut path = array_path.clone();
        path.push(PathPart::Index(index));
        if index == target {
            backend.put(&path, value, Some(baseline.syntax()), baseline.layout())?;
        } else {
            backend.relocate(
                &path,
                target,
                value,
                Some(baseline.syntax()),
                Some(position.prefix()),
                baseline.layout(),
            )?;
            let moved = current.remove(index);
            current.insert(target, moved);
        }
    }
    Ok(())
}

fn prune(
    backend: &mut dyn DocumentBackend,
    containers: &[CreatedContainer],
    owned: &[OwnedSelection],
) -> Result<Vec<CreatedContainer>, ValueError> {
    let mut ordered: Vec<&CreatedContainer> = containers.iter().collect();
    ordered.sort_by_key(|container| std::cmp::Reverse(container.selector().parts().len()));
    let mut retained: Vec<CreatedContainer> = Vec::new();
    for container in ordered {
        let Some(path) = resolve(backend, container.selector())? else {
            continue;
        };
        let claim = Claim::new(
            Ownership::Structured,
            ClaimSelector::Structured(container.selector().clone()),
        )?;
        let value = backend.native(&path)?;
        let empty = match &value {
            Value::Object(object) => object.is_empty(),
            Value::Array(array) => array.is_empty(),
            Value::Scalar(_) => false,
        };
        if empty
            && backend.empty_syntax(&path)? == container.empty_syntax()
            && !owned.iter().any(|item| claim.overlaps(item.claim()))
        {
            backend.delete(&path)?;
        } else {
            retained.push(container.clone());
        }
    }
    retained.reverse();
    Ok(retained)
}

#[derive(Clone)]
enum StructuredBackend {
    Json(JsonDocument),
    Toml(crate::toml_document::TomlDocument),
}

impl StructuredBackend {
    fn parse(text: &str, format: DocumentFormat) -> Result<Self, ValueError> {
        match format {
            DocumentFormat::Toml => {
                Ok(Self::Toml(crate::toml_document::TomlDocument::parse(text)?))
            }
            _ => Ok(Self::Json(JsonDocument::parse(
                text,
                format == DocumentFormat::Jsonc,
            )?)),
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Json(document) => document.text(),
            Self::Toml(document) => document.text(),
        }
    }

    fn as_ref(&self) -> &dyn DocumentBackend {
        match self {
            Self::Json(document) => document,
            Self::Toml(document) => document,
        }
    }

    fn as_mut(&mut self) -> &mut dyn DocumentBackend {
        match self {
            Self::Json(document) => document,
            Self::Toml(document) => document,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn structured_request(
    backend: &StructuredBackend,
    request: &DocumentRequest,
    current: Option<&SelectionSnapshot>,
    old: Option<&OwnedSelection>,
    status: EditStatus,
    format: DocumentFormat,
    containers: &mut Vec<CreatedContainer>,
    owned: &mut Vec<(Claim, OwnedSelection)>,
    retired: &mut Vec<Claim>,
    groups: &[(Vec<SelectorPart>, Vec<OwnedSelection>)],
) -> Result<(StructuredBackend, EditStatus), ValueError> {
    let content = match request.content() {
        Some(Content::Structured(item)) => Some(item),
        _ => None,
    };
    let reuse = matches!(status, EditStatus::Current | EditStatus::Adopted)
        && content.is_some_and(|item| item.placement().is_none());
    let mut candidate = if reuse {
        backend.clone()
    } else {
        StructuredBackend::parse(backend.text(), format)?
    };
    let mut status = status;
    if status == EditStatus::Removed {
        let Some(old) = old else {
            return Err(ValueError::new("retirement requires existing ownership"));
        };
        let claim = request.claim();
        let already = retired.iter().any(|item| item == claim);
        let batch = groups
            .iter()
            .find(|(_, group)| group.len() > 1 && group.iter().any(|item| item.claim() == claim))
            .map(|(_, group)| group);
        if let Some(group) = batch
            && !already
        {
            restore_batch(candidate.as_mut(), group)?;
            retired.extend(group.iter().map(|item| item.claim().clone()));
        } else if !already {
            let ClaimSelector::Structured(selector) = claim.selector() else {
                return Err(ValueError::new("structured editor requires a Selector"));
            };
            restore_structured(candidate.as_mut(), selector, old.baseline())?;
        }
        owned.retain(|(existing, _)| existing != claim);
    } else if let Some(content) = content {
        if !matches!(status, EditStatus::Adopted | EditStatus::Current)
            || content.placement().is_some()
        {
            let before = candidate.text().to_owned();
            put_structured(candidate.as_mut(), content, containers)?;
            if status == EditStatus::Current && candidate.text() != before {
                status = EditStatus::Updated;
            }
        }
        let ClaimSelector::Structured(selector) = request.claim().selector() else {
            return Err(ValueError::new("structured editor requires a Selector"));
        };
        let installed = structured_snapshot(candidate.as_ref(), selector)?;
        let matches_desired = installed.as_ref().is_some_and(|snapshot| {
            matches!(snapshot.value(), SnapshotValue::Typed(value) if value == content.value())
        });
        let Some(installed) = installed else {
            return Err(ValueError::new(
                "published selection differs from the requested value",
            ));
        };
        if !matches_desired {
            return Err(ValueError::new(
                "published selection differs from the requested value",
            ));
        }
        let selection = ownership_for(
            request.claim(),
            Some(format),
            installed,
            current.cloned(),
            old.cloned(),
            status,
            b"",
        )?;
        upsert_owned(owned, request.claim(), selection);
    }
    Ok((candidate, status))
}

#[allow(clippy::too_many_lines)]
fn edit_structured(
    data: Option<Vec<u8>>,
    text: &str,
    has_bom: bool,
    format: DocumentFormat,
    requests: &[DocumentRequest],
    previous: &[OwnedSelection],
    provenance: &DocumentProvenance,
) -> Result<DocumentEdit, ValueError> {
    let text = if data.is_none() && format == DocumentFormat::Toml {
        ""
    } else if data.is_none() {
        let root_array = requests
            .iter()
            .map(DocumentRequest::claim)
            .chain(previous.iter().map(OwnedSelection::claim))
            .any(|claim| {
                matches!(
                    claim.selector(),
                    ClaimSelector::Structured(selector)
                        if matches!(selector.parts().first(), Some(SelectorPart::Member(_)))
                )
            });
        if root_array { "[]" } else { "{}" }
    } else {
        text
    };
    let mut backend = StructuredBackend::parse(text, format)?;
    let mut owned: Vec<(Claim, OwnedSelection)> = previous
        .iter()
        .map(|item| (item.claim().clone(), item.clone()))
        .collect();
    let mut containers: Vec<CreatedContainer> = provenance.containers().to_vec();
    let mut observations: Vec<EditObservation> = Vec::new();
    let mut preimages: Vec<(Claim, Option<SelectionSnapshot>)> = Vec::new();
    for request in requests {
        if let ClaimSelector::Structured(selector) = request.claim().selector() {
            preimages.push((
                request.claim().clone(),
                structured_snapshot(backend.as_ref(), selector)?,
            ));
        }
    }
    let mut groups: Vec<(Vec<SelectorPart>, Vec<OwnedSelection>)> = Vec::new();
    for request in requests {
        let Some(old) = owned
            .iter()
            .find(|(claim, _)| claim == request.claim())
            .map(|(_, selection)| selection.clone())
        else {
            continue;
        };
        let ClaimSelector::Structured(selector) = request.claim().selector() else {
            continue;
        };
        if request.content().is_none()
            && old.baseline().is_some()
            && matches!(selector.parts().last(), Some(SelectorPart::Member(_)))
            && let Some((_, current)) = preimages.iter().find(|(claim, _)| claim == request.claim())
            && selection_status(
                current.as_ref(),
                Desired::Retired,
                Some(&old),
                request.acquisition(),
                Some(format),
            ) == EditStatus::Removed
        {
            let key: Vec<SelectorPart> = selector
                .parts()
                .iter()
                .take(selector.parts().len().saturating_sub(1))
                .cloned()
                .collect();
            match groups.iter_mut().find(|(existing, _)| *existing == key) {
                Some((_, group)) => group.push(old),
                None => groups.push((key, vec![old])),
            }
        }
    }
    let mut retired: Vec<Claim> = Vec::new();
    let original_owned = owned.clone();
    for request in requests {
        let ClaimSelector::Structured(_selector) = request.claim().selector() else {
            return Err(ValueError::new("structured editor requires a Selector"));
        };
        let current = preimages
            .iter()
            .find(|(claim, _)| claim == request.claim())
            .and_then(|(_, snapshot)| snapshot.as_ref());
        let desired = match request.content() {
            Some(Content::Structured(item)) => Desired::Structured(item.value()),
            _ => Desired::Retired,
        };
        let old = original_owned
            .iter()
            .find(|(claim, _)| claim == request.claim())
            .map(|(_, selection)| selection.clone());
        let status = selection_status(
            current,
            desired,
            old.as_ref(),
            request.acquisition(),
            Some(format),
        );
        if matches!(status, EditStatus::Conflict | EditStatus::Modified) {
            observations.push(EditObservation::new(
                request.claim().clone(),
                status,
                current.cloned(),
                String::new(),
            ));
            continue;
        }
        match structured_request(
            &backend,
            request,
            current,
            old.as_ref(),
            status,
            format,
            &mut containers,
            &mut owned,
            &mut retired,
            &groups,
        ) {
            Ok((candidate, final_status)) => {
                backend = candidate;
                observations.push(EditObservation::new(
                    request.claim().clone(),
                    final_status,
                    current.cloned(),
                    String::new(),
                ));
            }
            Err(error) if error.is_nesting() => return Err(error),
            Err(error) => {
                observations.push(EditObservation::new(
                    request.claim().clone(),
                    EditStatus::Conflict,
                    current.cloned(),
                    error.message().to_owned(),
                ));
            }
        }
    }
    if observations
        .iter()
        .any(|item| matches!(item.status(), EditStatus::Conflict | EditStatus::Modified))
    {
        return Ok(DocumentEdit::new(
            data.clone(),
            data,
            previous.to_vec(),
            provenance.clone(),
            observations,
        ));
    }
    let owned: Vec<OwnedSelection> = owned.into_iter().map(|(_, selection)| selection).collect();
    let containers = prune(backend.as_mut(), &containers, &owned)?;
    let mut separators: Vec<String> = Vec::new();
    for digest in provenance.trailing_separators() {
        if !separators.iter().any(|item| item == digest) {
            separators.push(digest.clone());
        }
    }
    if format == DocumentFormat::Toml
        && data.is_some()
        && !text.is_empty()
        && !text.ends_with(['\r', '\n'])
        && backend.text().ends_with(['\r', '\n'])
        && let Some(source) = &data
    {
        let digest = sha256_hex(&Sha256::digest(source));
        if !separators.iter().any(|item| item == &digest) {
            separators.push(digest);
        }
    }
    let empty_document = if data.is_none() && !owned.is_empty() {
        Some(text.as_bytes().to_vec())
    } else {
        provenance.empty_document().map(<[u8]>::to_vec)
    };
    let creation =
        DocumentProvenance::new(empty_document, containers, separators, Vec::new(), None)?;
    let mut after = Vec::with_capacity(backend.text().len() + 3);
    if has_bom {
        after.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    after.extend_from_slice(backend.text().as_bytes());
    Ok(edit_result(
        data,
        after,
        previous.to_vec(),
        owned,
        provenance,
        &creation,
        observations,
    ))
}

fn edit_result(
    before: Option<Vec<u8>>,
    after: Vec<u8>,
    previous: Vec<OwnedSelection>,
    owned: Vec<OwnedSelection>,
    old_provenance: &DocumentProvenance,
    provenance: &DocumentProvenance,
    observations: Vec<EditObservation>,
) -> DocumentEdit {
    if observations
        .iter()
        .any(|item| matches!(item.status(), EditStatus::Conflict | EditStatus::Modified))
    {
        return DocumentEdit::new(
            before.clone(),
            before,
            previous,
            old_provenance.clone(),
            observations,
        );
    }
    if owned.is_empty() {
        let mut after = after;
        for suffix in [b"\r\n" as &[u8], b"\n", b"\r"] {
            if after.ends_with(suffix)
                && let Some((body, _)) = after.split_at_checked(after.len() - suffix.len())
            {
                let digest = sha256_hex(&Sha256::digest(body));
                if provenance
                    .trailing_separators()
                    .iter()
                    .any(|item| item == &digest)
                {
                    after.truncate(body.len());
                    break;
                }
            }
        }
        let final_after = match provenance.empty_document() {
            Some(empty) if after == empty => None,
            _ => Some(after),
        };
        return DocumentEdit::new(
            before,
            final_after,
            owned,
            DocumentProvenance::default(),
            observations,
        );
    }
    DocumentEdit::new(before, Some(after), owned, provenance.clone(), observations)
}

pub fn edit_document(
    data: Option<Vec<u8>>,
    requests: &[DocumentRequest],
    ownership: &[OwnedSelection],
    provenance: &DocumentProvenance,
) -> Result<DocumentEdit, ValueError> {
    let format = validate_inputs(requests, ownership)?;
    let (text, has_bom) = decode_document(data.as_deref())?;
    if requests.is_empty() {
        return Ok(DocumentEdit::new(
            data.clone(),
            data,
            ownership.to_vec(),
            provenance.clone(),
            Vec::new(),
        ));
    }
    match Backend::for_format(format) {
        Backend::Section => edit_sections(data, &text, has_bom, requests, ownership, provenance),
        Backend::Structured(format) => edit_structured(
            data, &text, has_bom, format, requests, ownership, provenance,
        ),
    }
}

pub fn content_claim(content: &Content) -> Result<Claim, ValueError> {
    match content {
        Content::Section(section) => Claim::new(
            Ownership::Section,
            ClaimSelector::Section(section.selector()?),
        ),
        Content::Structured(structured) => Claim::new(
            Ownership::Structured,
            ClaimSelector::Structured(structured.selector().clone()),
        ),
        _ => Err(ValueError::type_error(
            "document content must be a section or structured selection",
        )),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentRequest {
    claim: Claim,
    content: Option<Content>,
    acquisition: Acquisition,
}

impl DocumentRequest {
    pub fn new(
        claim: Claim,
        content: Option<Content>,
        acquisition: Acquisition,
    ) -> Result<Self, ValueError> {
        if !matches!(claim.kind(), Ownership::Section | Ownership::Structured) {
            return Err(ValueError::new(
                "document requests require section or structured claims",
            ));
        }
        if let Some(value) = content.as_ref() {
            let expected = content_claim(value)?;
            if claim != expected {
                return Err(ValueError::new(
                    "requested content must address its physical claim",
                ));
            }
        } else if acquisition != Acquisition::Conflict {
            return Err(ValueError::new("retirement does not acquire ownership"));
        }
        Ok(Self {
            claim,
            content,
            acquisition,
        })
    }

    pub fn set(content: Content, acquisition: Acquisition) -> Result<Self, ValueError> {
        Self::new(content_claim(&content)?, Some(content), acquisition)
    }

    pub fn retire(claim: Claim) -> Result<Self, ValueError> {
        Self::new(claim, None, Acquisition::Conflict)
    }

    pub const fn claim(&self) -> &Claim {
        &self.claim
    }

    pub const fn content(&self) -> Option<&Content> {
        self.content.as_ref()
    }

    pub const fn acquisition(&self) -> Acquisition {
        self.acquisition
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditObservation {
    claim: Claim,
    status: EditStatus,
    current: Option<SelectionSnapshot>,
    message: String,
}

impl EditObservation {
    pub fn new(
        claim: Claim,
        status: EditStatus,
        current: Option<SelectionSnapshot>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            claim,
            status,
            current,
            message: message.into(),
        }
    }

    pub const fn claim(&self) -> &Claim {
        &self.claim
    }

    pub const fn status(&self) -> EditStatus {
        self.status
    }

    pub const fn current(&self) -> Option<&SelectionSnapshot> {
        self.current.as_ref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentEdit {
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    ownership: Vec<OwnedSelection>,
    provenance: DocumentProvenance,
    observations: Vec<EditObservation>,
}

impl DocumentEdit {
    pub(crate) const fn new(
        before: Option<Vec<u8>>,
        after: Option<Vec<u8>>,
        ownership: Vec<OwnedSelection>,
        provenance: DocumentProvenance,
        observations: Vec<EditObservation>,
    ) -> Self {
        Self {
            before,
            after,
            ownership,
            provenance,
            observations,
        }
    }

    pub fn before(&self) -> Option<&[u8]> {
        self.before.as_deref()
    }

    pub fn after(&self) -> Option<&[u8]> {
        self.after.as_deref()
    }

    pub fn ownership(&self) -> &[OwnedSelection] {
        &self.ownership
    }

    pub const fn provenance(&self) -> &DocumentProvenance {
        &self.provenance
    }

    pub fn observations(&self) -> &[EditObservation] {
        &self.observations
    }

    pub fn applicable(&self) -> bool {
        self.observations
            .iter()
            .all(|item| !matches!(item.status(), EditStatus::Conflict | EditStatus::Modified))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]
mod tests {
    use super::*;
    use crate::content::{FileContent, FileMode, Key, StructuredContent};
    use crate::json::Json;
    use crate::test_support::fixture;
    use crate::values::{ObjectValue, Scalar, Value, value_from_record};

    fn replace_all(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut cursor = 0;
        while cursor + from.len() <= data.len() {
            if &data[cursor..cursor + from.len()] == from {
                out.extend_from_slice(to);
                cursor += from.len();
            } else {
                out.push(data[cursor]);
                cursor += 1;
            }
        }
        out.extend_from_slice(&data[cursor..]);
        out
    }

    fn after_of(edit: &DocumentEdit) -> Option<Vec<u8>> {
        edit.after().map(<[u8]>::to_vec)
    }

    fn set_section(
        data: Option<Vec<u8>>,
        content: &SectionContent,
        acquisition: Acquisition,
    ) -> DocumentEdit {
        let request = DocumentRequest::set(Content::Section(content.clone()), acquisition)
            .expect("section request");
        edit_document(data, &[request], &[], &DocumentProvenance::default()).expect("edit")
    }

    fn update_section(
        edit: &DocumentEdit,
        content: &SectionContent,
        data: Option<Vec<u8>>,
    ) -> DocumentEdit {
        let request =
            DocumentRequest::set(Content::Section(content.clone()), Acquisition::Conflict)
                .expect("section request");
        edit_document(data, &[request], edit.ownership(), edit.provenance()).expect("edit")
    }

    fn retire_all(edit: &DocumentEdit, data: Option<Vec<u8>>) -> DocumentEdit {
        let requests: Vec<DocumentRequest> = edit
            .ownership()
            .iter()
            .map(|item| DocumentRequest::retire(item.claim().clone()).expect("retire request"))
            .collect();
        edit_document(data, &requests, edit.ownership(), edit.provenance()).expect("edit")
    }

    fn marker_claim(marker: &str) -> Claim {
        Claim::new(
            Ownership::Section,
            ClaimSelector::Subtree(marker.to_owned()),
        )
        .expect("marker claim")
    }

    fn structured_claim(key: &str) -> Claim {
        Claim::new(
            Ownership::Structured,
            ClaimSelector::Structured(
                Selector::new([SelectorPart::Key(Key::new(key).expect("key"))]).expect("selector"),
            ),
        )
        .expect("structured claim")
    }

    fn scalar_snapshot(value: &str) -> SelectionSnapshot {
        SelectionSnapshot::new(
            SnapshotValue::Typed(Value::Scalar(Scalar::text(value))),
            b"",
            b"",
            None,
            b"",
        )
        .expect("snapshot")
    }

    fn bytes_snapshot() -> SelectionSnapshot {
        SelectionSnapshot::new(SnapshotValue::Bytes(b"raw".to_vec()), b"", b"", None, b"")
            .expect("snapshot")
    }

    fn nested_snapshot(layout: &[u8]) -> SelectionSnapshot {
        let empty = Value::Object(ObjectValue::new::<String>([]).expect("empty object"));
        let nested = Value::Object(ObjectValue::new([("f", empty)]).expect("nested object"));
        SelectionSnapshot::new(SnapshotValue::Typed(nested), b"", b"", None, layout)
            .expect("snapshot")
    }

    #[test]
    fn array_position_rejects_identical_anchors() {
        let member = Member::new(Scalar::text("id"), ["k"]).expect("member");
        let same = ArrayPosition::new(Some(member.clone()), Some(member), b"");
        assert_eq!(
            same.unwrap_err().message(),
            "array position anchors must be distinct"
        );
    }

    #[test]
    fn selection_snapshot_layout_walks_match_python() {
        let object = Value::Object(
            ObjectValue::new([("f", Value::Scalar(Scalar::text("v")))]).expect("object"),
        );
        let missing = SelectionSnapshot::new(
            SnapshotValue::Typed(object.clone()),
            b"",
            b"",
            None,
            br#"{"version":1,"headers":[[["missing"],"h"]]}"#,
        );
        assert_eq!(
            missing.unwrap_err().message(),
            "invalid TOML restoration path"
        );
        let not_object = SelectionSnapshot::new(
            SnapshotValue::Typed(object),
            b"",
            b"",
            None,
            br#"{"version":1,"headers":[[["f"],"h"]]}"#,
        );
        assert_eq!(
            not_object.unwrap_err().message(),
            "TOML restoration header requires an object"
        );
        let layout = br#"{"version":1,"headers":[[["f"],"h"]]}"#;
        assert_eq!(nested_snapshot(layout).layout(), layout);
    }

    #[test]
    fn owned_selection_validation_messages_match_python() {
        let json = Some(DocumentFormat::Json);
        let file_claim = Claim::new(Ownership::File, ClaimSelector::Whole).expect("file claim");
        let wrong_kind = OwnedSelection::new(
            file_claim,
            None,
            bytes_snapshot(),
            Disposition::Created,
            None,
            b"",
        );
        assert_eq!(
            wrong_kind.unwrap_err().message(),
            "shared claim and document format disagree"
        );
        let baseline = OwnedSelection::new(
            structured_claim("b"),
            json,
            scalar_snapshot("v"),
            Disposition::Created,
            Some(scalar_snapshot("v")),
            b"",
        );
        assert_eq!(
            baseline.unwrap_err().message(),
            "only takeover ownership has a baseline"
        );
        let mismatched = OwnedSelection::new(
            structured_claim("b"),
            json,
            bytes_snapshot(),
            Disposition::Created,
            None,
            b"",
        );
        assert_eq!(
            mismatched.unwrap_err().message(),
            "selection snapshot and document format disagree"
        );
        let layout_needs_toml = OwnedSelection::new(
            structured_claim("b"),
            json,
            nested_snapshot(br#"{"version":1,"headers":[[["f"],"h"]]}"#),
            Disposition::Created,
            None,
            b"",
        );
        assert_eq!(
            layout_needs_toml.unwrap_err().message(),
            "restoration layout requires a TOML claim"
        );
        let header_mismatch = OwnedSelection::new(
            structured_claim("mcp"),
            Some(DocumentFormat::Toml),
            nested_snapshot(br#"{"version":1,"headers":[[["f"],"wrong.header"]]}"#),
            Disposition::Created,
            None,
            b"",
        );
        assert_eq!(
            header_mismatch.unwrap_err().message(),
            "TOML restoration header does not match its destination"
        );
        let separator = OwnedSelection::new(
            structured_claim("b"),
            json,
            scalar_snapshot("v"),
            Disposition::Created,
            None,
            b"x",
        );
        assert_eq!(
            separator.unwrap_err().message(),
            "section separators require a text claim"
        );
        let matched = OwnedSelection::new(
            structured_claim("mcp"),
            Some(DocumentFormat::Toml),
            nested_snapshot(br#"{"version":1,"headers":[[["f"],"mcp.f"]]}"#),
            Disposition::Created,
            None,
            b"",
        );
        assert!(matched.is_ok());
    }

    fn created_selection(key: &str) -> OwnedSelection {
        OwnedSelection::new(
            structured_claim(key),
            Some(DocumentFormat::Json),
            scalar_snapshot("value"),
            Disposition::Created,
            None,
            b"",
        )
        .expect("created selection")
    }

    fn container() -> CreatedContainer {
        CreatedContainer::new(
            Selector::new([SelectorPart::Key(Key::new("a").expect("key"))]).expect("selector"),
            b"",
        )
    }

    #[test]
    fn document_provenance_validation_messages_match_python() {
        let adopted = OwnedSelection::new(
            structured_claim("a"),
            Some(DocumentFormat::Json),
            scalar_snapshot("value"),
            Disposition::Adopted,
            None,
            b"",
        )
        .expect("adopted selection");
        let shape = DocumentProvenance::new(None, Vec::new(), Vec::new(), vec![adopted], None);
        assert_eq!(
            shape.unwrap_err().message(),
            "schema provenance requires created top-level scalar fields"
        );
        let duplicate_schema = DocumentProvenance::new(
            None,
            Vec::new(),
            Vec::new(),
            vec![created_selection("a"), created_selection("a")],
            None,
        );
        assert_eq!(
            duplicate_schema.unwrap_err().message(),
            "duplicate schema provenance"
        );
        let whitespace = DocumentProvenance::new(
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(b"x".to_vec()),
        );
        assert_eq!(
            whitespace.unwrap_err().message(),
            "schema empty-document evidence must be whitespace bytes"
        );
        let duplicate_container = DocumentProvenance::new(
            None,
            vec![container(), container()],
            Vec::new(),
            Vec::new(),
            None,
        );
        assert_eq!(
            duplicate_container.unwrap_err().message(),
            "duplicate container provenance"
        );
        let separator =
            DocumentProvenance::new(None, Vec::new(), vec![String::from("z")], Vec::new(), None);
        assert_eq!(
            separator.unwrap_err().message(),
            "separator evidence must be SHA-256 digests"
        );
        let ok = DocumentProvenance::new(
            None,
            vec![container()],
            vec!["a".repeat(64)],
            vec![created_selection("a")],
            Some(b" \n".to_vec()),
        );
        assert!(ok.is_ok());
    }

    fn fixture_boundaries(selector: &Json) -> Result<SectionBoundaries, ValueError> {
        SectionBoundaries::new(
            selector.get("start").and_then(Json::as_str).expect("start"),
            selector.get("end").and_then(Json::as_str).expect("end"),
        )
    }

    fn fixture_value(item: &Json) -> Result<Value, ValueError> {
        match item {
            Json::Null => Ok(Value::Scalar(Scalar::null())),
            Json::Bool(flag) => Ok(Value::Scalar(Scalar::boolean(*flag))),
            Json::Int(number) => Ok(Value::Scalar(Scalar::integer(*number))),
            Json::Float(number) => Ok(Value::Scalar(Scalar::float(*number)?)),
            Json::Str(text) => Ok(Value::Scalar(Scalar::text(text.as_str()))),
            Json::Array(items) => Ok(Value::Array(ArrayValue::new(
                items
                    .iter()
                    .map(fixture_value)
                    .collect::<Result<Vec<_>, _>>()?,
            ))),
            Json::Object(items) => Ok(Value::Object(ObjectValue::new(
                items
                    .iter()
                    .map(|(key, entry)| Ok((key.as_str(), fixture_value(entry)?)))
                    .collect::<Result<Vec<_>, _>>()?,
            )?)),
            Json::BigInt(_) => Err(ValueError::new("integers must fit signed 64 bits")),
        }
    }

    fn fixture_member(record: &Json) -> Result<Member, ValueError> {
        let identity = match record.get("semantic_identity") {
            Some(item) => value_from_record(item)?,
            None => fixture_value(record.get("identity").expect("member identity"))?,
        };
        let key: Vec<String> = record
            .get("key")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().expect("member key").to_owned())
                    .collect()
            })
            .unwrap_or_default();
        Member::new(identity, key)
    }

    fn fixture_request(record: &Json, format: &str) -> Result<DocumentRequest, ValueError> {
        if format != "text" {
            return fixture_structured_request(record, format);
        }
        let boundaries = fixture_boundaries(record.get("selector").expect("selector"))?;
        let claim = Claim::new(
            Ownership::Section,
            ClaimSelector::Section(boundaries.clone()),
        )?;
        if record.get("action").and_then(Json::as_str) == Some("retire") {
            return DocumentRequest::retire(claim);
        }
        let content = SectionContent::new(
            record
                .get("marker")
                .and_then(Json::as_str)
                .unwrap_or("fixture"),
            record.get("text").and_then(Json::as_str).expect("text"),
            Some(boundaries),
        )?;
        let acquisition = Acquisition::from_str(
            record
                .get("acquisition")
                .and_then(Json::as_str)
                .unwrap_or("conflict"),
        )?;
        DocumentRequest::set(Content::Section(content), acquisition)
    }

    fn fixture_structured_request(
        record: &Json,
        format: &str,
    ) -> Result<DocumentRequest, ValueError> {
        let parts: Vec<SelectorPart> = record
            .get("selector")
            .and_then(Json::as_array)
            .expect("selector")
            .iter()
            .map(|part| match part {
                Json::Str(name) => Key::new(name.as_str()).map(SelectorPart::Key),
                item => fixture_member(item).map(SelectorPart::Member),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let selector = Selector::new(parts)?;
        let claim = Claim::new(
            Ownership::Structured,
            ClaimSelector::Structured(selector.clone()),
        )?;
        if record.get("action").and_then(Json::as_str) == Some("retire") {
            return DocumentRequest::retire(claim);
        }
        let value = match record.get("semantic_value") {
            Some(item) => value_from_record(item)?,
            None => fixture_value(record.get("value").expect("value"))?,
        };
        let placement = match record.get("placement") {
            Some(item) => {
                let position = Position::from_str(
                    item.get("position")
                        .and_then(Json::as_str)
                        .expect("position"),
                )?;
                let anchor = match item.get("anchor") {
                    Some(anchor) => Some(fixture_member(anchor)?),
                    None => None,
                };
                Some(Placement::new(position, anchor)?)
            }
            None => None,
        };
        let content = StructuredContent::new(
            DocumentFormat::from_str(format)?,
            selector,
            value,
            placement,
        )?;
        let acquisition = Acquisition::from_str(
            record
                .get("acquisition")
                .and_then(Json::as_str)
                .unwrap_or("conflict"),
        )?;
        DocumentRequest::set(Content::Structured(content), acquisition)
    }

    #[test]
    fn fixture_edits_match_expected_bytes() {
        let Some(data) = fixture("edits.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let cases = data.get("cases").and_then(Json::as_array).expect("cases");
        for case in cases {
            let name = case.get("name").and_then(Json::as_str).expect("case name");
            let format = case
                .get("format")
                .and_then(Json::as_str)
                .expect("case format");
            let mut document = match case.get("before") {
                Some(Json::Str(text)) => Some(text.as_bytes().to_vec()),
                _ => None,
            };
            let mut ownership: Vec<OwnedSelection> = Vec::new();
            let mut provenance = DocumentProvenance::default();
            let mut baselines: Vec<(Claim, Option<SelectionSnapshot>)> = Vec::new();
            for step in case.get("steps").and_then(Json::as_array).expect("steps") {
                let observed = match step.get("observed") {
                    Some(Json::Str(text)) => Some(text.as_bytes().to_vec()),
                    Some(Json::Null) => None,
                    _ => document.clone(),
                };
                let records: Vec<&Json> = match step.get("requests") {
                    Some(Json::Array(items)) => items.iter().collect(),
                    _ => vec![step],
                };
                let changes: Vec<DocumentRequest> = records
                    .iter()
                    .map(|record| {
                        fixture_request(record, format)
                            .unwrap_or_else(|error| panic!("{name}: {error}"))
                    })
                    .collect();
                let result = edit_document(observed, &changes, &ownership, &provenance)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                let expected = match step.get("expected") {
                    Some(Json::Str(text)) => Some(text.as_bytes().to_vec()),
                    _ => None,
                };
                assert_eq!(
                    result.after(),
                    expected.as_deref(),
                    "case {name} produced the wrong document"
                );
                if let Some(items) = step.get("statuses").and_then(Json::as_array) {
                    let wanted: Vec<&str> = items
                        .iter()
                        .map(|item| item.as_str().expect("status"))
                        .collect();
                    assert_eq!(
                        result
                            .observations()
                            .iter()
                            .map(|item| item.status().as_str())
                            .collect::<Vec<_>>(),
                        wanted,
                        "case {name} statuses"
                    );
                } else {
                    let status = step.get("status").and_then(Json::as_str).expect("status");
                    let actual: Vec<&str> = result
                        .observations()
                        .iter()
                        .map(|item| item.status().as_str())
                        .collect();
                    assert_eq!(actual, vec![status], "case {name} status");
                }
                for owned in result.ownership() {
                    if owned.disposition() == Disposition::TakenOver {
                        let known = baselines
                            .iter()
                            .find(|(claim, _)| claim == owned.claim())
                            .map(|(_, value)| value.clone());
                        match known {
                            Some(baseline) => assert_eq!(
                                owned.baseline(),
                                baseline.as_ref(),
                                "case {name} baseline"
                            ),
                            None => {
                                baselines.push((owned.claim().clone(), owned.baseline().cloned()));
                            }
                        }
                    } else {
                        assert!(
                            owned.baseline().is_none(),
                            "case {name} only takeover owns a baseline"
                        );
                    }
                }
                if !result.applicable() {
                    assert_eq!(
                        result.ownership(),
                        &ownership,
                        "case {name} keeps ownership on conflict"
                    );
                    assert_eq!(
                        result.provenance(),
                        &provenance,
                        "case {name} keeps provenance on conflict"
                    );
                }
                document = after_of(&result);
                ownership = result.ownership().to_vec();
                provenance = result.provenance().clone();
            }
        }
        for case in data
            .get("invalid")
            .and_then(Json::as_array)
            .expect("invalid")
        {
            let name = case.get("name").and_then(Json::as_str).expect("name");
            let format = case.get("format").and_then(Json::as_str).expect("format");
            let document = case
                .get("document")
                .and_then(Json::as_str)
                .expect("document")
                .as_bytes()
                .to_vec();
            let mut requests: Vec<DocumentRequest> = Vec::new();
            for selector in case
                .get("selectors")
                .and_then(Json::as_array)
                .expect("selectors")
            {
                let request = if format == "text" {
                    let boundaries = fixture_boundaries(selector)
                        .unwrap_or_else(|error| panic!("{name}: {error}"));
                    let content = SectionContent::new("fixture", "body", Some(boundaries))
                        .unwrap_or_else(|error| panic!("{name}: {error}"));
                    DocumentRequest::set(Content::Section(content), Acquisition::Conflict)
                        .unwrap_or_else(|error| panic!("{name}: {error}"))
                } else {
                    let record = Json::Object(vec![
                        (String::from("action"), Json::Str(String::from("set"))),
                        (String::from("selector"), selector.clone()),
                        (String::from("value"), Json::Int(1)),
                    ]);
                    fixture_structured_request(&record, format)
                        .unwrap_or_else(|error| panic!("{name}: {error}"))
                };
                requests.push(request);
            }
            let error = edit_document(
                Some(document),
                &requests,
                &[],
                &DocumentProvenance::default(),
            )
            .expect_err(&format!("case {name} must be rejected"));
            let message = match name {
                "nested-custom-sections" => "nested or repeated section boundaries",
                "duplicate-json-key" | "duplicate-jsonc-key" => "duplicate JSON object key",
                "duplicate-toml-key" => "Key \"x\" already exists. at line 2 col 0",
                "duplicate-array-identity" => "duplicate or ambiguous array member identities",
                "overlapping-paths" => "document claims overlap",
                other => panic!("unexpected invalid case {other}"),
            };
            assert_eq!(error.message(), message, "case {name} rejection");
        }
    }

    #[test]
    fn sections_preserve_foreign_bytes() {
        for newline in ["\n", "\r\n", "\r"] {
            for prefix in ["", "foreign", "foreign\n", "foreign\n\n", "foreign\n\n\n"] {
                for body in ["", "hello", "hello\n", "hello\n\n", " a \n b \r\n"] {
                    let raw =
                        format!("header{newline}{}", prefix.replace('\n', newline)).into_bytes();
                    let content = SectionContent::new("guide", body, None).expect("section");
                    let installed = set_section(Some(raw.clone()), &content, Acquisition::Conflict);
                    assert!(installed.applicable(), "body {body:?}");
                    assert!(
                        installed
                            .after()
                            .is_some_and(|after| after.starts_with(&raw)),
                        "prefix {prefix:?}"
                    );
                    assert_eq!(
                        retire_all(&installed, after_of(&installed)).after(),
                        Some(raw.as_slice()),
                        "prefix {prefix:?} body {body:?}"
                    );
                    assert_eq!(
                        installed
                            .ownership()
                            .first()
                            .map(OwnedSelection::disposition),
                        Some(Disposition::Created)
                    );
                }
            }
        }
    }

    #[test]
    fn section_retirement_preserves_adjacent_sections_and_suffix() {
        for newline in ["\n", "\r\n", "\r"] {
            for retired in [vec!["b"], vec!["a", "b"], vec!["b", "a"]] {
                let prefix = format!("Header{newline}").into_bytes();
                let suffix = format!("Üser suffix{newline}").into_bytes();
                let installed = edit_document(
                    Some(prefix.clone()),
                    &["a", "b"]
                        .iter()
                        .map(|name| {
                            let content = SectionContent::new(*name, *name, None).expect("section");
                            DocumentRequest::set(Content::Section(content), Acquisition::Conflict)
                                .expect("request")
                        })
                        .collect::<Vec<_>>(),
                    &[],
                    &DocumentProvenance::default(),
                )
                .expect("edit");
                let after = after_of(&installed).expect("after");
                let doubled = format!("<!-- flyrail:a:end -->{newline}{newline}").into_bytes();
                let single = format!("<!-- flyrail:a:end -->{newline}").into_bytes();
                let observed = [replace_all(&after, &doubled, &single), suffix.clone()].concat();
                let requests: Vec<DocumentRequest> = retired
                    .iter()
                    .map(|name| {
                        let content = SectionContent::new(*name, *name, None).expect("section");
                        DocumentRequest::retire(
                            Claim::new(
                                Ownership::Section,
                                ClaimSelector::Section(content.selector().expect("selector")),
                            )
                            .expect("claim"),
                        )
                        .expect("retire request")
                    })
                    .collect();
                let result = edit_document(
                    Some(observed.clone()),
                    &requests,
                    installed.ownership(),
                    installed.provenance(),
                )
                .expect("edit");
                assert!(result.applicable());
                assert_eq!(
                    result
                        .observations()
                        .iter()
                        .map(EditObservation::status)
                        .collect::<Vec<_>>(),
                    vec![EditStatus::Removed; retired.len()]
                );
                if retired.len() == 2 {
                    assert_eq!(
                        result.after(),
                        Some([prefix.as_slice(), suffix.as_slice()].concat().as_slice())
                    );
                    assert_eq!(result.ownership(), [] as [OwnedSelection; 0]);
                } else {
                    let first = "<!-- flyrail:a:start -->\na\n<!-- flyrail:a:end -->\n"
                        .to_string()
                        .replace('\n', newline);
                    assert_eq!(
                        result.after(),
                        Some(
                            [
                                prefix.as_slice(),
                                newline.as_bytes(),
                                first.as_bytes(),
                                suffix.as_slice(),
                            ]
                            .concat()
                            .as_slice()
                        )
                    );
                    let current = update_section(
                        &result,
                        &SectionContent::new("a", "a", None).expect("section"),
                        after_of(&result),
                    );
                    assert_eq!(
                        current.observations().first().map(EditObservation::status),
                        Some(EditStatus::Current)
                    );
                    assert_eq!(current.after(), result.after());
                }
            }
        }
    }

    #[test]
    fn section_retirement_preserves_an_unowned_standard_neighbor() {
        for newline in ["\n", "\r\n", "\r"] {
            let foreign =
                "<!-- flyrail:foreign:start -->\nUser text\n<!-- flyrail:foreign:end -->\n"
                    .replace('\n', newline)
                    .into_bytes();
            let installed = set_section(
                Some(foreign.clone()),
                &SectionContent::new("owned", "generated", None).expect("section"),
                Acquisition::Conflict,
            );
            let after = after_of(&installed).expect("after");
            let doubled = format!("<!-- flyrail:foreign:end -->{newline}{newline}").into_bytes();
            let single = format!("<!-- flyrail:foreign:end -->{newline}").into_bytes();
            let observed = replace_all(&after, &doubled, &single);
            let result = retire_all(&installed, Some(observed));
            assert!(result.applicable());
            assert_eq!(
                result.after(),
                Some(foreign.as_slice()),
                "newline {newline:?}"
            );
        }
    }

    #[test]
    fn section_bom_takeover_update_rename_and_restore() {
        for prefix_newline in [b"\n".as_slice(), b"\r\n", b"\r"] {
            let pair =
                SectionBoundaries::new("<!-- splashdown:start -->", "<!-- splashdown:end -->")
                    .expect("pair");
            let prefix = [b"\xef\xbb\xbfUser", prefix_newline, prefix_newline].concat();
            let raw = [
                prefix.as_slice(),
                b"<!-- splashdown:start -->\r\n  old \r\n\r\n<!-- splashdown:end -->\r\nTAIL",
            ]
            .concat();
            let content =
                SectionContent::new("old-attribution", "new\n", Some(pair.clone())).expect("s");
            let conflict = set_section(Some(raw.clone()), &content, Acquisition::Conflict);
            assert_eq!(conflict.after(), Some(raw.as_slice()));
            assert_eq!(
                conflict.observations().first().map(EditObservation::status),
                Some(EditStatus::Conflict)
            );
            let first = set_section(Some(raw.clone()), &content, Acquisition::Takeover);
            assert_eq!(
                first.after(),
                Some(
                    [
                        prefix.as_slice(),
                        b"<!-- splashdown:start -->\r\nnew\r\n<!-- splashdown:end -->\r\nTAIL",
                    ]
                    .concat()
                    .as_slice()
                )
            );
            let renamed = SectionContent::new("renamed", "newer\n", Some(pair.clone())).expect("s");
            let second = update_section(&first, &renamed, after_of(&first));
            assert_eq!(
                second
                    .ownership()
                    .first()
                    .and_then(OwnedSelection::baseline),
                first.ownership().first().and_then(OwnedSelection::baseline)
            );
            assert_eq!(
                second.observations().first().map(EditObservation::status),
                Some(EditStatus::Updated)
            );
            assert_eq!(
                retire_all(&second, after_of(&second)).after(),
                Some(raw.as_slice())
            );
            let repeated = update_section(&second, &renamed, after_of(&second));
            assert_eq!(
                repeated.observations().first().map(EditObservation::status),
                Some(EditStatus::Current)
            );
        }
    }

    #[test]
    fn modified_section_retains_baseline() {
        let original =
            b"Foreign\n<!-- flyrail:x:start -->\r\nold\r\n<!-- flyrail:x:end -->\r\n".to_vec();
        let content = SectionContent::new("x", "new\n", None).expect("section");
        let first = set_section(Some(original), &content, Acquisition::Takeover);
        let after = after_of(&first).expect("after");
        for change in ["body", "space", "missing", "newline"] {
            let data = match change {
                "newline" => replace_all(&after, b"\r\n", b"\n"),
                "missing" => b"foreign\n".to_vec(),
                "body" => replace_all(&after, b"new", b"edited"),
                _ => replace_all(&after, b"new", b"new "),
            };
            let updated = update_section(&first, &content, Some(data.clone()));
            assert_eq!(updated.after(), Some(data.as_slice()), "update {change}");
            assert_eq!(updated.ownership(), first.ownership(), "update {change}");
            assert_eq!(updated.provenance(), first.provenance(), "update {change}");
            assert_eq!(
                updated.observations().first().map(EditObservation::status),
                Some(EditStatus::Modified),
                "update {change}"
            );
            let removed = retire_all(&first, Some(data.clone()));
            assert_eq!(removed.after(), Some(data.as_slice()), "remove {change}");
            assert_eq!(removed.ownership(), first.ownership(), "remove {change}");
            assert_eq!(removed.provenance(), first.provenance(), "remove {change}");
            assert_eq!(
                removed.observations().first().map(EditObservation::status),
                Some(EditStatus::Modified),
                "remove {change}"
            );
        }
    }

    #[test]
    fn adoption_is_exact_and_removes_without_a_baseline() {
        for prefix_newline in ["\n", "\r\n", "\r"] {
            for newline in ["\n", "\r\n", "\r"] {
                let pair = SectionBoundaries::new("START", "END").expect("pair");
                let content =
                    SectionContent::new("x", "generated\n", Some(pair.clone())).expect("s");
                let prefix = format!("before{prefix_newline}").into_bytes();
                let suffix = format!("after{prefix_newline}").into_bytes();
                let raw = [
                    prefix.as_slice(),
                    "START\ngenerated\nEND\n"
                        .to_string()
                        .replace('\n', newline)
                        .as_bytes(),
                    suffix.as_slice(),
                ]
                .concat();
                let adopted = set_section(Some(raw.clone()), &content, Acquisition::Adopt);
                assert_eq!(adopted.after(), Some(raw.as_slice()));
                assert_eq!(
                    adopted.ownership().first().map(OwnedSelection::disposition),
                    Some(Disposition::Adopted)
                );
                assert!(
                    adopted
                        .ownership()
                        .first()
                        .and_then(OwnedSelection::baseline)
                        .is_none()
                );
                let repeated = update_section(&adopted, &content, after_of(&adopted));
                assert_eq!(
                    repeated.observations().first().map(EditObservation::status),
                    Some(EditStatus::Current)
                );
                assert_eq!(repeated.after(), Some(raw.as_slice()));
                assert_eq!(repeated.ownership(), adopted.ownership());
                let removed = retire_all(&repeated, after_of(&repeated));
                assert_eq!(
                    removed.after(),
                    Some([prefix.as_slice(), suffix.as_slice()].concat().as_slice())
                );
                let alternate = if newline == "\n" { "\r\n" } else { "\n" };
                let variants = [
                    replace_all(&raw, b"generated", b"generated "),
                    replace_all(&raw, b"generated", format!("generated{newline}").as_bytes()),
                    replace_all(&raw, b"generated", b"GENERATED"),
                    replace_all(
                        &raw,
                        format!("generated{newline}").as_bytes(),
                        format!("generated{alternate}").as_bytes(),
                    ),
                    replace_all(
                        &raw,
                        format!("END{newline}").as_bytes(),
                        format!("END{alternate}").as_bytes(),
                    ),
                ];
                for variant in variants {
                    let result = set_section(Some(variant.clone()), &content, Acquisition::Adopt);
                    assert_eq!(
                        result.observations().first().map(EditObservation::status),
                        Some(EditStatus::Conflict),
                        "variant {variant:?}"
                    );
                    assert_eq!(result.after(), Some(variant.as_slice()));
                }
            }
        }
    }

    #[test]
    fn sections_use_their_own_newlines_and_new_sections_use_the_document() {
        let a = SectionContent::new(
            "a",
            "new",
            Some(SectionBoundaries::new("A", "END A").expect("p")),
        )
        .expect("section a");
        let b = SectionContent::new(
            "b",
            "keep",
            Some(SectionBoundaries::new("B", "END B").expect("p")),
        )
        .expect("section b");
        let c = SectionContent::new("c", "created", None).expect("section c");
        let raw = b"Header\nA\r\nold\r\nEND A\r\nB\rkeep\rEND B\rFooter\n".to_vec();
        let requests = [
            DocumentRequest::set(Content::Section(a), Acquisition::Takeover).expect("a"),
            DocumentRequest::set(Content::Section(b), Acquisition::Adopt).expect("b"),
            DocumentRequest::set(Content::Section(c), Acquisition::Conflict).expect("c"),
        ];
        let result =
            edit_document(Some(raw), &requests, &[], &DocumentProvenance::default()).expect("edit");
        assert_eq!(
            result
                .observations()
                .iter()
                .map(|item| item.status().as_str())
                .collect::<Vec<_>>(),
            ["taken-over", "adopted", "created"]
        );
        assert_eq!(
            result.after(),
            Some(
                b"Header\nA\r\nnew\r\nEND A\r\nB\rkeep\rEND B\rFooter\n\n\
                  <!-- flyrail:c:start -->\ncreated\n<!-- flyrail:c:end -->\n"
                    .as_slice()
            )
        );
        let removed = retire_all(&result, after_of(&result));
        assert_eq!(
            removed.after(),
            Some(b"Header\nA\r\nold\r\nEND A\r\nFooter\n".as_slice())
        );
    }

    #[test]
    fn malformed_or_intersecting_sections_are_rejected() {
        let requests = || {
            vec![
                DocumentRequest::set(
                    Content::Section(
                        SectionContent::new(
                            "x",
                            "new",
                            Some(SectionBoundaries::new("START", "END").expect("pair")),
                        )
                        .expect("section x"),
                    ),
                    Acquisition::Conflict,
                )
                .expect("request x"),
                DocumentRequest::set(
                    Content::Section(
                        SectionContent::new(
                            "y",
                            "other",
                            Some(SectionBoundaries::new("OTHER", "STOP").expect("pair")),
                        )
                        .expect("section y"),
                    ),
                    Acquisition::Conflict,
                )
                .expect("request y"),
            ]
        };
        for raw in [
            b"START\n".as_slice(),
            b"END\n",
            b"END\nSTART\n",
            b"START\nx\nSTART\nEND\n",
            b"START\nEND\nSTART\nEND\n",
            b"prefix START\nEND\n",
            b"START\nEND suffix\n",
            b"START\nOTHER\nEND\nSTOP\n",
            b"START\nOTHER\nSTOP\nEND\n",
            b"<!-- flyrail:bad marker:start -->\n",
        ] {
            let result = edit_document(
                Some(raw.to_vec()),
                &requests(),
                &[],
                &DocumentProvenance::default(),
            );
            assert!(result.is_err(), "raw {raw:?} must be rejected");
        }
    }

    #[test]
    fn unrequested_native_sections_are_also_checked_for_intersection() {
        let raw = b"<!-- flyrail:foreign:start -->\nSTART\n<!-- flyrail:foreign:end -->\nEND\n";
        let content = SectionContent::new(
            "x",
            "new",
            Some(SectionBoundaries::new("START", "END").expect("pair")),
        )
        .expect("section");
        let error = edit_document(
            Some(raw.to_vec()),
            &[
                DocumentRequest::set(Content::Section(content), Acquisition::Takeover)
                    .expect("request"),
            ],
            &[],
            &DocumentProvenance::default(),
        )
        .expect_err("sections must not intersect");
        assert_eq!(error.message(), "nested or repeated section boundaries");
    }

    #[test]
    fn sections_multiowner_creation_and_separators() {
        let a = set_section(
            None,
            &SectionContent::new("a", "A", None).expect("a"),
            Acquisition::Conflict,
        );
        let b = update_section(
            &a,
            &SectionContent::new("b", "B", None).expect("b"),
            after_of(&a),
        );
        let c = edit_document(
            after_of(&b),
            &[
                DocumentRequest::retire(a.ownership().first().expect("owned").claim().clone())
                    .expect("retire"),
            ],
            b.ownership(),
            b.provenance(),
        )
        .expect("edit");
        assert_eq!(
            c.after(),
            Some(b"\n<!-- flyrail:b:start -->\nB\n<!-- flyrail:b:end -->\n".as_slice())
        );
        assert_eq!(c.provenance().empty_document(), Some(b"".as_slice()));
        assert_eq!(retire_all(&c, after_of(&c)).after(), None);
        let foreign_data = [b"User\n".as_slice(), &after_of(&a).expect("a after")].concat();
        let foreign = update_section(
            &a,
            &SectionContent::new("b", "B", None).expect("b"),
            Some(foreign_data),
        );
        let final_edit = retire_all(&foreign, after_of(&foreign));
        assert_eq!(final_edit.after(), Some(b"User\n".as_slice()));
        assert_eq!(final_edit.provenance(), &DocumentProvenance::default());
    }

    #[test]
    fn separator_is_retained_when_no_longer_adjacent() {
        let first = set_section(
            Some(b"foreign".to_vec()),
            &SectionContent::new("x", "body", None).expect("section"),
            Acquisition::Conflict,
        );
        let after = after_of(&first).expect("after");
        let data = replace_all(&after, b"\n\n<!--", b"\n\nUSER\n<!--");
        let result = retire_all(&first, Some(data));
        assert_eq!(result.after(), Some(b"foreign\n\nUSER\n".as_slice()));
    }

    #[test]
    fn section_resource_conflicts_are_atomic() {
        let a = set_section(
            None,
            &SectionContent::new("a", "A", None).expect("a"),
            Acquisition::Conflict,
        );
        let after = after_of(&a).expect("after");
        let data = replace_all(&after, b"\nA\n", b"\nUSER\n");
        let requests = [
            DocumentRequest::retire(a.ownership().first().expect("owned").claim().clone())
                .expect("retire"),
            DocumentRequest::set(
                Content::Section(SectionContent::new("b", "B", None).expect("b")),
                Acquisition::Conflict,
            )
            .expect("request b"),
        ];
        let result = edit_document(Some(data.clone()), &requests, a.ownership(), a.provenance())
            .expect("edit");
        assert_eq!(result.after(), Some(data.as_slice()));
        assert_eq!(result.ownership(), a.ownership());
        assert!(!result.applicable());
    }

    #[test]
    fn conflicting_claims_are_rejected_before_changes() {
        let structured = |format: DocumentFormat, value: Value, parts: &[&str]| {
            let keys: Vec<SelectorPart> = parts
                .iter()
                .map(|name| SelectorPart::Key(Key::new(*name).expect("key")))
                .collect();
            StructuredContent::new(format, Selector::new(keys).expect("selector"), value, None)
                .expect("structured content")
        };
        let parent = structured(
            DocumentFormat::Json,
            Value::Object(ObjectValue::new::<String>([]).expect("object")),
            &["mcp"],
        );
        let child = structured(
            DocumentFormat::Json,
            Value::Scalar(Scalar::integer(1)),
            &["mcp", "x"],
        );
        let other = structured(
            DocumentFormat::Jsonc,
            Value::Scalar(Scalar::integer(2)),
            &["y"],
        );
        let rows = [
            (
                vec![
                    DocumentRequest::set(Content::Structured(parent), Acquisition::Conflict)
                        .expect("parent"),
                    DocumentRequest::set(Content::Structured(child.clone()), Acquisition::Conflict)
                        .expect("child"),
                ],
                "document claims overlap",
            ),
            (
                vec![
                    DocumentRequest::set(Content::Structured(child.clone()), Acquisition::Conflict)
                        .expect("child"),
                    DocumentRequest::set(Content::Structured(child.clone()), Acquisition::Conflict)
                        .expect("child"),
                ],
                "duplicate document claim",
            ),
            (
                vec![
                    DocumentRequest::set(Content::Structured(child.clone()), Acquisition::Conflict)
                        .expect("child"),
                    DocumentRequest::set(
                        Content::Section(SectionContent::new("x", "body", None).expect("s")),
                        Acquisition::Conflict,
                    )
                    .expect("section"),
                ],
                "document claims overlap",
            ),
            (
                vec![
                    DocumentRequest::set(Content::Structured(child), Acquisition::Conflict)
                        .expect("child"),
                    DocumentRequest::set(Content::Structured(other), Acquisition::Conflict)
                        .expect("other"),
                ],
                "one document must use one editor format",
            ),
        ];
        for (requests, message) in rows {
            let error = edit_document(None, &requests, &[], &DocumentProvenance::default())
                .expect_err("conflicting requests");
            assert_eq!(error.message(), message);
        }
        let section_a = SectionContent::new(
            "a",
            "A",
            Some(SectionBoundaries::new("START", "END").expect("pair")),
        )
        .expect("section a");
        let section_b = SectionContent::new(
            "b",
            "B",
            Some(SectionBoundaries::new("START", "STOP").expect("pair")),
        )
        .expect("section b");
        let error = edit_document(
            None,
            &[
                DocumentRequest::set(Content::Section(section_a), Acquisition::Conflict)
                    .expect("a"),
                DocumentRequest::set(Content::Section(section_b), Acquisition::Conflict)
                    .expect("b"),
            ],
            &[],
            &DocumentProvenance::default(),
        )
        .expect_err("section overlap");
        assert_eq!(error.message(), "document claims overlap");
    }

    #[test]
    fn document_requests_validate_like_python() {
        let file_claim = Claim::new(Ownership::File, ClaimSelector::Whole).expect("file claim");
        let non_document = DocumentRequest::new(file_claim, None, Acquisition::Conflict);
        assert_eq!(
            non_document.unwrap_err().message(),
            "document requests require section or structured claims"
        );
        let wrong_content = DocumentRequest::set(
            Content::File(FileContent::new(b"x".to_vec(), FileMode::Readable)),
            Acquisition::Conflict,
        );
        assert_eq!(
            wrong_content.unwrap_err().message(),
            "document content must be a section or structured selection"
        );
        let mismatched = DocumentRequest::new(
            marker_claim("a"),
            Some(Content::Section(
                SectionContent::new("b", "text", None).expect("s"),
            )),
            Acquisition::Conflict,
        );
        assert_eq!(
            mismatched.unwrap_err().message(),
            "requested content must address its physical claim"
        );
        let acquiring_retirement =
            DocumentRequest::new(marker_claim("a"), None, Acquisition::Adopt);
        assert_eq!(
            acquiring_retirement.unwrap_err().message(),
            "retirement does not acquire ownership"
        );
    }

    #[test]
    fn no_requests_preserve_documents_and_reject_bad_bytes() {
        let unchanged = edit_document(
            Some(b"unchanged".to_vec()),
            &[],
            &[],
            &DocumentProvenance::default(),
        )
        .expect("edit");
        assert_eq!(unchanged.after(), Some(b"unchanged".as_slice()));
        assert_eq!(
            edit_document(None, &[], &[], &DocumentProvenance::default())
                .expect("edit")
                .after(),
            None
        );
        assert_eq!(
            edit_document(
                Some(b"\xff".to_vec()),
                &[],
                &[],
                &DocumentProvenance::default()
            )
            .unwrap_err()
            .message(),
            "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"
        );
        assert_eq!(
            edit_document(
                Some(b"\xe0\xbf\x00".to_vec()),
                &[],
                &[],
                &DocumentProvenance::default()
            )
            .unwrap_err()
            .message(),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        let nul = edit_document(
            Some(b"bad\0".to_vec()),
            &[],
            &[],
            &DocumentProvenance::default(),
        );
        assert_eq!(nul.unwrap_err().message(), "documents cannot contain NUL");
        let section = set_section(
            None,
            &SectionContent::new("x", "body", None).expect("section"),
            Acquisition::Conflict,
        );
        let observed =
            edit_document(None, &[], section.ownership(), section.provenance()).expect("edit");
        assert_eq!(observed.after(), None);
        assert_eq!(observed.ownership(), section.ownership());
        assert_eq!(observed.provenance(), section.provenance());
    }

    #[test]
    fn duplicate_claims_and_retirement_edges_match_python() {
        let section = set_section(
            None,
            &SectionContent::new("x", "body", None).expect("section"),
            Acquisition::Conflict,
        );
        let owned = section.ownership().first().expect("owned");
        let duplicate = edit_document(
            after_of(&section),
            &[],
            &[owned.clone(), owned.clone()],
            &DocumentProvenance::default(),
        );
        assert_eq!(duplicate.unwrap_err().message(), "duplicate document claim");
        let structured_claim = Claim::new(
            Ownership::Structured,
            ClaimSelector::Structured(
                Selector::new([SelectorPart::Key(Key::new("x").expect("key"))]).expect("selector"),
            ),
        )
        .expect("structured claim");
        let structured_retire = edit_document(
            None,
            &[DocumentRequest::retire(structured_claim).expect("retire")],
            &[],
            &DocumentProvenance::default(),
        );
        assert_eq!(
            structured_retire.unwrap_err().message(),
            "retirement requires existing ownership and its document format"
        );
        let unowned = edit_document(
            Some(b"foreign".to_vec()),
            &[DocumentRequest::retire(marker_claim("x")).expect("retire")],
            &[],
            &DocumentProvenance::default(),
        )
        .expect("edit");
        assert_eq!(
            unowned.observations().first().map(EditObservation::status),
            Some(EditStatus::Conflict)
        );
        assert_eq!(unowned.after(), Some(b"foreign".as_slice()));
    }

    fn key(name: &str) -> SelectorPart {
        SelectorPart::Key(Key::new(name).expect("key"))
    }

    fn member_of(identity: Value, names: &[&str]) -> Member {
        Member::new(identity, names.iter().map(ToString::to_string)).expect("member")
    }

    fn member_part(identity: Value, names: &[&str]) -> SelectorPart {
        SelectorPart::Member(member_of(identity, names))
    }

    fn placement(position: Position) -> Placement {
        Placement::new(position, None).expect("placement")
    }

    fn anchored(position: Position, anchor: Member) -> Placement {
        Placement::new(position, Some(anchor)).expect("anchored placement")
    }

    fn structured_keys(
        format: DocumentFormat,
        value: Value,
        keys: &[&str],
        member: Option<Member>,
        placement: Option<Placement>,
    ) -> StructuredContent {
        let mut parts: Vec<SelectorPart> = keys.iter().map(|name| key(name)).collect();
        if let Some(member) = member {
            parts.push(SelectorPart::Member(member));
        }
        structured(format, value, parts, placement)
    }

    fn set_structured_expect_err(
        data: Option<Vec<u8>>,
        content: &StructuredContent,
        acquisition: Acquisition,
    ) -> ValueError {
        let request = DocumentRequest::set(Content::Structured(content.clone()), acquisition)
            .expect("structured request");
        edit_document(data, &[request], &[], &DocumentProvenance::default())
            .expect_err("structured edit must fail")
    }

    fn obj(entries: &[(&str, Value)]) -> Value {
        Value::Object(
            ObjectValue::new(
                entries
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone())),
            )
            .expect("object"),
        )
    }

    fn arr(items: &[Value]) -> Value {
        Value::Array(ArrayValue::new(items.iter().cloned()))
    }

    fn floatv(value: f64) -> Value {
        Value::Scalar(Scalar::float(value).expect("finite float"))
    }

    fn structured(
        format: DocumentFormat,
        value: Value,
        parts: Vec<SelectorPart>,
        placement: Option<Placement>,
    ) -> StructuredContent {
        StructuredContent::new(
            format,
            Selector::new(parts).expect("selector"),
            value,
            placement,
        )
        .expect("structured content")
    }

    fn set_structured(
        data: Option<Vec<u8>>,
        content: &StructuredContent,
        acquisition: Acquisition,
    ) -> DocumentEdit {
        let request = DocumentRequest::set(Content::Structured(content.clone()), acquisition)
            .expect("structured request");
        edit_document(data, &[request], &[], &DocumentProvenance::default())
            .expect("structured edit")
    }

    #[test]
    fn toml_created_values_follow_document_newline() {
        let original = b"foreign = 1\r\n".to_vec();
        let content = structured(
            DocumentFormat::Toml,
            Value::from(2),
            vec![SelectorPart::Key(Key::new("x").expect("key"))],
            None,
        );
        let installed = set_structured(Some(original.clone()), &content, Acquisition::Conflict);
        assert_eq!(
            installed.after(),
            Some(b"foreign = 1\r\nx = 2\r\n".as_slice())
        );
        assert_eq!(
            retire_all(&installed, installed.after().map(<[u8]>::to_vec)).after(),
            Some(original.as_slice())
        );
    }

    fn deep_object_value(depth: usize) -> String {
        let mut value = String::from("1");
        for _ in 0..depth {
            value = format!("{{\"a\":{value}}}");
        }
        value
    }

    fn deep_array_value(depth: usize) -> String {
        let mut value = String::from("1");
        for _ in 0..depth {
            value = format!("[{value}]");
        }
        value
    }

    fn deep_object(depth: usize) -> Value {
        let mut value = Value::from(1);
        for _ in 0..depth {
            value = obj(&[("a", value)]);
        }
        value
    }

    fn deep_array(depth: usize) -> Value {
        let mut value = Value::from(1);
        for _ in 0..depth {
            value = arr(&[value]);
        }
        value
    }

    fn deep_alternating_value(depth: usize) -> String {
        let mut value = String::from("1");
        for level in 0..depth {
            value = if level % 2 == 0 {
                format!("{{\"a\":{value}}}")
            } else {
                format!("[{value}]")
            };
        }
        value
    }

    fn update_structured(
        edit: &DocumentEdit,
        content: &StructuredContent,
        data: Option<Vec<u8>>,
    ) -> DocumentEdit {
        let request =
            DocumentRequest::set(Content::Structured(content.clone()), Acquisition::Conflict)
                .expect("structured request");
        edit_document(
            data.or_else(|| after_of(edit)),
            &[request],
            edit.ownership(),
            edit.provenance(),
        )
        .expect("structured update")
    }

    #[test]
    fn structured_creates_nested_and_removes() {
        for format in [
            DocumentFormat::Json,
            DocumentFormat::Jsonc,
            DocumentFormat::Toml,
        ] {
            let content = structured(
                format,
                Value::from(1),
                vec![key("mcp"), key("server"), key("key")],
                None,
            );
            for raw in [None, Some(Vec::new())] {
                if raw.as_ref().is_some_and(Vec::is_empty) && format != DocumentFormat::Toml {
                    let request = DocumentRequest::set(
                        Content::Structured(content.clone()),
                        Acquisition::Conflict,
                    )
                    .expect("request");
                    let error = edit_document(raw, &[request], &[], &DocumentProvenance::default())
                        .expect_err("empty documents are not structured roots");
                    assert_eq!(error.message(), "missing JSON value");
                    continue;
                }
                let result = set_structured(raw.clone(), &content, Acquisition::Conflict);
                assert!(result.applicable());
                assert!(matches!(
                    result.ownership().first().expect("owned").installed().value(),
                    SnapshotValue::Typed(value) if *value == Value::Scalar(Scalar::integer(1))
                ));
                assert_eq!(result.provenance().containers().len(), 2);
                let removed = retire_all(&result, after_of(&result));
                assert_eq!(removed.after(), raw.as_deref());
            }
        }
    }

    #[test]
    fn foreign_structured_syntax_roundtrips() {
        let rows = [
            (DocumentFormat::Json, b"{\"foreign\":1}".as_slice()),
            (
                DocumentFormat::Jsonc,
                b"\xef\xbb\xbf{\r\n // top\r\n \"foreign\" : 1,\r\n}\r\n".as_slice(),
            ),
            (
                DocumentFormat::Toml,
                b"\xef\xbb\xbf# top\r\nforeign = 0x12 # keep\r\n".as_slice(),
            ),
        ];
        for (format, raw) in rows {
            let first = set_structured(
                Some(raw.to_vec()),
                &structured(
                    format,
                    obj(&[
                        ("command", Value::from("runner")),
                        ("args", arr(&[Value::from("one"), Value::from("two")])),
                    ]),
                    vec![key("mcp"), key("server")],
                    None,
                ),
                Acquisition::Conflict,
            );
            assert!(first.applicable());
            let changed = update_structured(
                &first,
                &structured(
                    format,
                    obj(&[
                        ("command", Value::from("runner")),
                        ("args", arr(&[Value::from("three")])),
                    ]),
                    vec![key("mcp"), key("server")],
                    None,
                ),
                None,
            );
            assert_eq!(
                changed.observations().first().map(EditObservation::status),
                Some(EditStatus::Updated)
            );
            let removed = retire_all(&changed, after_of(&changed));
            assert_eq!(removed.after(), Some(raw));
        }
    }

    #[test]
    fn takeover_keeps_first_syntax_baseline_across_updates() {
        let rows = [
            (
                DocumentFormat::Json,
                b"{\"foreign\":true,\"mcp\":{\"server\":{\"command\":\"old\"}}}".as_slice(),
            ),
            (
                DocumentFormat::Jsonc,
                b"{ /* top */ \"mcp\": { \"server\": {\"command\" : \"old\" /* owned */} }, \"foreign\":true }".as_slice(),
            ),
            (
                DocumentFormat::Toml,
                b"foreign = true\n[mcp.server] # header\ncommand = 'old' # owned\n".as_slice(),
            ),
        ];
        for (format, raw) in rows {
            let content = structured(
                format,
                obj(&[("command", Value::from("new"))]),
                vec![key("mcp"), key("server")],
                None,
            );
            let conflict = set_structured(Some(raw.to_vec()), &content, Acquisition::Conflict);
            assert_eq!(
                conflict.observations().first().map(EditObservation::status),
                Some(EditStatus::Conflict)
            );
            let first = set_structured(Some(raw.to_vec()), &content, Acquisition::Takeover);
            assert!(first.applicable());
            let second = update_structured(
                &first,
                &structured(
                    format,
                    obj(&[("command", Value::from("newer"))]),
                    vec![key("mcp"), key("server")],
                    None,
                ),
                None,
            );
            let third = update_structured(&second, &content, None);
            assert_eq!(
                third.ownership().first().expect("owned").baseline(),
                first.ownership().first().expect("owned").baseline()
            );
            let removed = retire_all(&third, after_of(&third));
            assert_eq!(removed.after(), Some(raw));
            let removed_again = retire_all(&removed, after_of(&removed));
            assert_eq!(removed_again.ownership(), [] as [OwnedSelection; 0]);
        }
    }

    #[test]
    fn structured_exact_adoption_has_removal_disposition() {
        for format in [
            DocumentFormat::Json,
            DocumentFormat::Jsonc,
            DocumentFormat::Toml,
        ] {
            let generated = set_structured(
                None,
                &structured(
                    format,
                    Value::from(true),
                    vec![key("mcp"), key("enabled")],
                    None,
                ),
                Acquisition::Conflict,
            );
            let adopted = set_structured(
                after_of(&generated),
                &structured(
                    format,
                    Value::from(true),
                    vec![key("mcp"), key("enabled")],
                    None,
                ),
                Acquisition::Adopt,
            );
            assert_eq!(adopted.after(), after_of(&generated).as_deref());
            assert!(
                adopted
                    .ownership()
                    .first()
                    .expect("owned")
                    .baseline()
                    .is_none()
            );
            assert_eq!(
                adopted.ownership().first().expect("owned").disposition(),
                Disposition::Adopted
            );
            let removed = retire_all(&adopted, after_of(&adopted));
            assert!(removed.after().is_some());
            assert_eq!(removed.ownership(), [] as [OwnedSelection; 0]);
            let mismatch = set_structured(
                after_of(&generated),
                &structured(
                    format,
                    Value::from(1),
                    vec![key("mcp"), key("enabled")],
                    None,
                ),
                Acquisition::Adopt,
            );
            assert_eq!(
                mismatch.observations().first().map(EditObservation::status),
                Some(EditStatus::Conflict)
            );
        }
    }

    #[test]
    fn modified_values_and_absent_owned_selections_retain_claims() {
        for format in [
            DocumentFormat::Json,
            DocumentFormat::Jsonc,
            DocumentFormat::Toml,
        ] {
            let first = set_structured(
                None,
                &structured(format, Value::from("installed"), vec![key("x")], None),
                Acquisition::Conflict,
            );
            let after = after_of(&first).expect("after");
            let modified = replace_all(&after, b"installed", b"user");
            let result = retire_all(&first, Some(modified.clone()));
            assert_eq!(result.after(), Some(modified.as_slice()));
            assert_eq!(result.ownership(), first.ownership());
            assert_eq!(
                result.observations().first().map(EditObservation::status),
                Some(EditStatus::Modified)
            );
            let absent = retire_all(
                &first,
                Some(if format == DocumentFormat::Toml {
                    Vec::new()
                } else {
                    b"{}".to_vec()
                }),
            );
            assert_eq!(absent.ownership(), first.ownership());
            assert_eq!(
                absent.observations().first().map(EditObservation::status),
                Some(EditStatus::Modified)
            );
        }
    }

    #[test]
    fn comment_changes_inside_owned_units_are_modifications() {
        for (format, initial, edited) in [
            (
                DocumentFormat::Jsonc,
                b"{\"x\":{\"v\":1}}".as_slice(),
                b"{\"x\":{\"v\":1 /* changed */}}".as_slice(),
            ),
            (
                DocumentFormat::Toml,
                b"[x]\nv=1\n".as_slice(),
                b"[x]\nv=1 # changed\n".as_slice(),
            ),
        ] {
            let first = set_structured(
                Some(initial.to_vec()),
                &structured(format, obj(&[("v", Value::from(1))]), vec![key("x")], None),
                Acquisition::Adopt,
            );
            let result = retire_all(&first, Some(edited.to_vec()));
            assert_eq!(
                result.observations().first().map(EditObservation::status),
                Some(EditStatus::Modified)
            );
            assert_eq!(result.after(), Some(edited));
            assert_eq!(result.ownership(), first.ownership());
        }
    }

    #[test]
    fn json_whitespace_is_semantically_compared() {
        let first = set_structured(
            Some(b"{\"x\":{\"v\":1}}".to_vec()),
            &structured(
                DocumentFormat::Json,
                obj(&[("v", Value::from(1))]),
                vec![key("x")],
                None,
            ),
            Acquisition::Adopt,
        );
        let result = retire_all(&first, Some(b"{\"x\":{ \"v\" : 1 }}".to_vec()));
        assert_eq!(result.after(), Some(b"{}".as_slice()));
    }

    #[test]
    fn key_trivia_is_foreign_even_after_adoption() {
        for (format, raw, expected) in [
            (
                DocumentFormat::Jsonc,
                b"{\"x\": 1 /* foreign */}".as_slice(),
                b"{  /* foreign */}".as_slice(),
            ),
            (
                DocumentFormat::Toml,
                b"x = 1 # foreign\n".as_slice(),
                b" # foreign\n".as_slice(),
            ),
        ] {
            let first = set_structured(
                Some(raw.to_vec()),
                &structured(format, Value::from(1), vec![key("x")], None),
                Acquisition::Adopt,
            );
            let removed = retire_all(&first, after_of(&first));
            assert_eq!(removed.after(), Some(expected));
        }
    }

    #[test]
    fn created_containers_outlive_first_owner_but_foreign_empty_ones_remain() {
        for format in [
            DocumentFormat::Json,
            DocumentFormat::Jsonc,
            DocumentFormat::Toml,
        ] {
            let a = set_structured(
                None,
                &structured(format, Value::from("A"), vec![key("mcp"), key("a")], None),
                Acquisition::Conflict,
            );
            let b = update_structured(
                &a,
                &structured(format, Value::from("B"), vec![key("mcp"), key("b")], None),
                None,
            );
            let request =
                DocumentRequest::retire(a.ownership().first().expect("owned").claim().clone())
                    .expect("retire");
            let c = edit_document(after_of(&b), &[request], b.ownership(), b.provenance())
                .expect("edit");
            assert!(c.applicable() && c.after().is_some());
            assert_eq!(c.provenance(), b.provenance());
            let removed = retire_all(&c, after_of(&c));
            assert_eq!(removed.after(), None);
            let empty = if format == DocumentFormat::Toml {
                b"[mcp]\n".to_vec()
            } else {
                b"{\"mcp\":{}}".to_vec()
            };
            let foreign = set_structured(
                Some(empty.clone()),
                &structured(format, Value::from("A"), vec![key("mcp"), key("a")], None),
                Acquisition::Conflict,
            );
            assert_eq!(
                foreign.provenance().containers(),
                [] as [CreatedContainer; 0]
            );
            let removed_foreign = retire_all(&foreign, after_of(&foreign));
            assert_eq!(removed_foreign.after(), Some(empty.as_slice()));
        }
    }

    #[test]
    fn foreign_comments_prevent_pruning_and_end_creation_provenance() {
        let first = set_structured(
            None,
            &structured(
                DocumentFormat::Jsonc,
                Value::from(1),
                vec![key("mcp"), key("x")],
                None,
            ),
            Acquisition::Conflict,
        );
        let edited = replace_all(
            &after_of(&first).expect("after"),
            b"\"mcp\":{",
            b"\"mcp\":{/* foreign */",
        );
        let result = retire_all(&first, Some(edited));
        assert!(result.applicable() && result.after().is_some());
        assert!(
            result
                .after()
                .is_some_and(|after| after.windows(7).any(|window| window == b"foreign"))
        );
        assert_eq!(result.provenance(), &DocumentProvenance::default());
        let reinstall = set_structured(
            after_of(&result),
            &structured(
                DocumentFormat::Jsonc,
                Value::from(2),
                vec![key("mcp"), key("x")],
                None,
            ),
            Acquisition::Conflict,
        );
        let removed = retire_all(&reinstall, after_of(&reinstall));
        assert_eq!(removed.after(), result.after());
    }

    fn contains_bytes(data: &[u8], needle: &[u8]) -> bool {
        data.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn array_placements_preserve_foreign_values_and_order() {
        let raw = b"{\"hooks\":[{\"name\":\"a\",\"x\":1},{\"name\":\"b\",\"x\":2}]}";
        let value = obj(&[("name", Value::from("new")), ("x", Value::from(3))]);
        let placements = [
            placement(Position::First),
            placement(Position::Last),
            anchored(Position::Before, member_of(Value::from("b"), &["name"])),
            anchored(Position::After, member_of(Value::from("a"), &["name"])),
        ];
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            for item in placements.iter().cloned() {
                let content = structured_keys(
                    format,
                    value.clone(),
                    &["hooks"],
                    Some(member_of(Value::from("new"), &["name"])),
                    Some(item),
                );
                let first = set_structured(Some(raw.to_vec()), &content, Acquisition::Conflict);
                assert!(first.applicable());
                assert_eq!(
                    first
                        .ownership()
                        .first()
                        .expect("owned")
                        .installed()
                        .value(),
                    &SnapshotValue::Typed(value.clone())
                );
                let removed = retire_all(&first, after_of(&first));
                assert_eq!(removed.after(), Some(raw.as_slice()));
            }
        }
    }

    #[test]
    fn array_roundtrip_retains_comments_and_whitespace() {
        let raw = b"{\"hooks\":[1, /*two*/ 2,\n 3,]}";
        let inserted = structured_keys(
            DocumentFormat::Jsonc,
            Value::from(4),
            &["hooks"],
            Some(member_of(Value::from(4), &[])),
            Some(anchored(Position::Before, member_of(Value::from(2), &[]))),
        );
        let first = set_structured(Some(raw.to_vec()), &inserted, Acquisition::Conflict);
        assert!(first.applicable());
        let removed = retire_all(&first, after_of(&first));
        assert_eq!(removed.after(), Some(raw.as_slice()));
        let adopted = set_structured(
            Some(raw.to_vec()),
            &structured_keys(
                DocumentFormat::Jsonc,
                Value::from(2),
                &["hooks"],
                Some(member_of(Value::from(2), &[])),
                None,
            ),
            Acquisition::Adopt,
        );
        let result = retire_all(&adopted, after_of(&adopted));
        let after = result.after().expect("after");
        assert!(contains_bytes(after, b"two"));
        assert!(contains_bytes(after, b"\n 3,"));
    }

    #[test]
    fn array_takeover_restores_native_identity_payload_and_original_position() {
        let raw = b"{\"hooks\":[{\"name\":\"a\"},{\"name\":\"x\",\"v\":1},{\"name\":\"b\"}]}";
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let member = member_of(Value::from("x"), &["name"]);
            let first = set_structured(
                Some(raw.to_vec()),
                &structured_keys(
                    format,
                    obj(&[("name", Value::from("x")), ("v", Value::from(2))]),
                    &["hooks"],
                    Some(member.clone()),
                    Some(placement(Position::Last)),
                ),
                Acquisition::Takeover,
            );
            assert!(first.applicable());
            let second = update_structured(
                &first,
                &structured_keys(
                    format,
                    obj(&[("name", Value::from("x")), ("v", Value::from(3))]),
                    &["hooks"],
                    Some(member),
                    None,
                ),
                None,
            );
            assert_eq!(
                second.ownership().first().expect("owned").baseline(),
                first.ownership().first().expect("owned").baseline()
            );
            let removed = retire_all(&second, after_of(&second));
            assert_eq!(removed.after(), Some(raw.as_slice()));
            let after = after_of(&second).expect("after");
            let mut swapped = replace_all(&after, b"\"a\"", b"\"temp\"");
            swapped = replace_all(&swapped, b"\"b\"", b"\"a\"");
            swapped = replace_all(&swapped, b"\"temp\"", b"\"b\"");
            let blocked = retire_all(&second, Some(swapped.clone()));
            assert!(!blocked.applicable());
            assert_eq!(blocked.after(), Some(swapped.as_slice()));
            assert_eq!(blocked.ownership(), second.ownership());
        }
    }

    #[test]
    fn array_restore_with_surviving_anchor_and_missing_position_conflict() {
        let raw = b"{\"hooks\":[{\"name\":\"a\"},{\"name\":\"x\",\"v\":1},{\"name\":\"b\"}]}";
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let first = set_structured(
                Some(raw.to_vec()),
                &structured_keys(
                    format,
                    obj(&[("name", Value::from("x")), ("v", Value::from(2))]),
                    &["hooks"],
                    Some(member_of(Value::from("x"), &["name"])),
                    None,
                ),
                Acquisition::Takeover,
            );
            let after = after_of(&first).expect("after");
            let result = retire_all(&first, Some(replace_all(&after, b"{\"name\":\"a\"},", b"")));
            assert!(result.applicable());
            assert_eq!(
                result.after(),
                Some(replace_all(raw, b"{\"name\":\"a\"},", b"").as_slice())
            );
            let mut renamed = replace_all(&after, b"\"a\"", b"\"c\"");
            renamed = replace_all(&renamed, b"\"b\"", b"\"d\"");
            let blocked = retire_all(&first, Some(renamed));
            assert!(!blocked.applicable());
            assert_eq!(blocked.ownership(), first.ownership());
        }
    }

    #[test]
    fn single_array_member_restoration_and_exact_identity_retarget() {
        let raw = b"{\"hooks\":[{\"name\":\"x\",\"v\":1}]}";
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let first = set_structured(
                Some(raw.to_vec()),
                &structured_keys(
                    format,
                    obj(&[("name", Value::from("x")), ("v", Value::from(2))]),
                    &["hooks"],
                    Some(member_of(Value::from("x"), &["name"])),
                    None,
                ),
                Acquisition::Takeover,
            );
            let removed = retire_all(&first, after_of(&first));
            assert_eq!(removed.after(), Some(raw.as_slice()));
            let exact = set_structured(
                None,
                &structured_keys(
                    format,
                    Value::from("old"),
                    &["hooks"],
                    Some(member_of(Value::from("old"), &[])),
                    None,
                ),
                Acquisition::Conflict,
            );
            let retarget = edit_document(
                after_of(&exact),
                &[
                    DocumentRequest::retire(
                        exact.ownership().first().expect("owned").claim().clone(),
                    )
                    .expect("retire request"),
                    DocumentRequest::set(
                        Content::Structured(structured_keys(
                            format,
                            Value::from("new"),
                            &["hooks"],
                            Some(member_of(Value::from("new"), &[])),
                            None,
                        )),
                        Acquisition::Conflict,
                    )
                    .expect("structured request"),
                ],
                exact.ownership(),
                exact.provenance(),
            )
            .expect("retarget edit");
            assert!(retarget.applicable());
            let owned = retarget.ownership().first().expect("owned");
            assert_eq!(owned.disposition(), Disposition::Created);
            assert!(owned.baseline().is_none());
            let removed = retire_all(&retarget, after_of(&retarget));
            assert_eq!(removed.after(), None);
        }
    }

    #[test]
    fn nested_member_keys_and_nested_selections() {
        let raw =
            b"{\"hooks\":[{\"identity\":{\"name\":\"x\"},\"config\":{\"value\":1},\"foreign\":2}]}";
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let content = structured(
                format,
                Value::from(3),
                vec![
                    key("hooks"),
                    member_part(Value::from("x"), &["identity", "name"]),
                    key("config"),
                    key("value"),
                ],
                None,
            );
            let first = set_structured(Some(raw.to_vec()), &content, Acquisition::Takeover);
            assert!(first.applicable());
            let removed = retire_all(&first, after_of(&first));
            assert_eq!(removed.after(), Some(raw.as_slice()));
        }
    }

    #[test]
    fn array_rejects_ambiguous_identities_and_anchors() {
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let error = set_structured_expect_err(
                Some(b"{\"hooks\":[{\"name\":\"a\"},{\"name\":\"a\"}]}".to_vec()),
                &structured_keys(
                    format,
                    obj(&[("name", Value::from("new"))]),
                    &["hooks"],
                    Some(member_of(Value::from("new"), &["name"])),
                    None,
                ),
                Acquisition::Conflict,
            );
            assert_eq!(
                error.message(),
                "duplicate or ambiguous array member identities"
            );
            let error = set_structured_expect_err(
                Some(b"{\"hooks\":[1,1]}".to_vec()),
                &structured_keys(
                    format,
                    Value::from(2),
                    &["hooks"],
                    Some(member_of(Value::from(2), &[])),
                    None,
                ),
                Acquisition::Conflict,
            );
            assert_eq!(
                error.message(),
                "duplicate or ambiguous array member identities"
            );
            let bad_anchor = set_structured(
                None,
                &structured_keys(
                    format,
                    Value::from(2),
                    &["hooks"],
                    Some(member_of(Value::from(2), &[])),
                    Some(anchored(Position::After, member_of(Value::from(1), &[]))),
                ),
                Acquisition::Conflict,
            );
            assert!(!bad_anchor.applicable());
            assert_eq!(bad_anchor.after(), None);
            assert_eq!(
                bad_anchor
                    .observations()
                    .first()
                    .map(|item| item.message().to_owned()),
                Some("array placement anchor is missing".to_owned())
            );
            let mismatch = set_structured(
                None,
                &structured_keys(
                    format,
                    obj(&[("name", Value::from("wrong"))]),
                    &["hooks"],
                    Some(member_of(Value::from("right"), &["name"])),
                    None,
                ),
                Acquisition::Conflict,
            );
            assert!(!mismatch.applicable());
            assert_eq!(mismatch.after(), None);
            assert_eq!(
                mismatch
                    .observations()
                    .first()
                    .map(|item| item.message().to_owned()),
                Some("generated array member does not match its selector".to_owned())
            );
        }
    }

    #[test]
    fn typed_array_identities_distinguish_bool_integer_and_float() {
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let first = set_structured(
                Some(b"{\"hooks\":[true,1,1.0,-0.0,0.0]}".to_vec()),
                &structured_keys(
                    format,
                    Value::from(1),
                    &["hooks"],
                    Some(member_of(Value::from(1), &[])),
                    None,
                ),
                Acquisition::Adopt,
            );
            let result = retire_all(&first, after_of(&first));
            assert_eq!(
                result.after(),
                Some(b"{\"hooks\":[true,1.0,-0.0,0.0]}".as_slice())
            );
        }
    }

    #[test]
    fn scalar_and_container_types_roundtrip() {
        let values = [
            Value::from(true),
            Value::from(false),
            Value::from(12),
            Value::from(-12),
            floatv(1.25),
            floatv(-0.0),
            Value::from("ü string"),
            Value::from("line\nbreak"),
            arr(&[]),
            obj(&[]),
            arr(&[Value::from(1), Value::from("two")]),
        ];
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            for value in values.iter().cloned() {
                let first = set_structured(
                    None,
                    &structured_keys(format, value.clone(), &["x"], None, None),
                    Acquisition::Conflict,
                );
                assert!(first.applicable());
                assert_eq!(
                    first
                        .ownership()
                        .first()
                        .expect("owned")
                        .installed()
                        .value(),
                    &SnapshotValue::Typed(value)
                );
                let removed = retire_all(&first, after_of(&first));
                assert_eq!(removed.after(), None);
            }
        }
    }

    #[test]
    fn null_is_supported_in_json_documents() {
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            let first = set_structured(
                None,
                &structured_keys(format, Value::Scalar(Scalar::null()), &["x"], None, None),
                Acquisition::Conflict,
            );
            assert!(first.applicable());
            assert_eq!(
                first
                    .ownership()
                    .first()
                    .expect("owned")
                    .installed()
                    .value(),
                &SnapshotValue::Typed(Value::Scalar(Scalar::null()))
            );
            assert_eq!(after_of(&first), Some(b"{\"x\":null}".to_vec()));
            let removed = retire_all(&first, after_of(&first));
            assert_eq!(removed.after(), None);
        }
    }

    #[test]
    fn malformed_structured_input_is_rejected() {
        let rows: &[(DocumentFormat, &[u8], &str)] = &[
            (
                DocumentFormat::Json,
                b"{\"a\":1,\"a\":2}",
                "duplicate JSON object key",
            ),
            (
                DocumentFormat::Json,
                b"{\"a\":NaN}",
                "floats must be finite",
            ),
            (
                DocumentFormat::Json,
                b"{\"a\":1e400}",
                "floats must be finite",
            ),
            (
                DocumentFormat::Json,
                b"{\"a\":9223372036854775808}",
                "integers must fit signed 64 bits",
            ),
            (
                DocumentFormat::Json,
                b"{\"a\":1,}",
                "JSON trailing commas are unsupported",
            ),
            (
                DocumentFormat::Json,
                b"{/* c */}",
                "Expecting value: line 1 column 2 (char 1)",
            ),
            (
                DocumentFormat::Jsonc,
                b"{\"a\":1,\"a\":1}",
                "duplicate JSON object key",
            ),
            (
                DocumentFormat::Jsonc,
                b"{/*oops",
                "unterminated JSONC comment",
            ),
            (
                DocumentFormat::Jsonc,
                b"{\"a\":1} trailing",
                "unexpected data after JSON document",
            ),
            (
                DocumentFormat::Jsonc,
                b"{1:2}",
                "JSON object keys must be strings",
            ),
            (
                DocumentFormat::Jsonc,
                b"{\"a\" 1}",
                "missing JSON object colon",
            ),
            (
                DocumentFormat::Jsonc,
                b"[",
                "unterminated or malformed JSON container",
            ),
            (
                DocumentFormat::Jsonc,
                b"[]]",
                "unexpected data after JSON document",
            ),
            (
                DocumentFormat::Jsonc,
                b"{\"a\":\"\\ud800\"}",
                "scalar must contain Unicode scalar values",
            ),
        ];
        for (format, raw, message) in rows {
            let error = set_structured_expect_err(
                Some(raw.to_vec()),
                &structured_keys(*format, Value::from(1), &["mcp", "x"], None, None),
                Acquisition::Conflict,
            );
            assert_eq!(error.message(), *message);
        }
    }

    #[test]
    fn invalid_selector_paths_are_rejected() {
        let error = set_structured_expect_err(
            Some(b"{\"x\":1}".to_vec()),
            &structured_keys(
                DocumentFormat::Json,
                Value::from(1),
                &["x", "y"],
                None,
                None,
            ),
            Acquisition::Conflict,
        );
        assert_eq!(error.message(), "object key selector crosses a non-object");
        let error = set_structured_expect_err(
            Some(b"{\"x\":{}}".to_vec()),
            &structured_keys(
                DocumentFormat::Json,
                Value::from(1),
                &["x"],
                Some(member_of(Value::from("a"), &[])),
                None,
            ),
            Acquisition::Conflict,
        );
        assert_eq!(error.message(), "member selector requires an array");
        let error = set_structured_expect_err(
            Some(b"{}".to_vec()),
            &structured_keys(
                DocumentFormat::Json,
                Value::from(1),
                &[],
                Some(member_of(Value::from("a"), &[])),
                None,
            ),
            Acquisition::Conflict,
        );
        assert_eq!(error.message(), "member selector requires an array");
    }

    #[test]
    fn json_root_array_and_empty_comment_documents() {
        let first = set_structured(
            None,
            &structured_keys(
                DocumentFormat::Json,
                Value::from(1),
                &[],
                Some(member_of(Value::from(1), &[])),
                None,
            ),
            Acquisition::Conflict,
        );
        assert_eq!(after_of(&first), Some(b"[1]".to_vec()));
        let removed = retire_all(&first, after_of(&first));
        assert_eq!(removed.after(), None);
        let error = set_structured_expect_err(
            Some(b"// no root".to_vec()),
            &structured_keys(DocumentFormat::Jsonc, Value::from(1), &["x"], None, None),
            Acquisition::Conflict,
        );
        assert_eq!(error.message(), "missing JSON value");

        let first = set_structured(
            Some(b"# preserved\n".to_vec()),
            &structured_keys(DocumentFormat::Toml, Value::from(1), &["x"], None, None),
            Acquisition::Conflict,
        );
        let removed = retire_all(&first, after_of(&first));
        assert_eq!(removed.after(), Some(b"# preserved\n".as_slice()));
    }

    #[test]
    fn toml_no_final_newline_and_foreign_table_order() {
        for raw in [
            b"foreign=1".as_slice(),
            b"[foreign]\nx=1\n".as_slice(),
            b"first=1\n[foreign]\nx=1\n".as_slice(),
            b"a.b=1\nforeign=2\n".as_slice(),
        ] {
            let first = set_structured(
                Some(raw.to_vec()),
                &structured_keys(DocumentFormat::Toml, Value::from(2), &["x"], None, None),
                Acquisition::Conflict,
            );
            assert!(first.applicable());
            let removed = retire_all(&first, after_of(&first));
            assert_eq!(removed.after(), Some(raw));
        }
    }

    #[test]
    fn toml_foreign_nonfinite_numbers_remain_untouched() {
        let raw = b"foreign=inf\nother=nan\nx=1\n";
        let first = set_structured(
            Some(raw.to_vec()),
            &structured_keys(DocumentFormat::Toml, Value::from(2), &["x"], None, None),
            Acquisition::Takeover,
        );
        assert!(first.applicable());
        let removed = retire_all(&first, after_of(&first));
        assert_eq!(removed.after(), Some(raw.as_slice()));
    }

    #[test]
    fn toml_array_of_tables_and_out_of_order_foreign_tables() {
        let raw = b"[[hooks]]\nname=\"a\"\nx=1\n[[hooks]]\nname=\"b\"\nx=2\n";
        let first = set_structured(
            Some(raw.to_vec()),
            &structured_keys(
                DocumentFormat::Toml,
                obj(&[("name", Value::from("x")), ("x", Value::from(3))]),
                &["hooks"],
                Some(member_of(Value::from("x"), &["name"])),
                None,
            ),
            Acquisition::Conflict,
        );
        assert!(first.applicable());
        assert_eq!(
            retire_all(&first, after_of(&first)).after(),
            Some(raw.as_slice())
        );

        let out_of_order = b"[a.first]\nx=1\n[foreign]\ny=2\n[a.second]\nz=3\n";
        let second = set_structured(
            Some(out_of_order.to_vec()),
            &structured_keys(
                DocumentFormat::Toml,
                Value::from(4),
                &["a", "second", "z"],
                None,
                None,
            ),
            Acquisition::Takeover,
        );
        assert!(second.applicable(), "{:?}", second.observations());
        assert_eq!(
            retire_all(&second, after_of(&second)).after(),
            Some(out_of_order.as_slice())
        );
        let error = set_structured_expect_err(
            Some(out_of_order.to_vec()),
            &structured_keys(DocumentFormat::Toml, obj(&[]), &["a"], None, None),
            Acquisition::Takeover,
        );
        assert_eq!(
            error.message(),
            "a noncontiguous TOML table cannot be an owned unit"
        );
    }

    #[test]
    fn toml_reversible_object_updates_preserve_key_layout() {
        let rows = [
            (
                b"x=1\nforeign=2\n".as_slice(),
                obj(&[("nested", Value::from(1))]),
            ),
            (
                b"[x]\nnested=1\n[foreign]\ny=2\n".as_slice(),
                obj(&[("nested", Value::from(2))]),
            ),
            (
                b"x = { nested = 1 } # foreign\nother=3\n".as_slice(),
                obj(&[("nested", Value::from(2))]),
            ),
        ];
        for (raw, value) in rows {
            let first = set_structured(
                Some(raw.to_vec()),
                &structured_keys(DocumentFormat::Toml, value, &["x"], None, None),
                Acquisition::Takeover,
            );
            assert!(first.applicable());
            let newer = update_structured(
                &first,
                &structured_keys(
                    DocumentFormat::Toml,
                    obj(&[("newer", Value::from(true))]),
                    &["x"],
                    None,
                    None,
                ),
                None,
            );
            assert!(newer.applicable());
            assert_eq!(
                newer.ownership().first().and_then(OwnedSelection::baseline),
                first.ownership().first().and_then(OwnedSelection::baseline)
            );
            assert_eq!(retire_all(&newer, after_of(&newer)).after(), Some(raw));
        }
    }

    #[test]
    fn toml_representation_changes_refuse_without_mutation() {
        let rows = [
            (
                b"[x]\nnested=1\n[foreign]\ny=2\n".as_slice(),
                Value::from(2),
            ),
            (b"[x]\nnested=1\n".as_slice(), arr(&[])),
            (
                b"[[x]]\nname=\"a\"\n".as_slice(),
                obj(&[("name", Value::from("b"))]),
            ),
        ];
        for (raw, value) in rows {
            let result = set_structured(
                Some(raw.to_vec()),
                &structured_keys(DocumentFormat::Toml, value, &["x"], None, None),
                Acquisition::Takeover,
            );
            assert_eq!(
                result.observations().first().map(EditObservation::status),
                Some(EditStatus::Conflict)
            );
            assert_eq!(result.after(), Some(raw));
            assert_eq!(result.ownership(), [] as [OwnedSelection; 0]);
        }
    }

    #[test]
    fn toml_dotted_tables_and_child_before_parent_restore_exactly() {
        for gap in [b"".as_slice(), b"# between\n\n".as_slice()] {
            let mut raw = b"mcp.foo.command=\"old\"\n".to_vec();
            raw.extend_from_slice(gap);
            raw.extend_from_slice(b"mcp.foo.env.X=\"before\"\nforeign=1\n");
            let first = set_structured(
                Some(raw.clone()),
                &structured_keys(
                    DocumentFormat::Toml,
                    obj(&[
                        ("command", Value::from("new")),
                        ("env", obj(&[("Y", Value::from("z"))])),
                    ]),
                    &["mcp", "foo"],
                    None,
                    None,
                ),
                Acquisition::Takeover,
            );
            assert!(first.applicable());
            assert_eq!(
                retire_all(&first, after_of(&first)).after(),
                Some(raw.as_slice())
            );
        }

        let split = b"mcp.foo.command=\"old\"\nforeign=1\nmcp.foo.env.X=\"before\"\n";
        let error = set_structured_expect_err(
            Some(split.to_vec()),
            &structured_keys(
                DocumentFormat::Toml,
                obj(&[("command", Value::from("new"))]),
                &["mcp", "foo"],
                None,
                None,
            ),
            Acquisition::Takeover,
        );
        assert_eq!(
            error.message(),
            "a noncontiguous TOML table cannot be an owned unit"
        );
        let scalar = set_structured(
            Some(split.to_vec()),
            &structured_keys(
                DocumentFormat::Toml,
                Value::from("new"),
                &["mcp", "foo", "command"],
                None,
                None,
            ),
            Acquisition::Takeover,
        );
        assert!(scalar.applicable(), "{:?}", scalar.observations());
        assert_eq!(
            retire_all(&scalar, after_of(&scalar)).after(),
            Some(split.as_slice())
        );

        for newline in [b"\n".as_slice(), b"\r\n".as_slice()] {
            let raw = replace_all(
                b"# before\n[mcp . \"foo\" . env] # child\nX=\"old\"\n# between\n[mcp . \"foo\"] # parent\ncommand=\"old\"\n# gap\n[foreign]\nx=1\n",
                b"\n",
                newline,
            );
            for parts in [&["mcp", "foo"][..], &["mcp"][..]] {
                let value = obj(&[
                    ("command", Value::from("new")),
                    ("env", obj(&[("X", Value::from("new"))])),
                ]);
                let value = if parts.len() == 1 {
                    obj(&[("foo", value)])
                } else {
                    value
                };
                let first = set_structured(
                    Some(raw.clone()),
                    &structured_keys(DocumentFormat::Toml, value, parts, None, None),
                    Acquisition::Takeover,
                );
                assert!(first.applicable());
                assert_eq!(
                    retire_all(&first, after_of(&first)).after(),
                    Some(raw.as_slice())
                );
            }
        }
    }

    #[test]
    fn toml_nesting_boundaries_match_the_reference() {
        let content = structured_keys(DocumentFormat::Toml, Value::from(2), &["y"], None, None);
        for depth in [100, 101] {
            let key = format!("[{}]\nx=1\n", vec!["a"; depth].join("."));
            let array = format!("x={}1{}\n", "[".repeat(depth), "]".repeat(depth));
            let inline = format!("x={}1{}\n", "{a=".repeat(depth), "}".repeat(depth));
            for (document, message) in [
                (
                    key,
                    format!("TOML key nested more than {depth} levels deep"),
                ),
                (
                    array,
                    format!("TOML value nested more than {depth} levels deep"),
                ),
                (
                    inline,
                    format!("TOML value nested more than {depth} levels deep"),
                ),
            ] {
                if depth == 100 {
                    let edit = set_structured(
                        Some(document.into_bytes()),
                        &content,
                        Acquisition::Conflict,
                    );
                    assert!(edit.applicable());
                } else {
                    let error = set_structured_expect_err(
                        Some(document.into_bytes()),
                        &content,
                        Acquisition::Conflict,
                    );
                    assert_eq!(
                        error.message().split(" at line").next(),
                        message.replace("101", "100").split(" at line").next()
                    );
                }
            }
        }

        for depth in [100, 101] {
            let value = deep_array(depth);
            let result = set_structured(
                None,
                &structured_keys(DocumentFormat::Toml, value, &["x"], None, None),
                Acquisition::Conflict,
            );
            assert_eq!(result.applicable(), depth == 100);
            if depth == 101 {
                assert_eq!(
                    result.observations().first().map(EditObservation::message),
                    Some("TOML value nested more than 100 levels deep at line 1 col 108")
                );
            }
        }
    }

    #[test]
    fn jsonc_key_comments_and_spacing_are_foreign() {
        let raw = b"{\"x\" /* before colon */ : /* before value */ 1, \"foreign\":2}";
        let first = set_structured(
            Some(raw.to_vec()),
            &structured_keys(DocumentFormat::Jsonc, Value::from(1), &["x"], None, None),
            Acquisition::Adopt,
        );
        let removed = retire_all(&first, after_of(&first));
        assert_eq!(
            removed.after(),
            Some(b"{ /* before colon */  /* before value */  \"foreign\":2}".as_slice())
        );
    }

    #[test]
    fn created_container_with_user_whitespace_is_retained() {
        let first = set_structured(
            None,
            &structured_keys(
                DocumentFormat::Jsonc,
                Value::from(1),
                &["mcp", "x"],
                None,
                None,
            ),
            Acquisition::Conflict,
        );
        let after = after_of(&first).expect("after");
        let foreign = replace_all(&after, b"{\"x\"", b"{  \"x\"");
        let result = retire_all(&first, Some(foreign));
        assert_eq!(result.after(), Some(b"{\"mcp\":{  }}".as_slice()));
        assert_eq!(result.provenance(), &DocumentProvenance::default());
    }

    #[test]
    fn array_moves_restore_original_trivia() {
        let raw =
            b"{\"hooks\":[ {\"name\":\"a\"}, /* x */ {\"name\":\"x\"}, /* b */ {\"name\":\"b\"} ]}";
        for selected in ["a", "x", "b"] {
            for position in [Position::First, Position::Last] {
                let first = set_structured(
                    Some(raw.to_vec()),
                    &structured_keys(
                        DocumentFormat::Jsonc,
                        obj(&[("name", Value::from(selected)), ("v", Value::from(2))]),
                        &["hooks"],
                        Some(member_of(Value::from(selected), &["name"])),
                        Some(placement(position)),
                    ),
                    Acquisition::Takeover,
                );
                assert!(first.applicable());
                let result = retire_all(&first, after_of(&first));
                assert!(result.applicable());
                assert_eq!(result.after(), Some(raw.as_slice()));
            }
        }
    }

    #[test]
    fn changed_restoration_gap_preserves_foreign_revision() {
        let raw =
            b"{\"hooks\":[{\"name\":\"a\"}, /* original */ {\"name\":\"x\"},{\"name\":\"b\"}]}";
        let first = set_structured(
            Some(raw.to_vec()),
            &structured_keys(
                DocumentFormat::Jsonc,
                obj(&[("name", Value::from("x")), ("v", Value::from(2))]),
                &["hooks"],
                Some(member_of(Value::from("x"), &["name"])),
                Some(placement(Position::Last)),
            ),
            Acquisition::Takeover,
        );
        assert!(first.applicable());
        let after = after_of(&first).expect("after");
        let changed = replace_all(&after, b"original", b"changed");
        let result = retire_all(&first, Some(changed.clone()));
        assert_eq!(
            result.observations().first().map(EditObservation::status),
            Some(EditStatus::Conflict)
        );
        assert_eq!(result.after(), Some(changed.as_slice()));
        assert_eq!(result.ownership(), first.ownership());
    }

    #[test]
    fn batched_array_takeovers_capture_positions_before_any_edit() {
        let raw =
            b"{\"hooks\":[{\"name\":\"a\"},{\"name\":\"x\"},{\"name\":\"y\"},{\"name\":\"b\"}]}";
        let x = structured_keys(
            DocumentFormat::Json,
            obj(&[("name", Value::from("x")), ("v", Value::from(1))]),
            &["hooks"],
            Some(member_of(Value::from("x"), &["name"])),
            Some(placement(Position::Last)),
        );
        let y = structured_keys(
            DocumentFormat::Json,
            obj(&[("name", Value::from("y")), ("v", Value::from(2))]),
            &["hooks"],
            Some(member_of(Value::from("y"), &["name"])),
            None,
        );
        let result = edit_document(
            Some(raw.to_vec()),
            &[
                DocumentRequest::set(Content::Structured(x), Acquisition::Takeover)
                    .expect("structured request"),
                DocumentRequest::set(Content::Structured(y), Acquisition::Takeover)
                    .expect("structured request"),
            ],
            &[],
            &DocumentProvenance::default(),
        )
        .expect("batched edit");
        assert!(result.applicable());
        let baseline = result
            .ownership()
            .get(1)
            .and_then(OwnedSelection::baseline)
            .expect("baseline");
        let position = baseline.position().expect("position");
        assert_eq!(
            position.previous(),
            Some(&member_of(Value::from("x"), &["name"]))
        );
        assert_eq!(
            position.following(),
            Some(&member_of(Value::from("b"), &["name"]))
        );
        let removed = retire_all(&result, after_of(&result));
        assert_eq!(removed.after(), Some(raw.as_slice()));
    }

    #[test]
    fn batch_retirement_plans_all_original_positions() {
        let rows = [
            (
                DocumentFormat::Json,
                b"{\"hooks\":[{\"name\":\"a\"},{\"name\":\"x\"},{\"name\":\"y\"},{\"name\":\"b\"}]}"
                    .as_slice(),
            ),
            (
                DocumentFormat::Jsonc,
                b"{\"hooks\":[ {\"name\":\"a\"}, /* x */ {\"name\":\"x\"}, /* y */ {\"name\":\"y\"}, /* b */ {\"name\":\"b\"} ]}"
                    .as_slice(),
            ),
        ];
        for (format, raw) in rows {
            for reverse in [false, true] {
                for move_y in [false, true] {
                    let contents = ["x", "y"].map(|name| {
                        let placement = if name == "x" {
                            Some(placement(Position::Last))
                        } else if move_y {
                            Some(placement(Position::First))
                        } else {
                            None
                        };
                        structured_keys(
                            format,
                            obj(&[("name", Value::from(name)), ("v", Value::from(1))]),
                            &["hooks"],
                            Some(member_of(Value::from(name), &["name"])),
                            placement,
                        )
                    });
                    let first = edit_document(
                        Some(raw.to_vec()),
                        &contents
                            .iter()
                            .map(|content| {
                                DocumentRequest::set(
                                    Content::Structured(content.clone()),
                                    Acquisition::Takeover,
                                )
                                .expect("structured request")
                            })
                            .collect::<Vec<_>>(),
                        &[],
                        &DocumentProvenance::default(),
                    )
                    .expect("batched takeover");
                    assert!(first.applicable());
                    let mut ownership = first.ownership().to_vec();
                    if reverse {
                        ownership.reverse();
                    }
                    let requests: Vec<DocumentRequest> = ownership
                        .iter()
                        .map(|item| {
                            DocumentRequest::retire(item.claim().clone()).expect("retire request")
                        })
                        .collect();
                    let restored =
                        edit_document(after_of(&first), &requests, &ownership, first.provenance())
                            .expect("batch retirement");
                    assert!(restored.applicable());
                    assert_eq!(restored.after(), Some(raw));
                    let after = after_of(&first).expect("after");
                    let mut reordered = replace_all(&after, b"\"a\"", b"\"temp\"");
                    reordered = replace_all(&reordered, b"\"b\"", b"\"a\"");
                    reordered = replace_all(&reordered, b"\"temp\"", b"\"b\"");
                    let blocked = edit_document(
                        Some(reordered.clone()),
                        &requests,
                        &ownership,
                        first.provenance(),
                    )
                    .expect("blocked retirement");
                    assert!(!blocked.applicable());
                    assert_eq!(blocked.after(), Some(reordered.as_slice()));
                    assert_eq!(blocked.ownership(), ownership);
                }
            }
        }
    }

    #[test]
    fn documents_require_complete_roots() {
        let error = set_structured_expect_err(
            Some(b"1".to_vec()),
            &structured_keys(DocumentFormat::Json, Value::from(1), &["x"], None, None),
            Acquisition::Conflict,
        );
        assert_eq!(error.message(), "object key selector crosses a non-object");
        let first = set_structured(
            None,
            &structured_keys(DocumentFormat::Json, Value::from(1), &["x"], None, None),
            Acquisition::Conflict,
        );
        let changed = edit_document(
            after_of(&first),
            &[
                DocumentRequest::set(
                    Content::Structured(structured_keys(
                        DocumentFormat::Json,
                        Value::from(2),
                        &["x"],
                        None,
                        None,
                    )),
                    Acquisition::Conflict,
                )
                .expect("structured request"),
                DocumentRequest::set(
                    Content::Structured(structured_keys(
                        DocumentFormat::Json,
                        Value::from(3),
                        &["bad"],
                        Some(member_of(Value::from(4), &[])),
                        None,
                    )),
                    Acquisition::Conflict,
                )
                .expect("structured request"),
            ],
            first.ownership(),
            first.provenance(),
        )
        .expect("edit");
        assert!(!changed.applicable());
        assert_eq!(changed.after(), after_of(&first).as_deref());
        assert_eq!(changed.ownership(), first.ownership());
    }

    #[test]
    fn missing_intermediate_member_reports_conflict() {
        let result = set_structured(
            Some(b"{\"x\":[]}".to_vec()),
            &structured(
                DocumentFormat::Json,
                Value::from(1),
                vec![key("x"), member_part(Value::from("a"), &[]), key("y")],
                None,
            ),
            Acquisition::Conflict,
        );
        assert!(!result.applicable());
        assert_eq!(result.after(), Some(b"{\"x\":[]}".as_slice()));
        assert_eq!(
            result
                .observations()
                .first()
                .map(|item| item.message().to_owned()),
            Some("intermediate array member is missing".to_owned())
        );
    }

    #[test]
    fn deeply_nested_documents_report_a_nesting_error() {
        let mut raw = vec![b'['; 1200];
        raw.extend(std::iter::repeat_n(b']', 1200));
        let error = set_structured_expect_err(
            Some(raw),
            &structured_keys(DocumentFormat::Json, Value::from(1), &["x"], None, None),
            Acquisition::Conflict,
        );
        assert_eq!(error.message(), "document nesting is too deep");
    }

    #[test]
    fn snapshot_values_match_the_reference_recursion_budget() {
        for (value, accepted) in [
            (deep_object_value(197), true),
            (deep_object_value(198), false),
            (deep_array_value(247), true),
            (deep_array_value(248), false),
            (deep_alternating_value(219), true),
            (deep_alternating_value(220), false),
        ] {
            let raw = format!("{{\"a\":{value}}}").into_bytes();
            let content = structured_keys(DocumentFormat::Json, Value::from(2), &["a"], None, None);
            if accepted {
                let edit = set_structured(Some(raw), &content, Acquisition::Takeover);
                assert!(edit.applicable());
                assert_eq!(edit.after(), Some(b"{\"a\":2}".as_slice()));
            } else {
                let error = set_structured_expect_err(Some(raw), &content, Acquisition::Takeover);
                assert_eq!(error.message(), "document nesting is too deep");
            }
        }
    }

    #[test]
    fn deeply_nested_requested_values_match_the_reference() {
        for (value, expected) in [
            (
                deep_object(197),
                Some(format!("{{\"x\":{}}}", deep_object_value(197))),
            ),
            (deep_object(198), None),
            (
                deep_array(247),
                Some(format!("{{\"x\":{}}}", deep_array_value(247))),
            ),
            (deep_array(248), None),
        ] {
            let content = structured_keys(DocumentFormat::Json, value, &["x"], None, None);
            if let Some(document) = expected {
                let edit = set_structured(None, &content, Acquisition::Conflict);
                assert!(edit.applicable());
                assert_eq!(edit.after(), Some(document.as_bytes()));
            } else {
                let error = set_structured_expect_err(None, &content, Acquisition::Conflict);
                assert_eq!(error.message(), "document nesting is too deep");
            }
        }
        let existing = Some(b"{\"x\":1}".to_vec());
        let error = set_structured_expect_err(
            existing.clone(),
            &structured_keys(DocumentFormat::Json, deep_object(198), &["x"], None, None),
            Acquisition::Takeover,
        );
        assert_eq!(error.message(), "document nesting is too deep");
        let conflict = set_structured(
            existing,
            &structured_keys(DocumentFormat::Json, deep_object(198), &["x"], None, None),
            Acquisition::Conflict,
        );
        assert!(!conflict.applicable());
        assert_eq!(conflict.after(), Some(b"{\"x\":1}".as_slice()));
        assert_eq!(
            conflict
                .observations()
                .first()
                .map(EditObservation::message),
            Some("")
        );
    }

    #[test]
    fn parse_boundaries_match_the_reference_recursion_budget() {
        let content = structured_keys(DocumentFormat::Json, Value::from(2), &["a"], None, None);
        for (chain, accepted) in [
            (deep_array_value(984), true),
            (deep_array_value(985), false),
            (deep_object_value(984), true),
            (deep_object_value(985), false),
        ] {
            let raw = format!("{{\"a\":1,\"b\":{chain}}}").into_bytes();
            let expected = format!("{{\"a\":2,\"b\":{chain}}}").into_bytes();
            if accepted {
                let edit = set_structured(Some(raw), &content, Acquisition::Takeover);
                assert!(edit.applicable());
                assert_eq!(edit.after(), Some(expected.as_slice()));
            } else {
                let error = set_structured_expect_err(Some(raw), &content, Acquisition::Takeover);
                assert_eq!(error.message(), "document nesting is too deep");
            }
        }
    }

    #[test]
    fn anchor_neighbours_match_the_reference_recursion_budget() {
        let named_content = structured(
            DocumentFormat::Json,
            obj(&[("name", Value::from("c"))]),
            vec![key("hooks"), member_part(Value::from("b"), &["name"])],
            None,
        );
        let whole_content = structured(
            DocumentFormat::Json,
            Value::from("c"),
            vec![key("hooks"), member_part(Value::from("b"), &[])],
            None,
        );
        let vectors: Vec<(String, &StructuredContent, Option<&str>)> = vec![
            (
                format!(
                    "{{\"hooks\":[{},{{\"name\":\"b\"}}]}}",
                    deep_object_value(196)
                ),
                &named_content,
                None,
            ),
            (
                format!(
                    "{{\"hooks\":[{},{{\"name\":\"b\"}}]}}",
                    deep_object_value(197)
                ),
                &named_content,
                Some("document nesting is too deep"),
            ),
            (
                format!(
                    "{{\"hooks\":[{},{{\"name\":\"b\"}}]}}",
                    deep_object_value(198)
                ),
                &named_content,
                Some("document nesting is too deep"),
            ),
            (
                format!(
                    "{{\"hooks\":[{},{{\"name\":\"b\"}}]}}",
                    deep_array_value(246)
                ),
                &named_content,
                None,
            ),
            (
                format!(
                    "{{\"hooks\":[{},{{\"name\":\"b\"}}]}}",
                    deep_array_value(247)
                ),
                &named_content,
                Some("document nesting is too deep"),
            ),
            (
                format!("{{\"hooks\":[{},\"b\"]}}", deep_object_value(196)),
                &whole_content,
                None,
            ),
            (
                format!("{{\"hooks\":[{},\"b\"]}}", deep_object_value(197)),
                &whole_content,
                Some("document nesting is too deep"),
            ),
            (
                format!("{{\"hooks\":[{},\"b\"]}}", deep_array_value(246)),
                &whole_content,
                None,
            ),
            (
                format!("{{\"hooks\":[{},\"b\"]}}", deep_array_value(247)),
                &whole_content,
                Some("document nesting is too deep"),
            ),
            (
                format!(
                    "{{\"hooks\":[{{\"name\":{}}},{{\"name\":\"b\"}}]}}",
                    deep_object_value(195)
                ),
                &named_content,
                None,
            ),
            (
                format!(
                    "{{\"hooks\":[{{\"name\":{}}},{{\"name\":\"b\"}}]}}",
                    deep_object_value(196)
                ),
                &named_content,
                Some("document nesting is too deep"),
            ),
            (
                String::from("{\"hooks\":[{\"name\":{\"nested\":1}},{\"name\":\"b\"}]}"),
                &named_content,
                None,
            ),
        ];
        for (document, content, error) in vectors {
            let raw = document.into_bytes();
            if let Some(message) = error {
                let error = set_structured_expect_err(Some(raw), content, Acquisition::Takeover);
                assert_eq!(error.message(), message);
            } else {
                let edit = set_structured(Some(raw.clone()), content, Acquisition::Takeover);
                assert!(!edit.applicable());
                assert_eq!(edit.after(), Some(raw.as_slice()));
                let observation = edit.observations().first().expect("observation");
                assert_eq!(observation.status(), EditStatus::Conflict);
                assert_eq!(
                    observation.message(),
                    "generated array member does not match its selector"
                );
            }
        }
    }

    #[test]
    fn temporal_structured_values_conflict_in_json_documents() {
        for format in [DocumentFormat::Json, DocumentFormat::Jsonc] {
            for (value, message) in [
                (
                    Value::Scalar(Scalar::date(
                        crate::temporal::Date::new(2026, 1, 2).unwrap(),
                    )),
                    "Object of type date is not JSON serializable",
                ),
                (
                    Value::Scalar(Scalar::time(
                        crate::temporal::Time::new(1, 2, 3, 0).unwrap(),
                    )),
                    "Object of type time is not JSON serializable",
                ),
                (
                    Value::Scalar(Scalar::datetime(crate::temporal::DateTime::local(
                        crate::temporal::Date::new(2026, 1, 2).unwrap(),
                        crate::temporal::Time::new(1, 2, 3, 0).unwrap(),
                    ))),
                    "Object of type datetime is not JSON serializable",
                ),
            ] {
                let result = set_structured(
                    None,
                    &structured_keys(format, value, &["x"], None, None),
                    Acquisition::Conflict,
                );
                assert!(!result.applicable());
                assert_eq!(result.after(), None);
                assert_eq!(result.ownership(), [] as [OwnedSelection; 0]);
                let observation = result.observations().first().expect("observation");
                assert_eq!(observation.status(), EditStatus::Conflict);
                assert_eq!(observation.current(), None);
                assert_eq!(observation.message(), message);
            }
        }
    }

    #[test]
    fn edit_results_and_inputs_are_immutable() {
        let first = set_structured(
            None,
            &structured_keys(
                DocumentFormat::Json,
                obj(&[("x", arr(&[Value::from(1)]))]),
                &["mcp"],
                None,
                None,
            ),
            Acquisition::Conflict,
        );
        let snapshot = first.clone();
        let ownership = first.ownership().to_vec();
        let provenance = first.provenance().clone();
        let containers = first.provenance().containers().to_vec();
        assert_eq!(first.after(), Some(b"{\"mcp\":{\"x\":[1]}}".as_slice()));
        let second = update_structured(
            &first,
            &structured_keys(
                DocumentFormat::Json,
                obj(&[("x", arr(&[Value::from(2)]))]),
                &["mcp"],
                None,
                None,
            ),
            None,
        );
        assert!(second.applicable());
        assert_eq!(second.after(), Some(b"{\"mcp\":{\"x\":[2]}}".as_slice()));
        assert_eq!(first, snapshot);
        assert_eq!(first.ownership(), ownership.as_slice());
        assert_eq!(first.provenance(), &provenance);
        assert_eq!(first.provenance().containers(), containers.as_slice());
    }
}
