#![allow(dead_code)]

use crate::authority::{Claim, ClaimSelector, Ownership};
use crate::content::{
    DocumentFormat, DocumentSchema, Key, Member, SectionBoundaries, Selector, SelectorPart,
};
use crate::editors::{
    ArrayPosition, CreatedContainer, Disposition, DocumentProvenance, OwnedSelection,
    SelectionSnapshot, SnapshotValue,
};
use crate::json::{Json, write_escaped};
use crate::resource_models::{
    Ancestor, Count, Generation, Index, Node, OwnedClaim, Preparation, Receipt, ResourceKind,
    ResourceRef, Revision,
};
use crate::resource_transaction::Journal;
use crate::values::{Scalar, Value, ValueError, semantic_record, value_from_record};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr as _;

pub trait StateCodec: Sized {
    fn to_state(&self) -> Json;
    fn from_state(value: &Json) -> Result<Self, ValueError>;
}

pub fn encode<T: StateCodec>(value: &T) -> Vec<u8> {
    encode_state(&value.to_state())
}

pub fn encode_state(record: &Json) -> Vec<u8> {
    let mut encoded = String::new();
    write_canonical(record, &mut encoded);
    encoded.push('\n');
    encoded.into_bytes()
}

fn python_type_error(error: ValueError) -> ValueError {
    if error.is_python_type_error() {
        ValueError::new("invalid state record")
    } else {
        error
    }
}

pub fn decode<T: StateCodec>(data: &[u8]) -> Result<T, ValueError> {
    let text = std::str::from_utf8(data).map_err(|_| ValueError::new("invalid state record"))?;
    let raw = Json::parse(text.strip_prefix('\u{feff}').unwrap_or(text))?;
    let value = T::from_state(&raw).map_err(python_type_error)?;
    if canonical(&value.to_state()) != canonical(&raw) {
        return Err(ValueError::new("state record is not canonical"));
    }
    Ok(value)
}

pub fn decode_metadata<T: StateCodec>(bytes: &[u8], tag: &str) -> Result<T, ValueError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ValueError::new("invalid state record"))?;
    let raw = Json::parse(text.strip_prefix('\u{feff}').unwrap_or(text))?;
    decoded(&raw).map_err(python_type_error)?;
    if raw.get("type").and_then(Json::as_str) != Some(tag) {
        if canonical(&canonical_state(&raw).map_err(python_type_error)?) != canonical(&raw) {
            return Err(ValueError::new("state record is not canonical"));
        }
        return Err(ValueError::new("metadata record has the wrong type"));
    }
    decode::<T>(bytes)
}

fn canonical_state(value: &Json) -> Result<Json, ValueError> {
    match value {
        Json::Null | Json::Bool(_) | Json::Int(_) | Json::BigInt(_) | Json::Str(_) => {
            Ok(value.clone())
        }
        Json::Float(_) => Err(ValueError::new("invalid state scalar")),
        Json::Array(items) => Ok(Json::Array(
            items
                .iter()
                .map(canonical_state)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Json::Object(items) => {
            let only =
                |name: &str| items.len() == 1 && items.first().is_some_and(|(key, _)| key == name);
            if only("value") {
                let inner = value.get("value").ok_or_else(unknown_record)?;
                return Ok(encoded_semantic(&value_from_record(inner)?));
            }
            if only("bytes") {
                let inner = value
                    .get("bytes")
                    .and_then(Json::as_str)
                    .ok_or_else(unknown_record)?;
                return Ok(encoded_bytes(&parse_hex_bytes(inner)?));
            }
            if only("path") {
                return Ok(encoded_path(&decode_path(value)?));
            }
            let members = items
                .iter()
                .map(|(key, item)| canonical_state(item).map(|value| (key.clone(), value)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Json::Object(members))
        }
    }
}

fn unknown_record() -> ValueError {
    ValueError::new("unknown state record")
}

fn canonical(value: &Json) -> Json {
    match value {
        Json::Array(items) => Json::Array(items.iter().map(canonical).collect()),
        Json::Object(items) => {
            let mut sorted = items.clone();
            sorted.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            Json::Object(
                sorted
                    .into_iter()
                    .map(|(key, item)| (key, canonical(&item)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn write_canonical(value: &Json, out: &mut String) {
    match value {
        Json::Object(items) => {
            let mut sorted: Vec<&(String, Json)> = items.iter().collect();
            sorted.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            out.push('{');
            for (index, (key, item)) in sorted.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_escaped(key, out);
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
        Json::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => other.encode(out),
    }
}

pub fn posix_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    let double = bytes.first() == Some(&b'/')
        && bytes.get(1) == Some(&b'/')
        && !matches!(bytes.get(2), Some(&b'/'));
    let mut out = String::new();
    if path.is_absolute() {
        out.push_str(if double { "//" } else { "/" });
    }
    for component in path.components() {
        let part = match component {
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            Component::ParentDir => String::from(".."),
            Component::CurDir | Component::Prefix(_) | Component::RootDir => continue,
        };
        if !out.is_empty() && !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(&part);
    }
    out
}

fn object(type_name: &str, fields: Vec<(&str, Json)>) -> Json {
    Json::Object(vec![
        (String::from("type"), Json::Str(String::from(type_name))),
        (
            String::from("fields"),
            Json::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (String::from(key), value))
                    .collect(),
            ),
        ),
    ])
}

fn tag<'a>(value: &'a Json, key: &str) -> Result<Option<&'a str>, ValueError> {
    match value.get(key) {
        Some(Json::Str(text)) => Ok(Some(text)),
        Some(Json::Array(_)) => Err(ValueError::type_error("unhashable type: 'list'")),
        Some(Json::Object(_)) => Err(ValueError::type_error("unhashable type: 'dict'")),
        _ => Ok(None),
    }
}

fn record_fields<'a>(
    value: &'a Json,
    type_name: &str,
    expected: &[&str],
) -> Result<&'a [(String, Json)], ValueError> {
    let unknown = || ValueError::new("unknown state record");
    if tag(value, "type")? != Some(type_name) {
        return Err(unknown());
    }
    if value.as_object().is_none_or(|items| items.len() != 2) {
        return Err(unknown());
    }
    let fields = value
        .get("fields")
        .and_then(Json::as_object)
        .ok_or_else(|| ValueError::new("invalid state record"))?;
    if fields.len() != expected.len()
        || !expected
            .iter()
            .all(|name| fields.iter().any(|(key, _)| key == name))
    {
        return Err(ValueError::new("state record fields do not match schema"));
    }
    Ok(fields)
}

fn field<'a>(fields: &'a [(String, Json)], name: &str) -> Result<&'a Json, ValueError> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .ok_or_else(|| ValueError::new("state record fields do not match schema"))
}

enum Decoded {
    Scalar,
    Tuple,
    Bytes(Vec<u8>),
    Path(PathBuf),
    Enum,
    Record,
    Semantic(Value),
}

const RECORD_TAGS: [&str; 21] = [
    "Claim",
    "DocumentSchema",
    "Key",
    "Member",
    "SectionBoundaries",
    "Selector",
    "ArrayPosition",
    "CreatedContainer",
    "DocumentProvenance",
    "OwnedSelection",
    "SelectionSnapshot",
    "Ancestor",
    "Node",
    "Revision",
    "ResourceRef",
    "OwnedClaim",
    "Receipt",
    "Generation",
    "Index",
    "Journal",
    "Preparation",
];

fn decoded(value: &Json) -> Result<Decoded, ValueError> {
    match value {
        Json::Null | Json::Bool(_) | Json::Int(_) | Json::BigInt(_) | Json::Str(_) => {
            Ok(Decoded::Scalar)
        }
        Json::Float(_) => Err(ValueError::new("invalid state scalar")),
        Json::Array(items) => {
            for item in items {
                decoded(item)?;
            }
            Ok(Decoded::Tuple)
        }
        Json::Object(_) => decoded_object(value),
    }
}

fn decoded_object(value: &Json) -> Result<Decoded, ValueError> {
    let unknown = || ValueError::new("unknown state record");
    let items = value.as_object().ok_or_else(unknown)?;
    let only = |name: &str| items.len() == 1 && items.first().is_some_and(|(key, _)| key == name);
    if only("value") {
        return semantic_value(value).map(Decoded::Semantic);
    }
    if only("bytes") && matches!(value.get("bytes"), Some(Json::Str(_))) {
        let text = value
            .get("bytes")
            .and_then(Json::as_str)
            .ok_or_else(unknown)?;
        return parse_hex_bytes(text).map(Decoded::Bytes);
    }
    if only("path") && matches!(value.get("path"), Some(Json::Str(_))) {
        return decode_path(value).map(Decoded::Path);
    }
    if items.len() == 2
        && items.iter().any(|(key, _)| key == "enum")
        && items.iter().any(|(key, _)| key == "value")
    {
        if let Some(tag) = tag(value, "enum")? {
            match tag {
                "Ownership" => {
                    Ownership::from_state(value)?;
                    return Ok(Decoded::Enum);
                }
                "DocumentFormat" => {
                    DocumentFormat::from_state(value)?;
                    return Ok(Decoded::Enum);
                }
                "Disposition" => {
                    Disposition::from_state(value)?;
                    return Ok(Decoded::Enum);
                }
                "ResourceKind" => {
                    ResourceKind::from_state(value)?;
                    return Ok(Decoded::Enum);
                }
                _ => {}
            }
            if RECORD_TAGS.contains(&tag) {
                return Ok(Decoded::Record);
            }
        }
        return Err(unknown());
    }
    if items.len() == 2
        && items.iter().any(|(key, _)| key == "type")
        && items.iter().any(|(key, _)| key == "fields")
    {
        if let Some(tag) = tag(value, "type")? {
            if matches!(
                tag,
                "Ownership" | "DocumentFormat" | "Disposition" | "ResourceKind"
            ) {
                return Err(ValueError::new("invalid state record"));
            }
            if decoded_record(tag, value)? {
                return Ok(Decoded::Record);
            }
        }
        return Err(unknown());
    }
    Err(unknown())
}

fn decoded_record(tag: &str, value: &Json) -> Result<bool, ValueError> {
    Ok(match tag {
        "Claim" => {
            Claim::from_state(value)?;
            true
        }
        "DocumentSchema" => {
            DocumentSchema::from_state(value)?;
            true
        }
        "Key" => {
            Key::from_state(value)?;
            true
        }
        "Member" => {
            Member::from_state(value)?;
            true
        }
        "SectionBoundaries" => {
            SectionBoundaries::from_state(value)?;
            true
        }
        "Selector" => {
            Selector::from_state(value)?;
            true
        }
        "ArrayPosition" => {
            ArrayPosition::from_state(value)?;
            true
        }
        "CreatedContainer" => {
            CreatedContainer::from_state(value)?;
            true
        }
        "DocumentProvenance" => {
            DocumentProvenance::from_state(value)?;
            true
        }
        "OwnedSelection" => {
            OwnedSelection::from_state(value)?;
            true
        }
        "SelectionSnapshot" => {
            SelectionSnapshot::from_state(value)?;
            true
        }
        "Ancestor" => {
            Ancestor::from_state(value)?;
            true
        }
        "Node" => {
            Node::from_state(value)?;
            true
        }
        "Revision" => {
            Revision::from_state(value)?;
            true
        }
        "ResourceRef" => {
            ResourceRef::from_state(value)?;
            true
        }
        "OwnedClaim" => {
            OwnedClaim::from_state(value)?;
            true
        }
        "Receipt" => {
            Receipt::from_state(value)?;
            true
        }
        "Generation" => {
            Generation::from_state(value)?;
            true
        }
        "Index" => {
            Index::from_state(value)?;
            true
        }
        "Journal" => {
            Journal::from_state(value)?;
            true
        }
        "Preparation" => {
            Preparation::from_state(value)?;
            true
        }
        _ => false,
    })
}

fn decode_fields(fields: &[(String, Json)]) -> Result<(), ValueError> {
    fields
        .iter()
        .try_for_each(|(_, item)| decoded(item).map(drop))
}

fn wrong_kind(value: &Json, wrong: ValueError) -> ValueError {
    match decoded(value) {
        Ok(_) => wrong,
        Err(raw) => raw,
    }
}

fn classified<T: StateCodec>(value: &Json, wrong: ValueError) -> Result<T, ValueError> {
    T::from_state(value).map_err(|_| wrong_kind(value, wrong))
}

fn expect_bytes(value: &Json, wrong: ValueError) -> Result<Vec<u8>, ValueError> {
    match decoded(value) {
        Ok(Decoded::Bytes(data)) => Ok(data),
        Ok(_) => Err(wrong),
        Err(raw) => Err(raw),
    }
}

fn expect_path(value: &Json, wrong: ValueError) -> Result<PathBuf, ValueError> {
    match decoded(value) {
        Ok(Decoded::Path(path)) => Ok(path),
        Ok(_) => Err(wrong),
        Err(raw) => Err(raw),
    }
}

fn text_like(value: &Json, wrong: ValueError) -> Result<String, ValueError> {
    match value {
        Json::Str(text) => Ok(text.clone()),
        _ => Err(wrong_kind(value, wrong)),
    }
}

fn semantic(value: &Json, wrong: ValueError) -> Result<Value, ValueError> {
    match decoded(value) {
        Ok(Decoded::Semantic(item)) => Ok(item),
        Ok(_) => Err(wrong),
        Err(raw) => Err(raw),
    }
}

fn scalar_semantic(value: &Json, wrong: ValueError) -> Result<Scalar, ValueError> {
    match decoded(value) {
        Ok(Decoded::Semantic(Value::Scalar(scalar))) => Ok(scalar),
        Ok(_) => Err(wrong),
        Err(raw) => Err(raw),
    }
}

fn metadata(value: &Json, wrong: ValueError) -> Result<Count, ValueError> {
    match value {
        Json::Int(item) => Ok(Count::Small(*item)),
        Json::BigInt(text) => Ok(Count::Big(text.clone())),
        _ => Err(wrong_kind(value, wrong)),
    }
}

fn schema_version(value: &Json, wrong: ValueError) -> Result<i64, ValueError> {
    match value {
        Json::Int(item) => Ok(*item),
        Json::BigInt(_) => Err(wrong),
        _ => Err(wrong_kind(value, wrong)),
    }
}

fn sequence<T>(
    value: &Json,
    decode_item: impl Fn(&Json) -> Result<T, ValueError>,
) -> Result<Vec<T>, ValueError> {
    let Some(items) = value.as_array() else {
        return match decoded(value) {
            Ok(_) => Err(ValueError::new("invalid state record")),
            Err(raw) => Err(raw),
        };
    };
    items.iter().map(decode_item).collect()
}

fn optional<T>(
    value: &Json,
    decode_item: impl Fn(&Json) -> Result<T, ValueError>,
) -> Result<Option<T>, ValueError> {
    if value.is_null() {
        return Ok(None);
    }
    decode_item(value).map(Some)
}

fn optional_state<T: StateCodec>(value: Option<&T>) -> Json {
    value.map_or(Json::Null, StateCodec::to_state)
}

fn optional_bytes(value: Option<&[u8]>) -> Json {
    value.map_or(Json::Null, encoded_bytes)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_bytes(text: &str) -> Result<Vec<u8>, ValueError> {
    let invalid = |position: usize| {
        ValueError::new(format!(
            "non-hexadecimal number found in fromhex() arg at position {position}"
        ))
    };
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    loop {
        while matches!(
            bytes.get(index),
            Some(b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
        ) {
            index += 1;
        }
        let Some(first) = bytes.get(index).copied() else {
            return Ok(out);
        };
        let high = hex_nibble(first).ok_or_else(|| invalid(index))?;
        index += 1;
        let second = bytes.get(index).copied().ok_or_else(|| invalid(index))?;
        let low = hex_nibble(second).ok_or_else(|| invalid(index))?;
        index += 1;
        out.push(high * 16 + low);
    }
}

pub fn encoded_bytes(data: &[u8]) -> Json {
    let mut hex = String::with_capacity(data.len() * 2);
    for byte in data {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Json::Object(vec![(String::from("bytes"), Json::Str(hex))])
}

fn decode_path(value: &Json) -> Result<PathBuf, ValueError> {
    let invalid = || ValueError::new("unknown state record");
    let items = value.as_object().ok_or_else(invalid)?;
    let Some((key, text)) = items.first() else {
        return Err(invalid());
    };
    if items.len() != 1 || key != "path" {
        return Err(invalid());
    }
    let text = text.as_str().ok_or_else(invalid)?;
    let path = PathBuf::from(text);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(ValueError::new(
            "state paths must be absolute without traversal",
        ));
    }
    Ok(path)
}

pub fn encoded_path(path: &Path) -> Json {
    Json::Object(vec![(String::from("path"), Json::Str(posix_path(path)))])
}

fn semantic_value(value: &Json) -> Result<Value, ValueError> {
    let invalid = || ValueError::new("unknown state record");
    let items = value.as_object().ok_or_else(invalid)?;
    if items.len() != 1 || items.first().is_none_or(|(key, _)| key != "value") {
        return Err(invalid());
    }
    let record = items.first().map(|(_, item)| item).ok_or_else(invalid)?;
    value_from_record(record)
}

fn encoded_semantic(value: &Value) -> Json {
    Json::Object(vec![(String::from("value"), semantic_record(value))])
}

fn enum_text(value: &Json, enum_name: &str) -> Result<String, ValueError> {
    let unknown = || ValueError::new("unknown state record");
    let items = value.as_object().ok_or_else(unknown)?;
    if items.len() != 2 {
        return Err(unknown());
    }
    if tag(value, "enum")? != Some(enum_name) {
        return Err(unknown());
    }
    match value.get("value") {
        Some(Json::Str(text)) => Ok(text.clone()),
        Some(other) => Err(ValueError::new(format!(
            "{} is not a valid {enum_name}",
            crate::json::python_repr(other)
        ))),
        None => Err(unknown()),
    }
}

fn encoded_enum(enum_name: &str, value: &str) -> Json {
    Json::Object(vec![
        (String::from("enum"), Json::Str(String::from(enum_name))),
        (String::from("value"), Json::Str(String::from(value))),
    ])
}

fn requirement(value: &Json) -> Result<(PathBuf, String), ValueError> {
    let Some(pair) = value.as_array() else {
        return match decoded(value) {
            Ok(_) => Err(ValueError::new("invalid state record")),
            Err(raw) => Err(raw),
        };
    };
    for item in pair {
        decoded(item)?;
    }
    if pair.len() > 2 {
        return Err(ValueError::new("too many values to unpack (expected 2)"));
    }
    if pair.len() < 2 {
        return Err(ValueError::new(format!(
            "not enough values to unpack (expected 2, got {})",
            pair.len()
        )));
    }
    match pair {
        [first, second] => Ok((
            expect_path(first, ValueError::new("expected an absolute concrete path"))?,
            text_like(second, ValueError::new("invalid hexadecimal identity"))?,
        )),
        _ => Err(ValueError::new("invalid state record")),
    }
}

fn encoded_requirement((path, identifier): &(PathBuf, String)) -> Json {
    Json::Array(vec![encoded_path(path), Json::Str(identifier.clone())])
}

fn generation_member(value: &Json) -> Result<(ResourceRef, Vec<String>), ValueError> {
    let Some(pair) = value.as_array() else {
        return match decoded(value) {
            Ok(_) => Err(ValueError::new("invalid state record")),
            Err(raw) => Err(raw),
        };
    };
    for item in pair {
        decoded(item)?;
    }
    if pair.len() > 2 {
        return Err(ValueError::new("too many values to unpack (expected 2)"));
    }
    if pair.len() < 2 {
        return Err(ValueError::new(format!(
            "not enough values to unpack (expected 2, got {})",
            pair.len()
        )));
    }
    match pair {
        [first, second] => Ok((
            classified::<ResourceRef>(
                first,
                ValueError::type_error("generation requires resource references"),
            )?,
            sequence(second, |item| {
                text_like(item, ValueError::new("invalid hexadecimal identity"))
            })?,
        )),
        _ => Err(ValueError::new("invalid state record")),
    }
}

fn selector_part(value: &Json) -> Result<SelectorPart, ValueError> {
    match value.get("type").and_then(Json::as_str) {
        Some("Key") => Ok(SelectorPart::Key(Key::from_state(value)?)),
        Some("Member") => Ok(SelectorPart::Member(Member::from_state(value)?)),
        _ => match decoded(value) {
            Ok(_) => Err(ValueError::new("invalid state record")),
            Err(raw) => Err(raw),
        },
    }
}

fn claim_selector(value: &Json, kind: Ownership) -> Result<ClaimSelector, ValueError> {
    if value.is_null() {
        return Ok(ClaimSelector::Whole);
    }
    if let Some(path) = value.as_str() {
        return Ok(ClaimSelector::Subtree(path.to_owned()));
    }
    match value.get("type").and_then(Json::as_str) {
        Some("SectionBoundaries") => Ok(ClaimSelector::Section(SectionBoundaries::from_state(
            value,
        )?)),
        Some("Selector") => Ok(ClaimSelector::Structured(Selector::from_state(value)?)),
        _ => Err(wrong_kind(
            value,
            match kind {
                Ownership::File | Ownership::Tree => {
                    ValueError::new("whole-resource claims cannot have selectors")
                }
                Ownership::Structured => {
                    ValueError::type_error("structured claims require a Selector")
                }
                Ownership::Section => ValueError::type_error(
                    "section claims require SectionBoundaries or a marker name",
                ),
                Ownership::Subtree => {
                    ValueError::type_error("subtree claims require a string selector")
                }
            },
        )),
    }
}

fn snapshot_value(value: &Json) -> Result<SnapshotValue, ValueError> {
    match decoded(value) {
        Ok(Decoded::Bytes(data)) => Ok(SnapshotValue::Bytes(data)),
        Ok(Decoded::Semantic(item)) => Ok(SnapshotValue::Typed(item)),
        Ok(_) => Err(ValueError::new("invalid state record")),
        Err(raw) => Err(raw),
    }
}

impl StateCodec for Ownership {
    fn to_state(&self) -> Json {
        encoded_enum("Ownership", self.as_str())
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        Self::from_str(&enum_text(value, "Ownership")?)
    }
}

impl StateCodec for Disposition {
    fn to_state(&self) -> Json {
        encoded_enum("Disposition", self.as_str())
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        Self::from_str(&enum_text(value, "Disposition")?)
    }
}

impl StateCodec for DocumentFormat {
    fn to_state(&self) -> Json {
        encoded_enum("DocumentFormat", self.as_str())
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        Self::from_str(&enum_text(value, "DocumentFormat")?)
    }
}

impl StateCodec for ResourceKind {
    fn to_state(&self) -> Json {
        encoded_enum("ResourceKind", self.as_str())
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        Self::from_str(&enum_text(value, "ResourceKind")?)
    }
}

impl StateCodec for Key {
    fn to_state(&self) -> Json {
        object("Key", vec![("name", Json::Str(self.name().to_owned()))])
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "Key", &["name"])?;
        decode_fields(fields)?;
        Self::new(text_like(
            field(fields, "name")?,
            ValueError::type_error("selector key must be a string"),
        )?)
    }
}

impl StateCodec for Member {
    fn to_state(&self) -> Json {
        object(
            "Member",
            vec![
                ("identity", encoded_semantic(self.identity())),
                (
                    "key",
                    Json::Array(
                        self.key()
                            .iter()
                            .map(|name| Json::Str(name.clone()))
                            .collect(),
                    ),
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "Member", &["identity", "key"])?;
        decode_fields(fields)?;
        Self::new(
            semantic(
                field(fields, "identity")?,
                ValueError::type_error("values must be Scalar, ObjectValue or ArrayValue"),
            )?,
            sequence(field(fields, "key")?, |item| {
                text_like(
                    item,
                    ValueError::type_error("selector key must be a string"),
                )
            })?,
        )
    }
}

impl StateCodec for SectionBoundaries {
    fn to_state(&self) -> Json {
        object(
            "SectionBoundaries",
            vec![
                ("end", Json::Str(self.end().to_owned())),
                ("start", Json::Str(self.start().to_owned())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "SectionBoundaries", &["start", "end"])?;
        decode_fields(fields)?;
        Self::new(
            text_like(
                field(fields, "start")?,
                ValueError::type_error("section boundary must be a string"),
            )?,
            text_like(
                field(fields, "end")?,
                ValueError::type_error("section boundary must be a string"),
            )?,
        )
    }
}

impl StateCodec for Selector {
    fn to_state(&self) -> Json {
        object(
            "Selector",
            vec![(
                "parts",
                Json::Array(
                    self.parts()
                        .iter()
                        .map(|part| match part {
                            SelectorPart::Key(key) => key.to_state(),
                            SelectorPart::Member(member) => member.to_state(),
                        })
                        .collect(),
                ),
            )],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "Selector", &["parts"])?;
        decode_fields(fields)?;
        Self::new(sequence(field(fields, "parts")?, selector_part)?)
    }
}

impl StateCodec for DocumentSchema {
    fn to_state(&self) -> Json {
        object(
            "DocumentSchema",
            vec![
                ("format", self.format().to_state()),
                ("key", Json::Str(self.key().to_owned())),
                (
                    "value",
                    encoded_semantic(&Value::Scalar(self.value().clone())),
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "DocumentSchema", &["format", "key", "value"])?;
        decode_fields(fields)?;
        Self::new(
            classified::<DocumentFormat>(
                field(fields, "format")?,
                ValueError::new("document schema scaffolds require JSON or JSONC"),
            )?,
            text_like(
                field(fields, "key")?,
                ValueError::type_error("selector key must be a string"),
            )?,
            scalar_semantic(
                field(fields, "value")?,
                ValueError::type_error("document schema fields require a scalar value"),
            )?,
        )
    }
}

impl StateCodec for ArrayPosition {
    fn to_state(&self) -> Json {
        object(
            "ArrayPosition",
            vec![
                ("following", optional_state(self.following())),
                ("prefix", encoded_bytes(self.prefix())),
                ("previous", optional_state(self.previous())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "ArrayPosition", &["previous", "following", "prefix"])?;
        decode_fields(fields)?;
        let anchor = |item: &Json| {
            classified::<Member>(
                item,
                ValueError::type_error("array position anchors must be Members"),
            )
        };
        Self::new(
            optional(field(fields, "previous")?, anchor)?,
            optional(field(fields, "following")?, anchor)?,
            expect_bytes(
                field(fields, "prefix")?,
                ValueError::type_error("array position prefix must be immutable bytes"),
            )?,
        )
    }
}

impl StateCodec for SelectionSnapshot {
    fn to_state(&self) -> Json {
        object(
            "SelectionSnapshot",
            vec![
                ("layout", encoded_bytes(self.layout())),
                ("owned_syntax", encoded_bytes(self.owned_syntax())),
                ("position", optional_state(self.position())),
                ("syntax", encoded_bytes(self.syntax())),
                (
                    "value",
                    match self.value() {
                        SnapshotValue::Bytes(data) => encoded_bytes(data),
                        SnapshotValue::Typed(value) => encoded_semantic(value),
                    },
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "SelectionSnapshot",
            &["value", "syntax", "owned_syntax", "position", "layout"],
        )?;
        decode_fields(fields)?;
        let syntax = |item: &Json| {
            expect_bytes(
                item,
                ValueError::type_error("selection syntax must be immutable bytes"),
            )
        };
        Self::new(
            snapshot_value(field(fields, "value")?)?,
            syntax(field(fields, "syntax")?)?,
            syntax(field(fields, "owned_syntax")?)?,
            optional(field(fields, "position")?, |item| {
                classified::<ArrayPosition>(
                    item,
                    ValueError::type_error("selection position must be an ArrayPosition"),
                )
            })?,
            syntax(field(fields, "layout")?)?,
        )
    }
}

impl StateCodec for OwnedSelection {
    fn to_state(&self) -> Json {
        object(
            "OwnedSelection",
            vec![
                ("baseline", optional_state(self.baseline())),
                ("claim", self.claim().to_state()),
                ("disposition", self.disposition().to_state()),
                ("format", optional_state(self.format().as_ref())),
                ("installed", self.installed().to_state()),
                ("separator", encoded_bytes(self.separator())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "OwnedSelection",
            &[
                "claim",
                "format",
                "installed",
                "disposition",
                "baseline",
                "separator",
            ],
        )?;
        decode_fields(fields)?;
        Self::new(
            classified::<Claim>(
                field(fields, "claim")?,
                ValueError::type_error("ownership requires a Claim"),
            )?,
            optional(field(fields, "format")?, |item| {
                classified::<DocumentFormat>(
                    item,
                    ValueError::new("shared claim and document format disagree"),
                )
            })?,
            classified::<SelectionSnapshot>(
                field(fields, "installed")?,
                ValueError::type_error("ownership requires an installed snapshot and disposition"),
            )?,
            classified::<Disposition>(
                field(fields, "disposition")?,
                ValueError::type_error("ownership requires an installed snapshot and disposition"),
            )?,
            optional(field(fields, "baseline")?, |item| {
                classified::<SelectionSnapshot>(
                    item,
                    ValueError::type_error("takeover baseline must be a SelectionSnapshot"),
                )
            })?,
            expect_bytes(
                field(fields, "separator")?,
                ValueError::type_error("owned separators must be immutable bytes"),
            )?,
        )
    }
}

impl StateCodec for CreatedContainer {
    fn to_state(&self) -> Json {
        object(
            "CreatedContainer",
            vec![
                ("empty_syntax", encoded_bytes(self.empty_syntax())),
                ("selector", self.selector().to_state()),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "CreatedContainer", &["selector", "empty_syntax"])?;
        decode_fields(fields)?;
        let wrong = || {
            ValueError::type_error("container provenance requires a selector and immutable syntax")
        };
        Ok(Self::new(
            classified::<Selector>(field(fields, "selector")?, wrong())?,
            expect_bytes(field(fields, "empty_syntax")?, wrong())?,
        ))
    }
}

impl StateCodec for DocumentProvenance {
    fn to_state(&self) -> Json {
        object(
            "DocumentProvenance",
            vec![
                (
                    "containers",
                    Json::Array(self.containers().iter().map(StateCodec::to_state).collect()),
                ),
                ("empty_document", optional_bytes(self.empty_document())),
                (
                    "schema_empty_document",
                    optional_bytes(self.schema_empty_document()),
                ),
                (
                    "schema_fields",
                    Json::Array(
                        self.schema_fields()
                            .iter()
                            .map(StateCodec::to_state)
                            .collect(),
                    ),
                ),
                (
                    "trailing_separators",
                    Json::Array(
                        self.trailing_separators()
                            .iter()
                            .map(|digest| Json::Str(digest.clone()))
                            .collect(),
                    ),
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "DocumentProvenance",
            &[
                "empty_document",
                "containers",
                "trailing_separators",
                "schema_fields",
                "schema_empty_document",
            ],
        )?;
        decode_fields(fields)?;
        Self::new(
            optional(field(fields, "empty_document")?, |item| {
                expect_bytes(
                    item,
                    ValueError::type_error("created document evidence must be immutable bytes"),
                )
            })?,
            sequence(field(fields, "containers")?, |item| {
                classified::<CreatedContainer>(
                    item,
                    ValueError::type_error(
                        "container provenance must contain CreatedContainer values",
                    ),
                )
            })?,
            sequence(field(fields, "trailing_separators")?, |item| {
                text_like(
                    item,
                    ValueError::new("separator evidence must be SHA-256 digests"),
                )
            })?,
            sequence(field(fields, "schema_fields")?, |item| {
                classified::<OwnedSelection>(
                    item,
                    ValueError::new("schema provenance requires created top-level scalar fields"),
                )
            })?,
            optional(field(fields, "schema_empty_document")?, |item| {
                expect_bytes(
                    item,
                    ValueError::new("schema empty-document evidence must be whitespace bytes"),
                )
            })?,
        )
    }
}

impl StateCodec for Claim {
    fn to_state(&self) -> Json {
        object(
            "Claim",
            vec![
                ("kind", self.kind().to_state()),
                (
                    "selector",
                    match self.selector() {
                        ClaimSelector::Whole => Json::Null,
                        ClaimSelector::Subtree(path) => Json::Str(path.clone()),
                        ClaimSelector::Section(boundaries) => boundaries.to_state(),
                        ClaimSelector::Structured(selector) => selector.to_state(),
                    },
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "Claim", &["kind", "selector"])?;
        decode_fields(fields)?;
        let kind = classified::<Ownership>(
            field(fields, "kind")?,
            ValueError::type_error("claim kind must be an Ownership"),
        )?;
        Self::new(kind, claim_selector(field(fields, "selector")?, kind)?)
    }
}

impl StateCodec for Ancestor {
    fn to_state(&self) -> Json {
        object(
            "Ancestor",
            vec![
                ("device", self.device().to_json()),
                ("gid", self.gid().to_json()),
                ("inode", self.inode().to_json()),
                ("mode", self.mode().to_json()),
                ("path", encoded_path(self.path())),
                ("security", encoded_bytes(self.security())),
                ("uid", self.uid().to_json()),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Ancestor",
            &["path", "device", "inode", "mode", "uid", "gid", "security"],
        )?;
        decode_fields(fields)?;
        let count = |item: &Json| metadata(item, ValueError::new("invalid ancestor metadata"));
        Self::new(
            expect_path(
                field(fields, "path")?,
                ValueError::new("ancestor path must be absolute"),
            )?,
            count(field(fields, "device")?)?,
            count(field(fields, "inode")?)?,
            count(field(fields, "mode")?)?,
            count(field(fields, "uid")?)?,
            count(field(fields, "gid")?)?,
            expect_bytes(
                field(fields, "security")?,
                ValueError::type_error("ancestor security must be immutable"),
            )?,
        )
    }
}

impl StateCodec for Node {
    fn to_state(&self) -> Json {
        object(
            "Node",
            vec![
                ("data", optional_bytes(self.data())),
                ("device", self.device().to_json()),
                ("gid", self.gid().to_json()),
                ("inode", self.inode().to_json()),
                ("mode", self.mode().to_json()),
                ("path", Json::Str(self.path().to_owned())),
                ("security", encoded_bytes(self.security())),
                ("uid", self.uid().to_json()),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Node",
            &[
                "path", "data", "mode", "uid", "gid", "device", "inode", "security",
            ],
        )?;
        decode_fields(fields)?;
        let count = |item: &Json| metadata(item, ValueError::new("invalid revision metadata"));
        Self::new(
            text_like(
                field(fields, "path")?,
                ValueError::type_error("revision path must be a string"),
            )?,
            optional(field(fields, "data")?, |item| {
                expect_bytes(
                    item,
                    ValueError::type_error("revision bytes must be immutable"),
                )
            })?,
            count(field(fields, "mode")?)?,
            count(field(fields, "uid")?)?,
            count(field(fields, "gid")?)?,
            count(field(fields, "device")?)?,
            count(field(fields, "inode")?)?,
            expect_bytes(
                field(fields, "security")?,
                ValueError::type_error("security metadata must be immutable"),
            )?,
        )
    }
}

impl StateCodec for Revision {
    fn to_state(&self) -> Json {
        object(
            "Revision",
            vec![(
                "nodes",
                Json::Array(self.nodes().iter().map(StateCodec::to_state).collect()),
            )],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(value, "Revision", &["nodes"])?;
        decode_fields(fields)?;
        Self::new(sequence(field(fields, "nodes")?, |item| {
            classified::<Node>(item, ValueError::type_error("expected Node values"))
        })?)
    }
}

impl StateCodec for ResourceRef {
    fn to_state(&self) -> Json {
        object(
            "ResourceRef",
            vec![
                ("destination", encoded_path(self.destination())),
                ("kind", self.kind().to_state()),
                ("schema_version", Json::Int(self.schema_version())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "ResourceRef",
            &["destination", "kind", "schema_version"],
        )?;
        decode_fields(fields)?;
        Self::with_schema(
            expect_path(
                field(fields, "destination")?,
                ValueError::new("expected an absolute concrete path"),
            )?,
            classified::<ResourceKind>(
                field(fields, "kind")?,
                ValueError::type_error("resource kind must be a ResourceKind"),
            )?,
            schema_version(
                field(fields, "schema_version")?,
                ValueError::new("authority schema must be 2"),
            )?,
        )
    }
}

impl StateCodec for OwnedClaim {
    fn to_state(&self) -> Json {
        object(
            "OwnedClaim",
            vec![
                ("artifact_id", Json::Str(self.artifact_id().to_owned())),
                ("baseline", optional_state(self.baseline())),
                ("bundle_digest", Json::Str(self.bundle_digest().to_owned())),
                ("bundle_id", Json::Str(self.bundle_id().to_owned())),
                ("claim", self.claim().to_state()),
                ("claim_id", Json::Str(self.claim_id().to_owned())),
                ("disposition", self.disposition().to_state()),
                ("installed", optional_state(self.installed())),
                ("render_digest", Json::Str(self.render_digest().to_owned())),
                (
                    "requirements",
                    Json::Array(
                        self.requirements()
                            .iter()
                            .map(encoded_requirement)
                            .collect(),
                    ),
                ),
                (
                    "schema_requirements",
                    Json::Array(
                        self.schema_requirements()
                            .iter()
                            .map(StateCodec::to_state)
                            .collect(),
                    ),
                ),
                ("selection", optional_state(self.selection())),
                (
                    "stable_requirements",
                    Json::Array(
                        self.stable_requirements()
                            .iter()
                            .map(encoded_requirement)
                            .collect(),
                    ),
                ),
                ("version", Json::Str(self.version().to_owned())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "OwnedClaim",
            &[
                "claim",
                "claim_id",
                "bundle_id",
                "artifact_id",
                "version",
                "bundle_digest",
                "render_digest",
                "selection",
                "installed",
                "disposition",
                "baseline",
                "requirements",
                "stable_requirements",
                "schema_requirements",
            ],
        )?;
        decode_fields(fields)?;
        let hexadecimal =
            |item: &Json| text_like(item, ValueError::new("invalid hexadecimal identity"));
        let revision = |item: &Json| {
            classified::<Revision>(item, ValueError::type_error("invalid state record"))
        };
        Self::new(
            classified::<Claim>(
                field(fields, "claim")?,
                ValueError::type_error("ownership requires a claim and disposition"),
            )?,
            hexadecimal(field(fields, "claim_id")?)?,
            text_like(
                field(fields, "bundle_id")?,
                ValueError::type_error("owner must be a string"),
            )?,
            text_like(
                field(fields, "artifact_id")?,
                ValueError::type_error("artifact must be a string"),
            )?,
            text_like(
                field(fields, "version")?,
                ValueError::type_error("version must be a string"),
            )?,
            hexadecimal(field(fields, "bundle_digest")?)?,
            hexadecimal(field(fields, "render_digest")?)?,
            optional(field(fields, "selection")?, |item| {
                classified::<OwnedSelection>(item, ValueError::type_error("invalid state record"))
            })?,
            optional(field(fields, "installed")?, revision)?,
            classified::<Disposition>(
                field(fields, "disposition")?,
                ValueError::type_error("ownership requires a claim and disposition"),
            )?,
            optional(field(fields, "baseline")?, revision)?,
            sequence(field(fields, "requirements")?, requirement)?,
            sequence(field(fields, "stable_requirements")?, requirement)?,
            sequence(field(fields, "schema_requirements")?, |item| {
                classified::<DocumentSchema>(
                    item,
                    ValueError::type_error("expected DocumentSchema values"),
                )
            })?,
        )
    }
}

impl StateCodec for Receipt {
    fn to_state(&self) -> Json {
        object(
            "Receipt",
            vec![
                (
                    "claims",
                    Json::Array(self.claims().iter().map(StateCodec::to_state).collect()),
                ),
                (
                    "created_directories",
                    Json::Array(
                        self.created_directories()
                            .iter()
                            .map(StateCodec::to_state)
                            .collect(),
                    ),
                ),
                ("provenance", self.provenance().to_state()),
                ("resource", self.resource().to_state()),
                ("schema_version", Json::Int(self.schema_version())),
                (
                    "transaction_id",
                    Json::Str(self.transaction_id().to_owned()),
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Receipt",
            &[
                "resource",
                "transaction_id",
                "claims",
                "provenance",
                "created_directories",
                "schema_version",
            ],
        )?;
        decode_fields(fields)?;
        let wrong = || ValueError::type_error("invalid receipt resource or provenance");
        Self::new(
            classified::<ResourceRef>(field(fields, "resource")?, wrong())?,
            text_like(
                field(fields, "transaction_id")?,
                ValueError::new("invalid hexadecimal identity"),
            )?,
            sequence(field(fields, "claims")?, |item| {
                classified::<OwnedClaim>(item, ValueError::type_error("expected OwnedClaim values"))
            })?,
            classified::<DocumentProvenance>(field(fields, "provenance")?, wrong())?,
            sequence(field(fields, "created_directories")?, |item| {
                classified::<Node>(item, ValueError::type_error("expected Node values"))
            })?,
            schema_version(
                field(fields, "schema_version")?,
                ValueError::new("receipt schema must be 2"),
            )?,
        )
    }
}

impl StateCodec for Generation {
    fn to_state(&self) -> Json {
        object(
            "Generation",
            vec![
                ("bundle_digest", Json::Str(self.bundle_digest().to_owned())),
                (
                    "membership",
                    Json::Array(
                        self.membership()
                            .iter()
                            .map(|(reference, identifiers)| {
                                Json::Array(vec![
                                    reference.to_state(),
                                    Json::Array(
                                        identifiers
                                            .iter()
                                            .map(|identifier| Json::Str(identifier.clone()))
                                            .collect(),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("render_digest", Json::Str(self.render_digest().to_owned())),
                ("version", Json::Str(self.version().to_owned())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Generation",
            &["version", "bundle_digest", "render_digest", "membership"],
        )?;
        decode_fields(fields)?;
        let hexadecimal =
            |item: &Json| text_like(item, ValueError::new("invalid hexadecimal identity"));
        Self::new(
            text_like(
                field(fields, "version")?,
                ValueError::type_error("version must be a string"),
            )?,
            hexadecimal(field(fields, "bundle_digest")?)?,
            hexadecimal(field(fields, "render_digest")?)?,
            sequence(field(fields, "membership")?, generation_member)?,
        )
    }
}

impl StateCodec for Index {
    fn to_state(&self) -> Json {
        object(
            "Index",
            vec![
                ("bundle_id", Json::Str(self.bundle_id().to_owned())),
                (
                    "context_digest",
                    Json::Str(self.context_digest().to_owned()),
                ),
                ("current", optional_state(self.current())),
                ("pending", optional_state(self.pending())),
                ("previous", optional_state(self.previous())),
                (
                    "residual",
                    Json::Array(self.residual().iter().map(StateCodec::to_state).collect()),
                ),
                ("schema_version", Json::Int(self.schema_version())),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Index",
            &[
                "bundle_id",
                "context_digest",
                "current",
                "pending",
                "previous",
                "residual",
                "schema_version",
            ],
        )?;
        decode_fields(fields)?;
        let generation = |item: &Json| {
            classified::<Generation>(
                item,
                ValueError::type_error("index requires generation records"),
            )
        };
        Self::new(
            text_like(
                field(fields, "bundle_id")?,
                ValueError::type_error("index owner must be a string"),
            )?,
            text_like(
                field(fields, "context_digest")?,
                ValueError::new("invalid hexadecimal identity"),
            )?,
            optional(field(fields, "current")?, generation)?,
            optional(field(fields, "pending")?, generation)?,
            optional(field(fields, "previous")?, generation)?,
            sequence(field(fields, "residual")?, |item| {
                classified::<ResourceRef>(
                    item,
                    ValueError::type_error("expected ResourceRef values"),
                )
            })?,
            schema_version(
                field(fields, "schema_version")?,
                ValueError::new("index schema must be 2"),
            )?,
        )
    }
}

impl StateCodec for Journal {
    fn to_state(&self) -> Json {
        object(
            "Journal",
            vec![
                (
                    "ancestors",
                    Json::Array(self.ancestors().iter().map(StateCodec::to_state).collect()),
                ),
                ("backup", self.backup().to_state()),
                ("before", self.before().to_state()),
                ("previous", optional_state(self.previous())),
                ("published", self.published().to_state()),
                ("receipt", self.receipt().to_state()),
                ("resource", self.resource().to_state()),
                ("schema_version", Json::Int(self.schema_version())),
                ("staged", self.staged().to_state()),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Journal",
            &[
                "resource",
                "previous",
                "receipt",
                "before",
                "staged",
                "published",
                "backup",
                "ancestors",
                "schema_version",
            ],
        )?;
        decode_fields(fields)?;
        let wrong = || ValueError::type_error("journal requires resource and receipt");
        let revision = |item: &Json| {
            classified::<Revision>(item, ValueError::type_error("expected Revision values"))
        };
        Self::new(
            classified::<ResourceRef>(field(fields, "resource")?, wrong())?,
            optional(field(fields, "previous")?, |item| {
                classified::<Receipt>(
                    item,
                    ValueError::type_error("journal previous receipt is invalid"),
                )
            })?,
            classified::<Receipt>(field(fields, "receipt")?, wrong())?,
            revision(field(fields, "before")?)?,
            revision(field(fields, "staged")?)?,
            revision(field(fields, "published")?)?,
            revision(field(fields, "backup")?)?,
            sequence(field(fields, "ancestors")?, |item| {
                classified::<Ancestor>(item, ValueError::type_error("expected Ancestor values"))
            })?,
            schema_version(
                field(fields, "schema_version")?,
                ValueError::new("journal schema must be 2"),
            )?,
        )
    }
}

impl StateCodec for Preparation {
    fn to_state(&self) -> Json {
        object(
            "Preparation",
            vec![
                ("directory", encoded_path(self.directory())),
                ("resource", self.resource().to_state()),
                ("schema_version", Json::Int(self.schema_version())),
                (
                    "transaction_id",
                    Json::Str(self.transaction_id().to_owned()),
                ),
            ],
        )
    }

    fn from_state(value: &Json) -> Result<Self, ValueError> {
        let fields = record_fields(
            value,
            "Preparation",
            &["resource", "transaction_id", "directory", "schema_version"],
        )?;
        decode_fields(fields)?;
        Self::with_schema(
            classified::<ResourceRef>(
                field(fields, "resource")?,
                ValueError::type_error("preparation requires a resource"),
            )?,
            text_like(
                field(fields, "transaction_id")?,
                ValueError::new("invalid hexadecimal identity"),
            )?,
            expect_path(
                field(fields, "directory")?,
                ValueError::new("expected an absolute concrete path"),
            )?,
            schema_version(
                field(fields, "schema_version")?,
                ValueError::new("preparation schema must be 2"),
            )?,
        )
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
    use crate::resource_models::Receipt as ReceiptModel;
    use crate::resource_transaction::Journal as JournalModel;
    use crate::test_support::fixture;

    fn set_field(value: &Json, key: &str, replacement: &Json) -> Json {
        match value {
            Json::Object(items) => Json::Object(
                items
                    .iter()
                    .map(|(name, item)| {
                        if name == key {
                            (name.clone(), replacement.clone())
                        } else {
                            (name.clone(), item.clone())
                        }
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn relocated(raw: &Json, root: &std::path::Path) -> Result<Json, ValueError> {
        match raw {
            Json::Array(items) => Ok(Json::Array(
                items
                    .iter()
                    .map(|item| relocated(item, root))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Json::Object(items) if items.len() == 1 && items[0].0 == "path" => {
                let original = items[0]
                    .1
                    .as_str()
                    .ok_or_else(|| ValueError::new("invalid state record"))?;
                let name = original.rsplit('/').next().unwrap_or("");
                Ok(encoded_path(&root.join(name)))
            }
            Json::Object(items) => {
                let mut result = Vec::new();
                for (key, value) in items {
                    result.push((key.clone(), relocated(value, root)?));
                }
                let mut record = Json::Object(result);
                if record.get("type").and_then(Json::as_str) == Some("Receipt") {
                    let fields = record
                        .get("fields")
                        .and_then(Json::as_object)
                        .expect("fixture fields");
                    let resource_record = fields
                        .iter()
                        .find(|(key, _)| key == "resource")
                        .map(|(_, value)| value)
                        .expect("resource");
                    let resource = decode::<ResourceRef>(&encode_state(resource_record))?;
                    let authority = resource.authority()?;
                    let claims = fields
                        .iter()
                        .find(|(key, _)| key == "claims")
                        .map(|(_, value)| value)
                        .and_then(Json::as_array)
                        .expect("claims");
                    let mut rewritten = Vec::new();
                    for claim in claims {
                        let claim_fields = claim
                            .get("fields")
                            .and_then(Json::as_object)
                            .expect("claim fields");
                        let claim_record = claim_fields
                            .iter()
                            .find(|(key, _)| key == "claim")
                            .map(|(_, value)| value)
                            .expect("claim");
                        let parsed = decode::<Claim>(&encode_state(claim_record))?;
                        let identity = parsed.identity(&authority)?;
                        rewritten.push(set_field(
                            claim,
                            "fields",
                            &set_field(claim_fields_owned(claim), "claim_id", &Json::Str(identity)),
                        ));
                    }
                    record = set_field(
                        &record,
                        "fields",
                        &set_field(fields_owned(&record), "claims", &Json::Array(rewritten)),
                    );
                }
                Ok(record)
            }
            other => Ok(other.clone()),
        }
    }

    fn fields_owned(record: &Json) -> &Json {
        record.get("fields").expect("fixture record fields")
    }

    fn claim_fields_owned(claim: &Json) -> &Json {
        claim.get("fields").expect("claim fields")
    }

    fn digest_of(record: &Json) -> String {
        use sha2::Digest as _;

        let mut hasher = sha2::Sha256::new();
        hasher.update(encode_state(record));
        crate::authority::sha256_hex(&hasher.finalize())
    }

    #[test]
    fn fixture_receipts_decode_and_reencode_byte_identically() {
        let Some(data) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let root = PathBuf::from("/pin");
        let expected = [
            (
                "created-section",
                "1707a7b5e01b8cbdfb38a4c1672d3d194d7d71f6f5f0de177f587c99a24a6173",
            ),
            (
                "taken-over-section",
                "bdb08a71aaafc8659817ee1454b32de3f13646abf06a6e44739097ea7a71ef81",
            ),
            (
                "adopted-section",
                "13a94f0567e83a75ca6ab68f5d536e8d1afc0f1db5f056064829260e5117e722",
            ),
            (
                "taken-over-member",
                "70b2a778afb80feb8d6ef7e83e0a7d5fedc57a91c8a8085d01e121c6e7b23121",
            ),
            (
                "created-document-schema",
                "7132ca049258fa097bb8c0f980500f23c443e262e9631714f94c20c890103b75",
            ),
            (
                "empty-document-schema",
                "428ec3b7a5c5f401c3b5a776fadedeba98bc7134d46b17aa8ac30d67e0de4204",
            ),
        ];
        for (name, digest) in expected {
            let record = data
                .get("receipts")
                .and_then(Json::as_array)
                .expect("receipts")
                .iter()
                .find(|item| item.get("name").and_then(Json::as_str) == Some(name))
                .and_then(|item| item.get("record"))
                .expect("receipt record");
            let moved = relocated(record, &root).expect("relocation");
            let value =
                decode::<ReceiptModel>(&encode_state(&moved)).expect("fixture receipt must decode");
            assert_eq!(
                digest_of(&value.to_state()),
                *digest,
                "receipt {name} re-encode digest"
            );
            assert_eq!(
                encode(&value),
                encode_state(&moved),
                "receipt {name} re-encode must be canonical"
            );
        }
    }

    #[test]
    fn fixture_journals_decode_and_reencode_byte_identically() {
        let Some(data) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let root = PathBuf::from("/pin");
        let expected = [
            (
                "create-section",
                "1047c6c1919a5f496fd893445e98353cde1cae99c24dce84bb5a4f40aa8119b9",
            ),
            (
                "adopt-section",
                "9b7ea27d2cfcbacbb89d2082dd28e72c7b3a8e00e978407abf2e3af9bcc7116b",
            ),
        ];
        for (name, digest) in expected {
            let record = data
                .get("journals")
                .and_then(Json::as_array)
                .expect("journals")
                .iter()
                .find(|item| item.get("name").and_then(Json::as_str) == Some(name))
                .and_then(|item| item.get("record"))
                .expect("journal record");
            let moved = relocated(record, &root).expect("relocation");
            let value =
                decode::<JournalModel>(&encode_state(&moved)).expect("fixture journal must decode");
            assert_eq!(
                digest_of(&value.to_state()),
                digest,
                "journal {name} re-encode digest"
            );
            assert_eq!(
                encode(&value),
                encode_state(&moved),
                "journal {name} re-encode must be canonical"
            );
        }
    }

    #[test]
    fn fixture_preparations_decode_and_reencode_byte_identically() {
        let Some(data) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let root = PathBuf::from("/pin");
        let expected = [(
            "create-section",
            "c392d66b505eafb807a5cb156e64667127aefcc9d5a59d8588740c57133c003b",
        )];
        let cases = data
            .get("preparations")
            .and_then(Json::as_array)
            .expect("preparations");
        assert_eq!(cases.len(), expected.len());
        for (name, digest) in expected {
            let record = cases
                .iter()
                .find(|item| item.get("name").and_then(Json::as_str) == Some(name))
                .and_then(|item| item.get("record"))
                .expect("preparation record");
            let moved = relocated(record, &root).expect("relocation");
            let value =
                decode::<Preparation>(&encode_state(&moved)).expect("fixture preparation decodes");
            assert_eq!(
                digest_of(&value.to_state()),
                digest,
                "preparation {name} re-encode digest"
            );
            assert_eq!(
                encode(&value),
                encode_state(&moved),
                "preparation {name} re-encode must be canonical"
            );
            assert_eq!(
                value.directory(),
                crate::authority::transaction_path(
                    value.resource().destination(),
                    value.transaction_id()
                )
                .expect("transaction path")
            );
        }
    }

    #[test]
    fn decode_accepts_any_object_key_order() {
        let sorted = br#"{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":null}}"#;
        let unsorted = br#"{"type":"Claim","fields":{"selector":null,"kind":{"enum":"Ownership","value":"file"}}}"#;
        let first = decode::<Claim>(sorted).expect("sorted keys must decode");
        let second =
            decode::<Claim>(unsorted).expect("unsorted object keys must decode, matching Python");
        assert_eq!(first, second);
    }

    #[test]
    fn decode_rejects_duplicate_keys() {
        let data = b"{\"type\":\"Claim\",\"type\":\"Claim\"}";
        let error = decode::<Claim>(data).expect_err("duplicate keys must be rejected");
        assert_eq!(error.message(), "duplicate JSON field: type");
    }

    #[test]
    fn codec_round_trips_non_ascii_and_control_escapes() {
        let claim = Claim::new(
            Ownership::Structured,
            ClaimSelector::Structured(
                Selector::new([SelectorPart::Member(
                    Member::new(Scalar::text("café \u{1f} 日本語"), ["café-key"]).expect("member"),
                )])
                .expect("selector"),
            ),
        )
        .expect("claim");
        let data = encode(&claim);
        let expected = concat!(
            "{\"fields\":{\"kind\":{\"enum\":\"Ownership\",\"value\":\"structured\"},",
            "\"selector\":{\"fields\":{\"parts\":[{\"fields\":",
            "{\"identity\":{\"value\":[\"string\",\"café \\u001f 日本語\"]},",
            "\"key\":[\"café-key\"]},\"type\":\"Member\"}]},\"type\":\"Selector\"}},",
            "\"type\":\"Claim\"}\n"
        );
        assert_eq!(data.as_slice(), expected.as_bytes());
        assert_eq!(decode::<Claim>(&data).expect("round trip"), claim);
    }

    #[test]
    fn decode_rejects_unknown_tags_and_fields() {
        let unknown_type = br#"{"type":"NotAThing","fields":{}}"#;
        assert_eq!(
            decode::<Claim>(unknown_type).unwrap_err().message(),
            "unknown state record"
        );
        let bad_fields = br#"{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":null,"extra":1}}"#;
        assert_eq!(
            decode::<Claim>(bad_fields).unwrap_err().message(),
            "state record fields do not match schema"
        );
    }

    #[test]
    fn decode_rejects_floats_and_traversal_paths() {
        let float_claim = br#"{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":1.5}}"#;
        assert_eq!(
            decode::<Claim>(float_claim).unwrap_err().message(),
            "invalid state scalar"
        );
        let ancestor = br#"{"type":"Ancestor","fields":{"path":{"path":"/a/../b"},"device":0,"inode":0,"mode":0,"uid":0,"gid":0,"security":{"bytes":""}}}"#;
        assert_eq!(
            decode::<Ancestor>(ancestor).unwrap_err().message(),
            "state paths must be absolute without traversal"
        );
        let relative = br#"{"type":"Ancestor","fields":{"path":{"path":"relative"},"device":0,"inode":0,"mode":0,"uid":0,"gid":0,"security":{"bytes":""}}}"#;
        assert_eq!(
            decode::<Ancestor>(relative).unwrap_err().message(),
            "state paths must be absolute without traversal"
        );
    }

    #[test]
    fn non_canonical_byte_spelling_is_rejected() {
        let uppercase_hex = br#"{"type":"ArrayPosition","fields":{"previous":null,"following":null,"prefix":{"bytes":"AB"}}}"#;
        assert!(
            decode::<ArrayPosition>(uppercase_hex).is_err(),
            "re-encode with lowercase hex must disagree with raw input"
        );
        let canonical = br#"{"type":"ArrayPosition","fields":{"previous":null,"following":null,"prefix":{"bytes":"ab"}}}"#;
        assert!(decode::<ArrayPosition>(canonical).is_ok());
    }

    fn decode_outcome(kind: &str, data: &[u8]) -> String {
        fn run<T: StateCodec>(data: &[u8]) -> String {
            match decode::<T>(data) {
                Ok(_) => String::new(),
                Err(error) => String::from(error.message()),
            }
        }
        match kind {
            "Key" => run::<Key>(data),
            "Member" => run::<Member>(data),
            "DocumentSchema" => run::<DocumentSchema>(data),
            "ArrayPosition" => run::<ArrayPosition>(data),
            "SelectionSnapshot" => run::<SelectionSnapshot>(data),
            "CreatedContainer" => run::<CreatedContainer>(data),
            "DocumentProvenance" => run::<DocumentProvenance>(data),
            "Selector" => run::<Selector>(data),
            "Ancestor" => run::<Ancestor>(data),
            "Node" => run::<Node>(data),
            "Revision" => run::<Revision>(data),
            "Claim" => run::<Claim>(data),
            "ResourceRef" => run::<ResourceRef>(data),
            "Generation" => run::<Generation>(data),
            "Index" => run::<Index>(data),
            "Receipt" => run::<Receipt>(data),
            "Journal" => run::<Journal>(data),
            "Preparation" => run::<Preparation>(data),
            "OwnedClaim" => run::<OwnedClaim>(data),
            other => panic!("unhandled state type {other}"),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn decode_outcomes_match_the_python_reference() {
        let cases: [(&str, &str, &str); 64] = [
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":4095,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":1000000000000000000000000000000,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":511,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":1000000000000000000000000000000,"mode":0,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "",
            ),
            ("Key", r#"﻿{"fields":{"name":"a"},"type":"Key"}"#, ""),
            (
                "Key",
                r#"{"fields":{"name":5},"type":"Key"}"#,
                "invalid state record",
            ),
            (
                "Key",
                r#"{"fields":{"name":1.5},"type":"Key"}"#,
                "invalid state scalar",
            ),
            (
                "Member",
                r#"{"fields":{"identity":{"value":["string","x"]},"key":"ab"},"type":"Member"}"#,
                "invalid state record",
            ),
            (
                "Member",
                r#"{"fields":{"identity":"x","key":[]},"type":"Member"}"#,
                "invalid state record",
            ),
            (
                "DocumentSchema",
                r#"{"fields":{"format":"json","key":"k","value":{"value":["string","v"]}},"type":"DocumentSchema"}"#,
                "document schema scaffolds require JSON or JSONC",
            ),
            (
                "DocumentSchema",
                r#"{"fields":{"format":{"enum":"DocumentFormat","value":"json"},"key":"k","value":{"bytes":"61"}},"type":"DocumentSchema"}"#,
                "invalid state record",
            ),
            (
                "ArrayPosition",
                r#"{"fields":{"following":null,"prefix":5,"previous":null},"type":"ArrayPosition"}"#,
                "invalid state record",
            ),
            (
                "SelectionSnapshot",
                r#"{"fields":{"layout":{"bytes":""},"owned_syntax":{"bytes":""},"position":5,"syntax":{"bytes":""},"value":{"bytes":"78"}},"type":"SelectionSnapshot"}"#,
                "invalid state record",
            ),
            (
                "SelectionSnapshot",
                r#"{"fields":{"layout":{"bytes":""},"owned_syntax":{"bytes":""},"position":null,"syntax":{"bytes":""},"value":5},"type":"SelectionSnapshot"}"#,
                "invalid state record",
            ),
            (
                "DocumentProvenance",
                r#"{"fields":{"containers":[],"empty_document":5,"schema_empty_document":null,"schema_fields":[],"trailing_separators":[]},"type":"DocumentProvenance"}"#,
                "invalid state record",
            ),
            (
                "DocumentProvenance",
                r#"{"fields":{"containers":[],"empty_document":null,"schema_empty_document":5,"schema_fields":[],"trailing_separators":[]},"type":"DocumentProvenance"}"#,
                "schema empty-document evidence must be whitespace bytes",
            ),
            (
                "DocumentProvenance",
                r#"{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":[5]},"type":"DocumentProvenance"}"#,
                "separator evidence must be SHA-256 digests",
            ),
            (
                "CreatedContainer",
                r#"{"fields":{"empty_syntax":5,"selector":{"fields":{"parts":[{"fields":{"name":"a"},"type":"Key"}]},"type":"Selector"}},"type":"CreatedContainer"}"#,
                "invalid state record",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":"/a","security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "ancestor path must be absolute",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":"0","gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "invalid ancestor metadata",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":true,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "invalid ancestor metadata",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":4096,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "invalid ancestor metadata",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":-1000000000000000000000000000000,"mode":0,"path":{"path":"/a"},"security":{"bytes":""},"uid":0},"type":"Ancestor"}"#,
                "invalid ancestor metadata",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":5,"uid":0},"type":"Ancestor"}"#,
                "invalid state record",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"abc"},"uid":0},"type":"Ancestor"}"#,
                "non-hexadecimal number found in fromhex() arg at position 3",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"zz"},"uid":0},"type":"Ancestor"}"#,
                "non-hexadecimal number found in fromhex() arg at position 0",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"ab0g"},"uid":0},"type":"Ancestor"}"#,
                "non-hexadecimal number found in fromhex() arg at position 3",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"security":{"bytes":"ab cd"},"uid":0},"type":"Ancestor"}"#,
                "state record is not canonical",
            ),
            (
                "Ancestor",
                r#"{"fields":{"device":0,"gid":0,"inode":0,"mode":0,"path":{"path":"/a"},"uid":0},"type":"Ancestor"}"#,
                "state record fields do not match schema",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":0,"path":5,"security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "invalid state record",
            ),
            (
                "Node",
                r#"{"fields":{"data":"x","device":0,"gid":0,"inode":0,"mode":0,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "invalid state record",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":true,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "invalid revision metadata",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":512,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "invalid revision metadata",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":-1000000000000000000000000000000,"mode":0,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "invalid revision metadata",
            ),
            (
                "Revision",
                r#"{"fields":{"nodes":[{"fields":{"name":"a"},"type":"Key"}]},"type":"Revision"}"#,
                "invalid state record",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":5},"selector":null},"type":"Claim"}"#,
                "5 is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":true},"selector":null},"type":"Claim"}"#,
                "True is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":null},"selector":null},"type":"Claim"}"#,
                "None is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":{}},"selector":null},"type":"Claim"}"#,
                "{} is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":[]},"selector":null},"type":"Claim"}"#,
                "[] is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":1.5},"selector":null},"type":"Claim"}"#,
                "1.5 is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":"bogus"},"selector":null},"type":"Claim"}"#,
                "'bogus' is not a valid Ownership",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":"file","selector":null},"type":"Claim"}"#,
                "invalid state record",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Disposition","value":"created"},"selector":null},"type":"Claim"}"#,
                "invalid state record",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":"file"},"selector":5},"type":"Claim"}"#,
                "whole-resource claims cannot have selectors",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":"file"},"selector":{"fields":{"parts":[{"fields":{"name":"a"},"type":"Key"}]},"type":"Selector"}},"type":"Claim"}"#,
                "whole-resource claims cannot have selectors",
            ),
            (
                "Claim",
                r#"{"fields":{"kind":{"enum":"Ownership","value":"structured"},"selector":5},"type":"Claim"}"#,
                "invalid state record",
            ),
            (
                "ResourceRef",
                r#"{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":3},"type":"ResourceRef"}"#,
                "authority schema must be 2",
            ),
            (
                "ResourceRef",
                r#"{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":true},"type":"ResourceRef"}"#,
                "authority schema must be 2",
            ),
            (
                "ResourceRef",
                r#"{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":"document","schema_version":2},"type":"ResourceRef"}"#,
                "invalid state record",
            ),
            (
                "ResourceRef",
                r#"{"fields":{"destination":"/pin/AGENTS.md","kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"}"#,
                "expected an absolute concrete path",
            ),
            (
                "ResourceRef",
                r#"{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2,"state_root":{"path":"/pin/x"}},"type":"ResourceRef"}"#,
                "state record fields do not match schema",
            ),
            (
                "Preparation",
                r#"{"fields":{"directory":{"path":"/pin/x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":2,"transaction_id":"1111"},"type":"Preparation"}"#,
                "invalid hexadecimal identity",
            ),
            (
                "Preparation",
                r#"{"fields":{"directory":{"path":"x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":2,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}"#,
                "state paths must be absolute without traversal",
            ),
            (
                "Preparation",
                r#"{"fields":{"directory":{"path":"/pin/x"},"resource":5,"schema_version":2,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}"#,
                "invalid state record",
            ),
            (
                "Preparation",
                r#"{"fields":{"directory":{"path":"/pin/x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":3,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}"#,
                "preparation schema must be 2",
            ),
            (
                "Preparation",
                r#"{"fields":{"directory":{"path":"/pin/x"},"resource":{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"},"schema_version":2,"transaction_id":"11111111111111111111111111111111"},"type":"Preparation"}"#,
                "",
            ),
            (
                "Index",
                r#"{"fields":{"bundle_id":5,"context_digest":"0000000000000000000000000000000000000000000000000000000000000000","current":null,"pending":null,"previous":null,"residual":[],"schema_version":2},"type":"Index"}"#,
                "invalid state record",
            ),
            (
                "Index",
                r#"{"fields":{"bundle_id":"owner","context_digest":"0000000000000000000000000000000000000000000000000000000000000000","current":5,"pending":null,"previous":null,"residual":[],"schema_version":2},"type":"Index"}"#,
                "invalid state record",
            ),
            (
                "Index",
                r#"{"fields":{"bundle_id":"owner","context_digest":"0000000000000000000000000000000000000000000000000000000000000000","current":null,"pending":null,"previous":null,"residual":[],"schema_version":3},"type":"Index"}"#,
                "index schema must be 2",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":NaN,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "non-JSON numeric constant: NaN",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":Infinity,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "non-JSON numeric constant: Infinity",
            ),
            (
                "Node",
                r#"{"fields":{"data":{"bytes":"78"},"device":0,"gid":0,"inode":0,"mode":-Infinity,"path":"","security":{"bytes":""},"uid":0},"type":"Node"}"#,
                "non-JSON numeric constant: -Infinity",
            ),
            (
                "Key",
                r#"{"fields":{"name":"a"},"type":"Key","type":"Key"}"#,
                "duplicate JSON field: type",
            ),
        ];
        for (kind, record, expected) in cases {
            assert_eq!(
                decode_outcome(kind, record.as_bytes()),
                expected,
                "{kind} record {record}"
            );
        }
    }

    #[test]
    fn decode_pair_outcomes_match_the_python_reference() {
        let reference = r#"{"fields":{"destination":{"path":"/pin/AGENTS.md"},"kind":{"enum":"ResourceKind","value":"document"},"schema_version":2},"type":"ResourceRef"}"#;
        let digest = "0000000000000000000000000000000000000000000000000000000000000000";
        let cases: [(&str, &str); 5] = [
            (
                r#"{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,["<digest>"]]],"render_digest":"<digest>","version":5},"type":"Generation"}"#,
                "invalid state record",
            ),
            (
                r#"{"fields":{"bundle_digest":5,"membership":[[<reference>,["<digest>"]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}"#,
                "invalid hexadecimal identity",
            ),
            (
                r#"{"fields":{"bundle_digest":"<digest>","membership":[[<reference>]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}"#,
                "not enough values to unpack (expected 2, got 1)",
            ),
            (
                r#"{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,["<digest>"],"extra"]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}"#,
                "too many values to unpack (expected 2)",
            ),
            (
                r#"{"fields":{"bundle_digest":"<digest>","membership":[[<reference>,[5]]],"render_digest":"<digest>","version":"1.0"},"type":"Generation"}"#,
                "invalid hexadecimal identity",
            ),
        ];
        for (template, expected) in cases {
            let record = template
                .replace("<reference>", reference)
                .replace("<digest>", digest);
            assert_eq!(
                decode_outcome("Generation", record.as_bytes()),
                expected,
                "generation record {record}"
            );
        }
    }

    #[test]
    fn decode_rejects_unpaired_surrogate_escapes() {
        // Python rejects lone surrogates in model validation; String cannot hold one at all.
        assert_eq!(
            decode_outcome(
                "Key",
                r#"{"fields":{"name":"\ud800"},"type":"Key"}"#.as_bytes()
            ),
            "JSON escape contains an unpaired surrogate"
        );
    }

    #[test]
    fn decode_rejects_wrong_typed_sequence_fields() {
        // Python's sequence() decodes a wrong-typed sequence value by iterating it, so its message may differ; both reject.
        assert_eq!(
            decode_outcome(
                "DocumentProvenance",
                r#"{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":"x"},"type":"DocumentProvenance"}"#.as_bytes()
            ),
            "invalid state record"
        );
        assert_eq!(
            decode_outcome(
                "DocumentProvenance",
                r#"{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":"x","trailing_separators":[]},"type":"DocumentProvenance"}"#.as_bytes()
            ),
            "invalid state record"
        );
        assert_eq!(
            decode_outcome(
                "DocumentProvenance",
                r#"{"fields":{"containers":"x","empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":[]},"type":"DocumentProvenance"}"#.as_bytes()
            ),
            "invalid state record"
        );
        assert_eq!(
            decode_outcome(
                "DocumentProvenance",
                r#"{"fields":{"containers":[],"empty_document":null,"schema_empty_document":null,"schema_fields":[],"trailing_separators":""},"type":"DocumentProvenance"}"#.as_bytes()
            ),
            "invalid state record"
        );
        assert_eq!(
            decode_outcome(
                "Selector",
                r#"{"fields":{"parts":"x"},"type":"Selector"}"#.as_bytes()
            ),
            "invalid state record"
        );
    }
}
