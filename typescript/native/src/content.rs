use crate::bundles::{entry_from_parts, entry_view};
use crate::convert::ValueTree;
use crate::errors::{BindingError, BindingResult, status};
use crate::guard::on_large_stack;
use flyrail::{
    Content, DocumentFormat, DocumentSchema, FileContent, FileMode, Key, Member, Placement,
    Position, Scalar, SectionBoundaries, SectionContent, Selector, SelectorPart, StructuredContent,
    TreeContent, Value, ValueError,
};
use napi::bindgen_prelude::{FromNapiValue, JsObjectValue, Object, Uint8Array, Unknown};
use napi::{Env, JsValue, ValueType};
use std::str::FromStr;

pub(crate) fn unknown_kind(kind: &str) -> BindingError {
    BindingError::Argument(format!("unknown model kind '{kind}'"))
}

pub(crate) fn parse<T: FromStr<Err = ValueError>>(text: &str) -> BindingResult<T> {
    Ok(text.parse::<T>()?)
}

pub(crate) enum Raw {
    Absent,
    Bool(bool),
    Number(f64),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<Self>),
    Map(Vec<(String, Self)>),
    Tree(Value),
}

const MAX_WIRE_DEPTH: usize = 64;

fn carries_value(kind: Option<&str>, name: &str) -> bool {
    matches!(
        (kind, name),
        (Some("structured" | "schema" | "typed"), "value") | (Some("member"), "identity")
    )
}

fn capture(env: Env, unknown: Unknown<'_>, depth: usize) -> BindingResult<Raw> {
    if depth > MAX_WIRE_DEPTH {
        return Err(BindingError::Argument(String::from("wire is too deep")));
    }
    let raw = unknown.raw();
    let env_raw = env.raw();
    match status(unknown.get_type())? {
        ValueType::Null | ValueType::Undefined => Ok(Raw::Absent),
        ValueType::Boolean => Ok(Raw::Bool(status(unsafe {
            bool::from_napi_value(env_raw, raw)
        })?)),
        ValueType::Number => Ok(Raw::Number(status(unsafe {
            f64::from_napi_value(env_raw, raw)
        })?)),
        ValueType::String => Ok(Raw::Text(status(unsafe {
            String::from_napi_value(env_raw, raw)
        })?)),
        ValueType::Object => {
            let object = Object::from_raw(env_raw, raw);
            if status(object.is_typedarray())? {
                let bytes = status(unsafe { Uint8Array::from_napi_value(env_raw, raw) })?;
                return Ok(Raw::Bytes(bytes.to_vec()));
            }
            if status(object.is_array())? {
                let length = status(object.get_array_length())?;
                return (0..length)
                    .map(|index| {
                        capture(
                            env,
                            status(object.get_element::<Unknown>(index))?,
                            depth + 1,
                        )
                    })
                    .collect::<BindingResult<Vec<_>>>()
                    .map(Raw::List);
            }
            let kind = status(object.get::<String>("kind"))?;
            let names = status(object.get_property_names())?;
            let length = status(names.get_array_length())?;
            (0..length)
                .map(|index| {
                    let name = status(names.get_element::<String>(index))?;
                    let property = status(object.get_named_property::<Unknown>(&name))?;
                    let captured = if carries_value(kind.as_deref(), &name) {
                        ValueTree::read(env_raw, property.raw()).map(|tree| Raw::Tree(tree.0))?
                    } else {
                        capture(env, property, depth + 1)?
                    };
                    Ok((name, captured))
                })
                .collect::<BindingResult<Vec<_>>>()
                .map(Raw::Map)
        }
        _ => Err(BindingError::Argument(String::from(
            "unsupported wire value",
        ))),
    }
}

pub(crate) struct Captured(Raw);

impl Captured {
    pub(crate) fn read(env: Env, wire: Unknown<'_>) -> BindingResult<Self> {
        capture(env, wire, 0).map(Self)
    }

    pub(crate) const fn wire(&self) -> Wire<'_> {
        Wire { raw: &self.0 }
    }
}

pub(crate) trait FromRaw: Sized {
    fn from_raw(raw: &Raw) -> Option<Self>;
}

impl FromRaw for String {
    fn from_raw(raw: &Raw) -> Option<Self> {
        match raw {
            Raw::Text(text) => Some(text.clone()),
            _ => None,
        }
    }
}

impl FromRaw for f64 {
    fn from_raw(raw: &Raw) -> Option<Self> {
        match raw {
            Raw::Number(number) => Some(*number),
            _ => None,
        }
    }
}

impl FromRaw for bool {
    fn from_raw(raw: &Raw) -> Option<Self> {
        match raw {
            Raw::Bool(flag) => Some(*flag),
            _ => None,
        }
    }
}

impl FromRaw for Vec<u8> {
    fn from_raw(raw: &Raw) -> Option<Self> {
        match raw {
            Raw::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        }
    }
}

impl FromRaw for Vec<String> {
    fn from_raw(raw: &Raw) -> Option<Self> {
        match raw {
            Raw::List(items) => items.iter().map(String::from_raw).collect(),
            _ => None,
        }
    }
}

pub(crate) struct Wire<'a> {
    raw: &'a Raw,
}

impl<'a> Wire<'a> {
    fn property(&self, name: &str) -> Option<&'a Raw> {
        match self.raw {
            Raw::Map(members) => members
                .iter()
                .find(|(member, _)| member == name)
                .map(|(_, raw)| raw)
                .filter(|raw| !matches!(raw, Raw::Absent)),
            _ => None,
        }
    }

    fn missing(name: &str) -> BindingError {
        BindingError::Argument(format!("missing wire field '{name}'"))
    }

    pub(crate) fn field<T: FromRaw>(&self, name: &str) -> BindingResult<T> {
        let raw = self.property(name).ok_or_else(|| Self::missing(name))?;
        T::from_raw(raw).ok_or_else(|| {
            BindingError::Argument(format!("wire field '{name}' has the wrong type"))
        })
    }

    pub(crate) fn optional<T: FromRaw>(&self, name: &str) -> BindingResult<Option<T>> {
        self.property(name)
            .map(|raw| {
                T::from_raw(raw).ok_or_else(|| {
                    BindingError::Argument(format!("wire field '{name}' has the wrong type"))
                })
            })
            .transpose()
    }

    pub(crate) fn kind(&self) -> BindingResult<String> {
        self.field("kind")
    }

    pub(crate) fn child(&self, name: &str) -> BindingResult<Self> {
        self.optional_child(name).ok_or_else(|| Self::missing(name))
    }

    pub(crate) fn optional_child(&self, name: &str) -> Option<Self> {
        self.property(name).map(|raw| Self { raw })
    }

    pub(crate) fn children(&self, name: &str) -> BindingResult<Vec<Self>> {
        match self.property(name).ok_or_else(|| Self::missing(name))? {
            Raw::List(items) => Ok(items.iter().map(|raw| Self { raw }).collect()),
            _ => Err(BindingError::Argument(format!(
                "wire field '{name}' has the wrong type"
            ))),
        }
    }

    pub(crate) fn value(&self, name: &str) -> BindingResult<Value> {
        match self.property(name).ok_or_else(|| Self::missing(name))? {
            Raw::Tree(value) => Ok(value.clone()),
            _ => Err(BindingError::Argument(format!(
                "wire field '{name}' has the wrong type"
            ))),
        }
    }
}

pub(crate) fn cloned_tree(value: &Value) -> napi::Result<ValueTree> {
    on_large_stack(|| Ok(ValueTree(value.clone()))).map_err(|error| match error {
        BindingError::Native(message)
        | BindingError::Panic(message)
        | BindingError::Argument(message) => napi::Error::from_reason(message),
        _ => napi::Error::from_reason("value clone failed"),
    })
}

pub(crate) fn node<'env>(env: &'env Env, kind: &str) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("kind", kind)?;
    Ok(object)
}

fn strings(wire: &Wire<'_>, name: &str) -> BindingResult<Vec<String>> {
    wire.field(name)
}

pub(crate) fn read_boundaries(wire: &Wire<'_>) -> BindingResult<SectionBoundaries> {
    match wire.kind()?.as_str() {
        "boundaries" => Ok(SectionBoundaries::new(
            wire.field::<String>("start")?,
            wire.field::<String>("end")?,
        )?),
        "boundaries-marker" => Ok(SectionBoundaries::from_marker(
            &wire.field::<String>("marker")?,
        )?),
        other => Err(unknown_kind(other)),
    }
}

pub(crate) fn write_boundaries<'env>(
    env: &'env Env,
    boundaries: &SectionBoundaries,
) -> napi::Result<Object<'env>> {
    let mut object = node(env, "boundaries")?;
    object.set("start", boundaries.start())?;
    object.set("end", boundaries.end())?;
    Ok(object)
}

pub(crate) fn read_key(wire: &Wire<'_>) -> BindingResult<Key> {
    Ok(Key::new(wire.field::<String>("name")?)?)
}

pub(crate) fn write_key<'env>(env: &'env Env, key: &Key) -> napi::Result<Object<'env>> {
    let mut object = node(env, "key")?;
    object.set("name", key.name())?;
    Ok(object)
}

pub(crate) fn read_member(wire: &Wire<'_>) -> BindingResult<Member> {
    Ok(Member::new(wire.value("identity")?, strings(wire, "key")?)?)
}

pub(crate) fn write_member<'env>(env: &'env Env, member: &Member) -> napi::Result<Object<'env>> {
    let mut object = node(env, "member")?;
    object.set("identity", cloned_tree(member.identity())?)?;
    object.set("key", member.key().to_vec())?;
    Ok(object)
}

fn read_part(wire: &Wire<'_>) -> BindingResult<SelectorPart> {
    match wire.kind()?.as_str() {
        "key" => Ok(SelectorPart::Key(read_key(wire)?)),
        "member" => Ok(SelectorPart::Member(read_member(wire)?)),
        other => Err(unknown_kind(other)),
    }
}

pub(crate) fn read_selector(wire: &Wire<'_>) -> BindingResult<Selector> {
    let parts = wire
        .children("parts")?
        .iter()
        .map(read_part)
        .collect::<BindingResult<Vec<_>>>()?;
    Ok(Selector::new(parts)?)
}

pub(crate) fn write_selector<'env>(
    env: &'env Env,
    selector: &Selector,
) -> napi::Result<Object<'env>> {
    let parts = selector
        .parts()
        .iter()
        .map(|part| match part {
            SelectorPart::Key(key) => write_key(env, key),
            SelectorPart::Member(member) => write_member(env, member),
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = node(env, "selector")?;
    object.set("parts", parts)?;
    Ok(object)
}

pub(crate) fn read_placement(wire: &Wire<'_>) -> BindingResult<Placement> {
    let anchor = wire
        .optional_child("anchor")
        .map(|anchor| read_member(&anchor))
        .transpose()?;
    Ok(Placement::new(
        parse::<Position>(&wire.field::<String>("position")?)?,
        anchor,
    )?)
}

pub(crate) fn write_placement<'env>(
    env: &'env Env,
    placement: &Placement,
) -> napi::Result<Object<'env>> {
    let anchor = placement
        .anchor()
        .map(|anchor| write_member(env, anchor))
        .transpose()?;
    let mut object = node(env, "placement")?;
    object.set("position", placement.position().as_str())?;
    object.set("anchor", anchor)?;
    Ok(object)
}

pub(crate) fn read_tree(wire: &Wire<'_>) -> BindingResult<TreeContent> {
    let entries = wire
        .children("entries")?
        .iter()
        .map(|entry| {
            entry_from_parts(
                entry.field("path")?,
                &entry.field::<String>("kind")?,
                entry.optional::<Vec<u8>>("data")?,
                entry.field("executable")?,
            )
        })
        .collect::<BindingResult<Vec<_>>>()?;
    Ok(TreeContent::new(entries)?)
}

pub(crate) fn write_tree<'env>(env: &'env Env, tree: &TreeContent) -> napi::Result<Object<'env>> {
    let mut object = node(env, "tree")?;
    object.set(
        "entries",
        tree.entries().iter().map(entry_view).collect::<Vec<_>>(),
    )?;
    Ok(object)
}

fn read_section(wire: &Wire<'_>) -> BindingResult<SectionContent> {
    let boundaries = wire
        .optional_child("boundaries")
        .map(|boundaries| read_boundaries(&boundaries))
        .transpose()?;
    Ok(SectionContent::new(
        wire.field::<String>("marker")?,
        wire.field::<String>("text")?,
        boundaries,
    )?)
}

fn write_section<'env>(env: &'env Env, section: &SectionContent) -> napi::Result<Object<'env>> {
    let boundaries = section
        .boundaries()
        .map(|boundaries| write_boundaries(env, boundaries))
        .transpose()?;
    let selector = section
        .selector()
        .map_err(|error| napi::Error::from_reason(error.message().to_owned()))?;
    let mut object = node(env, "section")?;
    object.set("marker", section.marker())?;
    object.set("text", section.text())?;
    object.set("boundaries", boundaries)?;
    object.set("selector", write_boundaries(env, &selector)?)?;
    Ok(object)
}

fn read_structured(wire: &Wire<'_>) -> BindingResult<StructuredContent> {
    let placement = wire
        .optional_child("placement")
        .map(|placement| read_placement(&placement))
        .transpose()?;
    Ok(StructuredContent::new(
        parse::<DocumentFormat>(&wire.field::<String>("format")?)?,
        read_selector(&wire.child("selector")?)?,
        wire.value("value")?,
        placement,
    )?)
}

fn write_structured<'env>(
    env: &'env Env,
    structured: &StructuredContent,
) -> napi::Result<Object<'env>> {
    let placement = structured
        .placement()
        .map(|placement| write_placement(env, placement))
        .transpose()?;
    let mut object = node(env, "structured")?;
    object.set("format", structured.format().as_str())?;
    object.set("selector", write_selector(env, structured.selector())?)?;
    object.set("value", cloned_tree(structured.value())?)?;
    object.set("placement", placement)?;
    Ok(object)
}

pub(crate) fn read_content(wire: &Wire<'_>) -> BindingResult<Content> {
    match wire.kind()?.as_str() {
        "file" => Ok(Content::File(FileContent::new(
            wire.field::<Vec<u8>>("data")?,
            parse::<FileMode>(&wire.field::<String>("mode")?)?,
        ))),
        "tree" => Ok(Content::Tree(read_tree(wire)?)),
        "section" => Ok(Content::Section(read_section(wire)?)),
        "structured" => Ok(Content::Structured(read_structured(wire)?)),
        other => Err(unknown_kind(other)),
    }
}

pub(crate) fn write_content<'env>(env: &'env Env, content: &Content) -> napi::Result<Object<'env>> {
    match content {
        Content::File(file) => {
            let mut object = node(env, "file")?;
            object.set("data", Uint8Array::from(file.data().to_vec()))?;
            object.set("mode", file.mode().as_str())?;
            Ok(object)
        }
        Content::Tree(tree) => write_tree(env, tree),
        Content::Section(section) => write_section(env, section),
        Content::Structured(structured) => write_structured(env, structured),
    }
}

pub(crate) fn read_schema(wire: &Wire<'_>) -> BindingResult<DocumentSchema> {
    let Value::Scalar(value) = wire.value("value")? else {
        return Err(BindingError::Argument(String::from(
            "document schema values must be scalars",
        )));
    };
    Ok(DocumentSchema::new(
        parse::<DocumentFormat>(&wire.field::<String>("format")?)?,
        wire.field::<String>("key")?,
        value,
    )?)
}

pub(crate) fn write_schema<'env>(
    env: &'env Env,
    schema: &DocumentSchema,
) -> napi::Result<Object<'env>> {
    let scalar: Scalar = schema.value().clone();
    let content = schema
        .content()
        .map_err(|error| napi::Error::from_reason(error.message().to_owned()))?;
    let mut object = node(env, "schema")?;
    object.set("format", schema.format().as_str())?;
    object.set("key", schema.key())?;
    object.set("value", ValueTree(Value::Scalar(scalar)))?;
    object.set("content", write_structured(env, &content)?)?;
    Ok(object)
}
