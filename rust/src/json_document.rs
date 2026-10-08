use crate::editors::{DocumentBackend, PathPart, decode_utf8};
use crate::json::{python_float_repr, write_escaped};
use crate::values::{ArrayValue, ObjectValue, Scalar, ScalarKind, Value, ValueError};
use std::fmt::Write as _;

const MAX_DEPTH: usize = 985;
const SNAPSHOT_FRAME_BUDGET: usize = 990;
const IDENTITY_ENTRY_FRAMES: usize = 5;
const OBJECT_LEVEL_FRAMES: usize = 5;
const ARRAY_LEVEL_FRAMES: usize = 4;
const NON_FINITE: &str = "floats must be finite";
const SURROGATES: &str = "scalar must contain Unicode scalar values";
const NESTING: &str = "document nesting is too deep";

pub fn check_snapshot_frames(value: &Value) -> Result<(), ValueError> {
    frames_used(value, 0)
}

pub fn check_identity_frames(value: &Value) -> Result<(), ValueError> {
    frames_used(value, IDENTITY_ENTRY_FRAMES)
}

fn frames_used(value: &Value, used: usize) -> Result<(), ValueError> {
    match value {
        Value::Scalar(_) => Ok(()),
        Value::Object(object) => {
            let used = used + OBJECT_LEVEL_FRAMES;
            if used >= SNAPSHOT_FRAME_BUDGET {
                return Err(ValueError::nesting(NESTING));
            }
            object
                .items()
                .iter()
                .try_for_each(|(_, item)| frames_used(item, used))
        }
        Value::Array(array) => {
            let used = used + ARRAY_LEVEL_FRAMES;
            if used >= SNAPSHOT_FRAME_BUDGET {
                return Err(ValueError::nesting(NESTING));
            }
            array
                .items()
                .iter()
                .try_for_each(|item| frames_used(item, used))
        }
    }
}

#[derive(Clone, Debug)]
pub struct JsonDocument {
    text: String,
    comments: bool,
    root: Node,
}

#[derive(Clone, Debug)]
struct Node {
    start: usize,
    end: usize,
    value: Value,
    children: Vec<(PathPart, Self, usize)>,
    commas: Vec<usize>,
}

impl JsonDocument {
    pub fn parse(text: &str, comments: bool) -> Result<Self, ValueError> {
        let mut scanner = Scanner {
            text,
            pos: 0,
            comments,
        };
        let root = scanner.node(0)?;
        scanner.space()?;
        if scanner.pos != text.len() {
            return Err(ValueError::new("unexpected data after JSON document"));
        }
        Ok(Self {
            text: text.to_owned(),
            comments,
            root,
        })
    }

    fn node(&self, path: &[PathPart]) -> Result<&Node, ValueError> {
        let mut node = &self.root;
        for part in path {
            let Some((_, child, _)) = node
                .children
                .iter()
                .find(|(key, _, _)| parts_match(key, part))
            else {
                return Err(ValueError::new("document selector path is missing"));
            };
            node = child;
        }
        Ok(node)
    }

    fn patch(&mut self, mut patches: Vec<(usize, usize, String)>) -> Result<(), ValueError> {
        patches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        for (start, end, replacement) in &patches {
            self.text.replace_range(*start..*end, replacement.as_str());
        }
        let comments = self.comments;
        let parsed = Self::parse(&self.text, comments)?;
        self.root = parsed.root;
        Ok(())
    }

    fn insert_at(
        &mut self,
        parent_path: &[PathPart],
        index: usize,
        raw: String,
        prefix: Option<&[u8]>,
    ) -> Result<(), ValueError> {
        let parent = self.node(parent_path)?;
        let patches = if index < parent.children.len() {
            let start = if index == 0 {
                parent.start + 1
            } else {
                parent
                    .commas
                    .get(index - 1)
                    .copied()
                    .unwrap_or(parent.start)
                    + 1
            };
            let key_start = parent
                .children
                .get(index)
                .map_or(start, |(_, _, start)| *start);
            let start = self.prefixed(start, key_start, prefix)?;
            vec![(start, start, format!("{raw},"))]
        } else {
            let mut start = parent.end - 1;
            if let Some(prefix) = prefix {
                let boundary =
                    if !parent.commas.is_empty() && parent.commas.len() == parent.children.len() {
                        parent.commas.last().copied().unwrap_or(parent.start) + 1
                    } else if let Some((_, last, _)) = parent.children.last() {
                        last.end
                    } else {
                        parent.start + 1
                    };
                start = self.prefixed(boundary, start, Some(prefix))?;
            }
            let mut patches = Vec::new();
            if !parent.children.is_empty() && parent.commas.len() < parent.children.len() {
                let end = parent
                    .children
                    .last()
                    .map_or(start, |(_, node, _)| node.end);
                patches.push((start, start, raw));
                patches.push((end, end, String::from(",")));
            } else if !parent.commas.is_empty() {
                patches.push((start, start, format!("{raw},")));
            } else {
                patches.push((start, start, raw));
            }
            patches
        };
        self.patch(patches)
    }

    fn prefixed(
        &self,
        start: usize,
        end: usize,
        prefix: Option<&[u8]>,
    ) -> Result<usize, ValueError> {
        let raw = prefix.unwrap_or(b"");
        if !self
            .text
            .as_bytes()
            .get(start..end)
            .is_some_and(|span| span.starts_with(raw))
        {
            return Err(ValueError::new("original array position trivia changed"));
        }
        Ok(start + raw.len())
    }
}

fn parts_match(key: &PathPart, part: &PathPart) -> bool {
    matches!(
        (key, part),
        (PathPart::Key(existing), PathPart::Key(want)) if existing == want
    ) || matches!(
        (key, part),
        (PathPart::Index(existing), PathPart::Index(want)) if existing == want
    )
}

impl DocumentBackend for JsonDocument {
    fn text(&self) -> &str {
        &self.text
    }

    fn native(&self, path: &[PathPart]) -> Result<Value, ValueError> {
        Ok(self.node(path)?.value.clone())
    }

    fn syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        let node = self.node(path)?;
        Ok(self
            .text
            .as_bytes()
            .get(node.start..node.end)
            .map_or_else(Vec::new, <[u8]>::to_vec))
    }

    fn owned_syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        self.syntax(path)
    }

    fn layout(&self, _path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        Ok(Vec::new())
    }

    fn position_prefix(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        let Some((last, parent_path)) = path.split_last() else {
            return Err(ValueError::new("array position requires a member"));
        };
        let PathPart::Index(index) = last else {
            return Err(ValueError::new("array position requires a member"));
        };
        let parent = self.node(parent_path)?;
        let start = if *index == 0 {
            parent.start + 1
        } else {
            parent
                .commas
                .get(index - 1)
                .copied()
                .unwrap_or(parent.start)
                + 1
        };
        let node = self.node(path)?;
        Ok(self
            .text
            .as_bytes()
            .get(start..node.start)
            .map_or_else(Vec::new, <[u8]>::to_vec))
    }

    fn empty_syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        self.syntax(path)
    }

    fn put(
        &mut self,
        path: &[PathPart],
        value: &Value,
        syntax: Option<&[u8]>,
        _layout: &[u8],
    ) -> Result<(), ValueError> {
        let raw = if let Some(bytes) = syntax {
            decode_utf8(bytes)?
        } else {
            let mut out = String::new();
            encode_value(value, &mut out, 0)?;
            out
        };
        let Some((last, parent_path)) = path.split_last() else {
            let (start, end) = (self.root.start, self.root.end);
            return self.patch(vec![(start, end, raw)]);
        };
        let parent = self.node(parent_path)?;
        if let (PathPart::Key(name), Value::Object(object)) = (last, &parent.value)
            && object.get(name).is_none()
        {
            let index = parent.children.len();
            let mut out = String::new();
            write_escaped(name, &mut out);
            out.push(':');
            out.push_str(&raw);
            return self.insert_at(parent_path, index, out, None);
        }
        let node = self.node(path)?;
        self.patch(vec![(node.start, node.end, raw)])
    }

    fn insert(
        &mut self,
        path: &[PathPart],
        index: usize,
        value: &Value,
        syntax: Option<&[u8]>,
        prefix: Option<&[u8]>,
        _layout: &[u8],
    ) -> Result<(), ValueError> {
        let raw = if let Some(bytes) = syntax {
            decode_utf8(bytes)?
        } else {
            let mut out = String::new();
            encode_value(value, &mut out, 0)?;
            out
        };
        self.insert_at(path, index, raw, prefix)
    }

    fn delete(&mut self, path: &[PathPart]) -> Result<(), ValueError> {
        let Some((last, parent_path)) = path.split_last() else {
            return Err(ValueError::new("document selector path is missing"));
        };
        let parent = self.node(parent_path)?;
        let Some(index) = parent
            .children
            .iter()
            .position(|(key, _, _)| parts_match(key, last))
        else {
            return Err(ValueError::new("document selector path is missing"));
        };
        let Some((_, child, key_start)) = parent.children.get(index) else {
            return Err(ValueError::new("document selector path is missing"));
        };
        let mut patches = vec![(child.start, child.end, String::new())];
        if matches!(last, PathPart::Key(_)) {
            let mut scanner = Scanner {
                text: &self.text,
                pos: *key_start,
                comments: self.comments,
            };
            scanner.scan_string()?;
            let key_end = scanner.pos;
            scanner.space()?;
            patches.push((*key_start, key_end, String::new()));
            patches.push((scanner.pos, scanner.pos + 1, String::new()));
        }
        if let Some(&comma) = parent.commas.get(index) {
            patches.push((comma, comma + 1, String::new()));
        } else if let Some(&comma) = parent.commas.last() {
            patches.push((comma, comma + 1, String::new()));
        }
        self.patch(patches)
    }

    fn relocate(
        &mut self,
        path: &[PathPart],
        index: usize,
        value: &Value,
        syntax: Option<&[u8]>,
        prefix: Option<&[u8]>,
        layout: &[u8],
    ) -> Result<(), ValueError> {
        self.delete(path)?;
        let Some((_, parent_path)) = path.split_last() else {
            return Err(ValueError::new("document selector path is missing"));
        };
        self.insert(parent_path, index, value, syntax, prefix, layout)
    }
}

fn encode_value(value: &Value, out: &mut String, depth: usize) -> Result<(), ValueError> {
    if depth > MAX_DEPTH {
        return Err(ValueError::nesting(NESTING));
    }
    match value {
        Value::Scalar(scalar) => match scalar.kind() {
            ScalarKind::Null => out.push_str("null"),
            ScalarKind::Boolean(item) => out.push_str(if *item { "true" } else { "false" }),
            ScalarKind::Integer(item) => {
                let _ = write!(out, "{item}");
            }
            ScalarKind::Float(item) => out.push_str(&python_float_repr(*item)),
            ScalarKind::Text(item) => write_escaped(item, out),
            ScalarKind::Date(_) => {
                return Err(ValueError::type_error(
                    "Object of type date is not JSON serializable",
                ));
            }
            ScalarKind::Time(_) => {
                return Err(ValueError::type_error(
                    "Object of type time is not JSON serializable",
                ));
            }
            ScalarKind::DateTime(_) => {
                return Err(ValueError::type_error(
                    "Object of type datetime is not JSON serializable",
                ));
            }
        },
        Value::Array(array) => {
            out.push('[');
            for (index, item) in array.items().iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_value(item, out, depth + 1)?;
            }
            out.push(']');
        }
        Value::Object(object) => {
            out.push('{');
            for (index, (key, item)) in object.items().iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_escaped(key, out);
                out.push(':');
                encode_value(item, out, depth + 1)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

struct Scanner<'a> {
    text: &'a str,
    pos: usize,
    comments: bool,
}

struct Frame {
    opening: u8,
    start: usize,
    depth: usize,
    entry_start: usize,
    next_key: Option<String>,
    children: Vec<(PathPart, Node, usize)>,
    commas: Vec<usize>,
    members: Vec<(String, Value)>,
    items: Vec<Value>,
}

impl Frame {
    const fn new(opening: u8, start: usize, depth: usize) -> Self {
        Self {
            opening,
            start,
            depth,
            entry_start: 0,
            next_key: None,
            children: Vec::new(),
            commas: Vec::new(),
            members: Vec::new(),
            items: Vec::new(),
        }
    }

    const fn closing(&self) -> u8 {
        if self.opening == b'{' { b'}' } else { b']' }
    }

    fn attach(&mut self, node: Node) -> Result<(), ValueError> {
        let key_start = self.entry_start;
        if self.opening == b'{' {
            let Some(name) = self.next_key.take() else {
                return Err(ValueError::new("missing JSON object colon"));
            };
            self.members.push((name.clone(), node.value.clone()));
            self.children.push((PathPart::Key(name), node, key_start));
        } else {
            let index = self.items.len();
            self.items.push(node.value.clone());
            self.children
                .push((PathPart::Index(index), node, key_start));
        }
        Ok(())
    }

    fn finish(self, end: usize) -> Node {
        let value = if self.opening == b'{' {
            Value::Object(ObjectValue::from_pairs(self.members))
        } else {
            Value::Array(ArrayValue::new(self.items))
        };
        Node {
            start: self.start,
            end,
            value,
            children: self.children,
            commas: self.commas,
        }
    }
}

enum Phase {
    Entry,
    Separator,
}

impl Scanner<'_> {
    fn space(&mut self) -> Result<(), ValueError> {
        self.pos = self.skip_space(self.pos)?;
        Ok(())
    }

    fn skip_space(&self, from: usize) -> Result<usize, ValueError> {
        let bytes = self.text.as_bytes();
        let mut pos = from;
        loop {
            let Some(&byte) = bytes.get(pos) else {
                return Ok(pos);
            };
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' => pos += 1,
                b'/' if self.comments && bytes.get(pos + 1) == Some(&b'/') => {
                    pos = bytes
                        .get(pos + 2..)
                        .and_then(|rest| rest.iter().position(|byte| matches!(byte, b'\r' | b'\n')))
                        .map_or(bytes.len(), |offset| pos + 2 + offset);
                }
                b'/' if self.comments && bytes.get(pos + 1) == Some(&b'*') => {
                    match bytes
                        .get(pos + 2..)
                        .and_then(|rest| rest.windows(2).position(|window| window == b"*/"))
                    {
                        Some(offset) => pos += 2 + offset + 2,
                        None => return Err(ValueError::new("unterminated JSONC comment")),
                    }
                }
                _ => return Ok(pos),
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn node(&mut self, depth: usize) -> Result<Node, ValueError> {
        self.space()?;
        let start = self.pos;
        let Some(&opening) = self.text.as_bytes().get(start) else {
            return Err(ValueError::new("missing JSON value"));
        };
        if !matches!(opening, b'[' | b'{') {
            let (value, end) = self.scalar()?;
            return Ok(Node {
                start,
                end,
                value,
                children: Vec::new(),
                commas: Vec::new(),
            });
        }
        self.pos += 1;
        let mut stack: Vec<Frame> = Vec::new();
        let mut current = Frame::new(opening, start, depth);
        let mut phase = Phase::Entry;
        loop {
            match phase {
                Phase::Entry => {
                    self.space()?;
                    let Some(&byte) = self.text.as_bytes().get(self.pos) else {
                        return Err(ValueError::new("unterminated or malformed JSON container"));
                    };
                    if byte == current.closing() {
                        self.pos += 1;
                        let node = current.finish(self.pos);
                        let Some(parent) = stack.pop() else {
                            return Ok(node);
                        };
                        current = parent;
                        current.attach(node)?;
                        phase = Phase::Separator;
                        continue;
                    }
                    current.entry_start = self.pos;
                    if current.opening == b'{' {
                        let name = self.scan_key(current.depth + 1)?;
                        if current
                            .members
                            .iter()
                            .any(|(existing, _)| existing == &name)
                        {
                            return Err(ValueError::new("duplicate JSON object key"));
                        }
                        self.space()?;
                        if self.text.as_bytes().get(self.pos) != Some(&b':') {
                            return Err(ValueError::new("missing JSON object colon"));
                        }
                        self.pos += 1;
                        current.next_key = Some(name);
                    }
                    self.space()?;
                    let value_start = self.pos;
                    let Some(&value_byte) = self.text.as_bytes().get(value_start) else {
                        return Err(ValueError::new("missing JSON value"));
                    };
                    if matches!(value_byte, b'[' | b'{') {
                        if current.depth + 1 >= MAX_DEPTH {
                            return Err(ValueError::nesting(NESTING));
                        }
                        self.pos += 1;
                        let child = Frame::new(value_byte, value_start, current.depth + 1);
                        stack.push(current);
                        current = child;
                        continue;
                    }
                    let (value, end) = self.scalar()?;
                    let node = Node {
                        start: value_start,
                        end,
                        value,
                        children: Vec::new(),
                        commas: Vec::new(),
                    };
                    current.attach(node)?;
                    phase = Phase::Separator;
                }
                Phase::Separator => {
                    self.space()?;
                    if self.text.as_bytes().get(self.pos) != Some(&b',') {
                        if self.text.as_bytes().get(self.pos) != Some(&current.closing()) {
                            return Err(ValueError::new(
                                "unterminated or malformed JSON container",
                            ));
                        }
                        self.pos += 1;
                        let node = current.finish(self.pos);
                        let Some(parent) = stack.pop() else {
                            return Ok(node);
                        };
                        current = parent;
                        current.attach(node)?;
                        continue;
                    }
                    current.commas.push(self.pos);
                    self.pos += 1;
                    self.space()?;
                    if !self.comments
                        && self.text.as_bytes().get(self.pos) == Some(&current.closing())
                    {
                        return Err(ValueError::new("JSON trailing commas are unsupported"));
                    }
                    phase = Phase::Entry;
                }
            }
        }
    }

    fn scan_key(&mut self, depth: usize) -> Result<String, ValueError> {
        if self.pos == self.text.len() {
            return Err(ValueError::new("missing JSON value"));
        }
        if self.text.as_bytes().get(self.pos) == Some(&b'"') {
            return self.scan_string();
        }
        if matches!(self.text.as_bytes().get(self.pos), Some(b'[' | b'{')) {
            self.node(depth)?;
            return Err(ValueError::new("JSON object keys must be strings"));
        }
        let (value, _) = self.scalar()?;
        let Value::Scalar(scalar) = &value else {
            return Err(ValueError::new("JSON object keys must be strings"));
        };
        scalar.as_text().map_or_else(
            || Err(ValueError::new("JSON object keys must be strings")),
            |text| Ok(text.to_owned()),
        )
    }

    fn scalar(&mut self) -> Result<(Value, usize), ValueError> {
        let start = self.pos;
        match self.text.as_bytes().get(start) {
            Some(b'"') => {
                let text = self.scan_string()?;
                Ok((Value::Scalar(Scalar::text(text)), self.pos))
            }
            Some(b't') => self.literal("true", Ok(Value::Scalar(Scalar::boolean(true)))),
            Some(b'f') => self.literal("false", Ok(Value::Scalar(Scalar::boolean(false)))),
            Some(b'n') => self.literal("null", Ok(Value::Scalar(Scalar::null()))),
            Some(b'N') => self.literal("NaN", Err(ValueError::new(NON_FINITE))),
            Some(b'I') => self.literal("Infinity", Err(ValueError::new(NON_FINITE))),
            Some(b'-') => {
                if self.text.as_bytes().get(start + 1..start + 9) == Some(b"Infinity") {
                    Err(ValueError::new(NON_FINITE))
                } else {
                    self.number()
                }
            }
            Some(b'0'..=b'9') => self.number(),
            _ => Err(decode_error(self.text, "Expecting value", start)),
        }
    }

    fn literal(
        &mut self,
        word: &str,
        value: Result<Value, ValueError>,
    ) -> Result<(Value, usize), ValueError> {
        let start = self.pos;
        if self.text.as_bytes().get(start..start + word.len()) == Some(word.as_bytes()) {
            self.pos += word.len();
            value.map(|value| (value, self.pos))
        } else {
            Err(decode_error(self.text, "Expecting value", start))
        }
    }

    fn number(&mut self) -> Result<(Value, usize), ValueError> {
        let start = self.pos;
        let bytes = self.text.as_bytes();
        let mut pos = start;
        if bytes.get(pos) == Some(&b'-') {
            pos += 1;
        }
        match bytes.get(pos) {
            Some(b'0') => pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(bytes.get(pos), Some(b'0'..=b'9')) {
                    pos += 1;
                }
            }
            _ => return Err(decode_error(self.text, "Expecting value", start)),
        }
        let mut floating = false;
        if bytes.get(pos) == Some(&b'.') && matches!(bytes.get(pos + 1), Some(b'0'..=b'9')) {
            floating = true;
            pos += 1;
            while matches!(bytes.get(pos), Some(b'0'..=b'9')) {
                pos += 1;
            }
        }
        if matches!(bytes.get(pos), Some(b'e' | b'E')) {
            let mut end = pos + 1;
            if matches!(bytes.get(end), Some(b'+' | b'-')) {
                end += 1;
            }
            if matches!(bytes.get(end), Some(b'0'..=b'9')) {
                pos = end;
                while matches!(bytes.get(pos), Some(b'0'..=b'9')) {
                    pos += 1;
                }
                floating = true;
            }
        }
        let Some(text) = self.text.get(start..pos) else {
            return Err(decode_error(self.text, "Expecting value", start));
        };
        let value = if floating {
            let parsed: f64 = text.parse().map_err(|_| ValueError::new(NON_FINITE))?;
            Value::Scalar(Scalar::float(parsed)?)
        } else {
            Value::Scalar(Scalar::integer(
                text.parse::<i64>()
                    .map_err(|_| ValueError::new("integers must fit signed 64 bits"))?,
            ))
        };
        self.pos = pos;
        Ok((value, self.pos))
    }

    #[allow(clippy::too_many_lines)]
    fn scan_string(&mut self) -> Result<String, ValueError> {
        let opening = self.pos;
        let bytes = self.text.as_bytes();
        let mut pos = opening + 1;
        let mut out = String::new();
        let mut surrogate = false;
        loop {
            let Some(&byte) = bytes.get(pos) else {
                return Err(decode_error(
                    self.text,
                    "Unterminated string starting at",
                    opening,
                ));
            };
            match byte {
                b'"' => {
                    pos += 1;
                    self.pos = pos;
                    if surrogate {
                        return Err(ValueError::new(SURROGATES));
                    }
                    return Ok(out);
                }
                b'\\' => {
                    pos += 1;
                    let Some(&escaped) = bytes.get(pos) else {
                        return Err(decode_error(
                            self.text,
                            "Unterminated string starting at",
                            opening,
                        ));
                    };
                    match escaped {
                        b'"' => {
                            out.push('"');
                            pos += 1;
                        }
                        b'\\' => {
                            out.push('\\');
                            pos += 1;
                        }
                        b'/' => {
                            out.push('/');
                            pos += 1;
                        }
                        b'b' => {
                            out.push('\u{0008}');
                            pos += 1;
                        }
                        b'f' => {
                            out.push('\u{000C}');
                            pos += 1;
                        }
                        b'n' => {
                            out.push('\n');
                            pos += 1;
                        }
                        b'r' => {
                            out.push('\r');
                            pos += 1;
                        }
                        b't' => {
                            out.push('\t');
                            pos += 1;
                        }
                        b'u' => {
                            let Some(code) = self.hex4(pos + 1) else {
                                return Err(decode_error(self.text, "Invalid \\uXXXX escape", pos));
                            };
                            pos += 5;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if bytes.get(pos..pos + 2) == Some(b"\\u")
                                    && let Some(second) = self.hex4(pos + 2)
                                    && (0xDC00..=0xDFFF).contains(&second)
                                {
                                    let combined = 0x10000
                                        + (u32::from(code - 0xD800) << 10)
                                        + u32::from(second - 0xDC00);
                                    out.push(
                                        char::from_u32(combined)
                                            .ok_or_else(|| ValueError::new(SURROGATES))?,
                                    );
                                    pos += 6;
                                } else {
                                    surrogate = true;
                                }
                            } else if (0xDC00..=0xDFFF).contains(&code) {
                                surrogate = true;
                            } else {
                                out.push(
                                    char::from_u32(u32::from(code))
                                        .ok_or_else(|| ValueError::new(SURROGATES))?,
                                );
                            }
                        }
                        _ => return Err(decode_error(self.text, "Invalid \\escape", pos - 1)),
                    }
                }
                0x00..=0x1F => {
                    return Err(decode_error(self.text, "Invalid control character at", pos));
                }
                _ => {
                    let Some(character) = self.text[pos..].chars().next() else {
                        return Err(decode_error(
                            self.text,
                            "Unterminated string starting at",
                            opening,
                        ));
                    };
                    out.push(character);
                    pos += character.len_utf8();
                }
            }
        }
    }

    fn hex4(&self, pos: usize) -> Option<u16> {
        let digits = self.text.as_bytes().get(pos..pos + 4)?;
        let mut value: u16 = 0;
        for &digit in digits {
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return None,
            };
            value = value * 16 + u16::from(nibble);
        }
        Some(value)
    }
}

fn decode_error(text: &str, message: &str, byte_pos: usize) -> ValueError {
    let Some(prefix) = text.get(..byte_pos) else {
        return ValueError::new(message);
    };
    let char_pos = prefix.chars().count();
    let line = prefix.matches('\n').count() + 1;
    let column = prefix.rfind('\n').map_or_else(
        || char_pos + 1,
        |index| text[index..byte_pos].chars().count(),
    );
    ValueError::new(format!(
        "{message}: line {line} column {column} (char {char_pos})"
    ))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::literal_string_with_formatting_args
)]
mod tests {
    use super::JsonDocument;
    use crate::editors::{DocumentBackend, PathPart};
    use crate::values::{Scalar, Value, ValueError};

    fn parse(text: &str, comments: bool) -> Result<Value, ValueError> {
        JsonDocument::parse(text, comments).and_then(|document| document.native(&[]))
    }

    fn rejects(text: &str, comments: bool) -> String {
        JsonDocument::parse(text, comments)
            .expect_err("must be rejected")
            .message()
            .to_owned()
    }

    #[test]
    fn parses_scalars_and_containers() {
        assert_eq!(
            parse(r#"{"a":[1,2.5,"x",null,true,{"b":"é"}],"c":{}}"#, false).unwrap(),
            Value::Object(
                crate::values::ObjectValue::new([
                    (
                        "a",
                        Value::Array(crate::values::ArrayValue::new([
                            Value::from(1),
                            Value::Scalar(Scalar::float(2.5).unwrap()),
                            Value::from("x"),
                            Value::Scalar(Scalar::null()),
                            Value::from(true),
                            Value::Object(
                                crate::values::ObjectValue::new([("b", Value::from("é"))]).unwrap(),
                            ),
                        ])),
                    ),
                    ("c", Value::Object(crate::values::ObjectValue::default())),
                ])
                .unwrap()
            )
        );
        assert_eq!(
            parse("[-0,0e0,1E+5,-0.0,1e-05]", false).unwrap(),
            Value::Array(crate::values::ArrayValue::new([
                Value::from(0),
                Value::Scalar(Scalar::float(0.0).unwrap()),
                Value::Scalar(Scalar::float(100_000.0).unwrap()),
                Value::Scalar(Scalar::float(-0.0).unwrap()),
                Value::Scalar(Scalar::float(1e-5).unwrap()),
            ]))
        );
        assert_eq!(
            parse(r#"{"b":2,"a":1}"#, false).unwrap(),
            Value::Object(
                crate::values::ObjectValue::new([("a", Value::from(1)), ("b", Value::from(2))])
                    .unwrap()
            )
        );
        assert_eq!(parse(r#""😀""#, false).unwrap(), Value::from("😀"));
        assert_eq!(
            parse(r#""\ud83d\ude00""#, false).unwrap(),
            Value::from("😀")
        );
    }

    #[test]
    fn rejects_malformed_documents_with_reference_messages() {
        let cases = [
            ("", "missing JSON value"),
            ("// no root", "Expecting value: line 1 column 1 (char 0)"),
            ("1 2", "unexpected data after JSON document"),
            ("[]]", "unexpected data after JSON document"),
            ("{\"a\":1} trailing", "unexpected data after JSON document"),
            ("[", "unterminated or malformed JSON container"),
            ("[01]", "unterminated or malformed JSON container"),
            ("[00]", "unterminated or malformed JSON container"),
            ("[1.]", "unterminated or malformed JSON container"),
            ("[1e]", "unterminated or malformed JSON container"),
            ("[0x10]", "unterminated or malformed JSON container"),
            ("[1 2]", "unterminated or malformed JSON container"),
            ("[truex]", "unterminated or malformed JSON container"),
            ("[Infinityx]", "floats must be finite"),
            ("[NaNx]", "floats must be finite"),
            ("[-NaN]", "Expecting value: line 1 column 2 (char 1)"),
            ("[-Infinit]", "Expecting value: line 1 column 2 (char 1)"),
            ("[-.5]", "Expecting value: line 1 column 2 (char 1)"),
            ("[,]", "Expecting value: line 1 column 2 (char 1)"),
            ("[tru]", "Expecting value: line 1 column 2 (char 1)"),
            ("[+1]", "Expecting value: line 1 column 2 (char 1)"),
            ("[.5]", "Expecting value: line 1 column 2 (char 1)"),
            ("{\"a\":}", "Expecting value: line 1 column 6 (char 5)"),
            ("{\"a\":1,,}", "Expecting value: line 1 column 8 (char 7)"),
            ("{\"a\"}", "missing JSON object colon"),
            ("{\"a\" 1}", "missing JSON object colon"),
            ("{1:2}", "JSON object keys must be strings"),
            ("{01:2}", "JSON object keys must be strings"),
            ("{[1]:2}", "JSON object keys must be strings"),
            ("{[]:2}", "JSON object keys must be strings"),
            ("{[1,2]:2}", "JSON object keys must be strings"),
            ("{[[1]]:2}", "JSON object keys must be strings"),
            ("{{}:1}", "JSON object keys must be strings"),
            ("{[1]:}", "JSON object keys must be strings"),
            ("{\"a\":1, [2]:3}", "JSON object keys must be strings"),
            ("{[1]\"a\":2}", "JSON object keys must be strings"),
            ("{[1],:2}", "JSON object keys must be strings"),
            ("{[1:2}", "unterminated or malformed JSON container"),
            ("{[}:1}", "Expecting value: line 1 column 3 (char 2)"),
            ("[\"a\" \"b\"]", "unterminated or malformed JSON container"),
            ("[\"a\\qb\"]", "Invalid \\escape: line 1 column 4 (char 3)"),
            (
                "[\"a\\u12\"]",
                "Invalid \\uXXXX escape: line 1 column 5 (char 4)",
            ),
            (
                "[\"a\\u00ZZ\"]",
                "Invalid \\uXXXX escape: line 1 column 5 (char 4)",
            ),
            (
                "[\"abc",
                "Unterminated string starting at: line 1 column 2 (char 1)",
            ),
            (
                "[\"a\\",
                "Unterminated string starting at: line 1 column 2 (char 1)",
            ),
            (
                "[\"a\nb\"]",
                "Invalid control character at: line 1 column 4 (char 3)",
            ),
            ("{\"a\":NaN}", "floats must be finite"),
            ("{\"a\":1e400}", "floats must be finite"),
            ("{\"a\":-Infinity}", "floats must be finite"),
            (
                "{\"a\":9223372036854775808}",
                "integers must fit signed 64 bits",
            ),
            (
                "{\"a\":-9223372036854775809}",
                "integers must fit signed 64 bits",
            ),
            ("[\"\\ud800\"]", "scalar must contain Unicode scalar values"),
            ("[\"\\udc00\"]", "scalar must contain Unicode scalar values"),
            (
                "[\"\\ud800abc\"]",
                "scalar must contain Unicode scalar values",
            ),
            (
                "[\"\\ud800\\ud800\"]",
                "scalar must contain Unicode scalar values",
            ),
            (
                "{\"\\ud800\":1}",
                "scalar must contain Unicode scalar values",
            ),
            ("{/* c */}", "Expecting value: line 1 column 2 (char 1)"),
            ("{\"a\":1,}", "JSON trailing commas are unsupported"),
            ("{\"a\":1,\"a\":2}", "duplicate JSON object key"),
            ("{\"a\":", "missing JSON value"),
            ("{\"a\":1", "unterminated or malformed JSON container"),
            ("{\"a\":1,", "unterminated or malformed JSON container"),
            ("[1,", "unterminated or malformed JSON container"),
            ("[1", "unterminated or malformed JSON container"),
            ("{", "unterminated or malformed JSON container"),
            ("{true:1}", "JSON object keys must be strings"),
            ("[1,]", "JSON trailing commas are unsupported"),
        ];
        for (text, message) in cases {
            assert_eq!(rejects(text, false), message, "text {text:?}");
        }
    }

    #[test]
    fn reports_error_positions_on_later_lines() {
        assert_eq!(
            rejects("{\n  \"a\": tru\n}", false),
            "Expecting value: line 2 column 8 (char 9)"
        );
    }

    #[test]
    fn jsonc_accepts_comments_and_trailing_commas() {
        for (text, comments) in [
            ("{\"a\":1,}", true),
            ("{\"a\":1 /* c */}", true),
            ("[1 /* c */, 2,]", true),
            ("{\"a\" // k\n:1}", true),
            ("/*header*/{}", true),
        ] {
            assert!(JsonDocument::parse(text, comments).is_ok(), "text {text:?}");
        }
        assert_eq!(rejects("{/*oops", true), "unterminated JSONC comment");
        assert_eq!(
            rejects("{/* c */[1]:2}", true),
            "JSON object keys must be strings"
        );
        assert_eq!(
            rejects("{\"a\":1,}", false),
            "JSON trailing commas are unsupported"
        );
    }

    #[test]
    fn rejects_excessive_nesting_like_the_reference() {
        let brackets = "[".repeat(986);
        assert_eq!(rejects(&brackets, false), "document nesting is too deep");
        let shallow = format!("{}{}", "[".repeat(985), "]".repeat(985));
        assert!(JsonDocument::parse(&shallow, false).is_ok());
    }

    #[test]
    fn puts_inserts_deletes_and_relocates_members() {
        let mut document = JsonDocument::parse("{\"x\": 1 /* foreign */}", true).expect("document");
        document
            .put(
                &[PathPart::Key(String::from("x"))],
                &Value::from(2),
                None,
                b"",
            )
            .expect("put");
        assert_eq!(document.text(), "{\"x\": 2 /* foreign */}");
        document
            .put(
                &[PathPart::Key(String::from("y"))],
                &Value::from("new"),
                None,
                b"",
            )
            .expect("put new key");
        assert_eq!(document.text(), "{\"x\": 2, /* foreign */\"y\":\"new\"}");
        document
            .delete(&[PathPart::Key(String::from("x"))])
            .expect("delete");
        assert_eq!(document.text(), "{  /* foreign */\"y\":\"new\"}");
        assert_eq!(
            document.native(&[]).expect("native"),
            Value::Object(crate::values::ObjectValue::new([("y", Value::from("new"))]).unwrap())
        );

        let mut array = JsonDocument::parse("[1, 2, 3]", false).expect("document");
        array
            .insert(&[], 0, &Value::from(0), None, None, b"")
            .expect("insert first");
        assert_eq!(array.text(), "[0,1, 2, 3]");
        array
            .relocate(&[PathPart::Index(3)], 0, &Value::from(3), None, None, b"")
            .expect("relocate");
        assert_eq!(array.text(), "[3,0,1, 2 ]");
        assert_eq!(
            array
                .position_prefix(&[PathPart::Index(2)])
                .expect("prefix"),
            b""
        );
        assert_eq!(array.syntax(&[PathPart::Index(2)]).expect("syntax"), b"1");
        assert_eq!(array.layout(&[]).expect("layout"), b"");
        assert_eq!(array.empty_syntax(&[]).expect("empty"), b"[3,0,1, 2 ]");

        let mut middle = JsonDocument::parse("[1, 2]", false).expect("document");
        middle
            .insert(&[], 1, &Value::from(9), None, None, b"")
            .expect("insert middle");
        assert_eq!(middle.text(), "[1,9, 2]");
        middle
            .insert(&[], 9, &Value::from(7), None, None, b"")
            .expect("append");
        assert_eq!(middle.text(), "[1,9, 2,7]");
    }

    #[test]
    fn relocation_prefix_must_match_original_trivia() {
        let mut document = JsonDocument::parse("[1, /* two */ 2]", true).expect("document");
        let error = document
            .relocate(
                &[PathPart::Index(1)],
                0,
                &Value::from(2),
                None,
                Some(b" /* wrong */ "),
                b"",
            )
            .expect_err("prefix mismatch");
        assert_eq!(error.message(), "original array position trivia changed");
        assert_eq!(document.text(), "[1 /* two */ ]");
        let mut matched = JsonDocument::parse("[1, /* two */ 2]", true).expect("document");
        matched
            .relocate(
                &[PathPart::Index(1)],
                0,
                &Value::from(2),
                None,
                Some(b""),
                b"",
            )
            .expect("empty prefix match");
        assert_eq!(matched.text(), "[2,1 /* two */ ]");
        let mut restored =
            JsonDocument::parse("[ /* zero */ 1, /* two */ 2]", true).expect("document");
        restored
            .relocate(
                &[PathPart::Index(1)],
                1,
                &Value::from(2),
                None,
                Some(b" /* two */ "),
                b"",
            )
            .expect("prefix restore");
        assert_eq!(restored.text(), "[ /* zero */ 1, /* two */ 2]");
    }

    #[test]
    fn encodes_values_with_python_json_dumps_spelling() {
        let mut document = JsonDocument::parse("{}", false).expect("document");
        let object = crate::values::ObjectValue::new([
            ("text", Value::from("line\nbreak")),
            ("float", Value::Scalar(Scalar::float(-0.0).unwrap())),
            ("int", Value::from(-12)),
            ("bool", Value::from(false)),
            ("null", Value::Scalar(Scalar::null())),
        ])
        .unwrap();
        document
            .put(
                &[PathPart::Key(String::from("k"))],
                &object.into(),
                None,
                b"",
            )
            .expect("put");
        assert_eq!(
            document.text(),
            "{\"k\":{\"bool\":false,\"float\":-0.0,\"int\":-12,\"null\":null,\"text\":\"line\\nbreak\"}}"
        );
    }

    #[test]
    fn temporal_scalars_are_not_json_serializable() {
        let mut document = JsonDocument::parse("{\"x\":1}", false).expect("document");
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
            let error = document
                .put(&[PathPart::Key(String::from("x"))], &value, None, b"")
                .expect_err("temporal value");
            assert_eq!(error.message(), message);
        }
    }
}
