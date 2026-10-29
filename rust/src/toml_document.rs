use crate::editors::{DocumentBackend, PathPart};
use crate::json::{Json, python_float_repr};
use crate::temporal::{Date, DateTime, Time};
use crate::toml_layout::{LayoutPart, LayoutPath, header_path, layout_headers};
use crate::values::{ArrayValue, ObjectValue, Scalar, ScalarKind, Value, ValueError};

const MAX_KEY_DEPTH: usize = 100;
const MAX_VALUE_DEPTH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPart {
    pub(crate) name: String,
    pub(crate) raw: String,
    pub(crate) quoted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub(crate) parts: Vec<KeyPart>,
    pub(crate) dotted: bool,
    pub(crate) sep: String,
}

impl Key {
    pub(crate) fn name(&self) -> &str {
        self.parts.first().map_or("", |part| part.name.as_str())
    }

    pub(crate) fn as_string(&self) -> String {
        let mut text = String::new();
        for (index, part) in self.parts.iter().enumerate() {
            if index > 0 {
                text.push('.');
            }
            text.push_str(&part.raw);
        }
        text
    }

    pub(crate) fn names(&self) -> Vec<&str> {
        self.parts.iter().map(|part| part.name.as_str()).collect()
    }

    pub(crate) fn dotted_string(&self) -> String {
        self.parts
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn part_key(part: &KeyPart, dotted: bool) -> Key {
    Key {
        parts: vec![part.clone()],
        dotted,
        sep: String::from(" = "),
    }
}

fn single_key(name: &str, dotted: bool) -> Key {
    Key {
        parts: vec![KeyPart {
            name: name.to_owned(),
            raw: render_toml_key(name),
            quoted: render_toml_key(name) != name,
        }],
        dotted,
        sep: String::from(" = "),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trivia {
    pub(crate) indent: String,
    pub(crate) comment_ws: String,
    pub(crate) comment: String,
    pub(crate) trail: String,
}

impl Default for Trivia {
    fn default() -> Self {
        Self {
            indent: String::new(),
            comment_ws: String::new(),
            comment: String::new(),
            trail: String::from("\n"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Container {
    pub(crate) body: Vec<Entry>,
    table_keys: Vec<Key>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub(crate) key: Option<Key>,
    pub(crate) item: Item,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub(crate) body: Container,
    pub(crate) trivia: Trivia,
    pub(crate) aot_element: bool,
    pub(crate) implicit: bool,
    pub(crate) display_name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub enum Item {
    Value {
        raw: String,
        value: Value,
        trivia: Trivia,
    },
    Array {
        raw: String,
        value: Value,
        trivia: Trivia,
        items: Vec<Self>,
        spans: Vec<(usize, usize)>,
        commas: Vec<usize>,
    },
    InlineTable {
        raw: String,
        value: Value,
        trivia: Trivia,
        entries: Vec<(Key, Self)>,
    },
    Table(Table),
    Aot(Vec<Table>),
    Whitespace(String),
    Comment(Trivia),
    #[default]
    Null,
}

impl Item {
    pub(crate) const fn trivia(&self) -> Option<&Trivia> {
        match self {
            Self::Value { trivia, .. }
            | Self::Array { trivia, .. }
            | Self::InlineTable { trivia, .. } => Some(trivia),
            _ => None,
        }
    }

    pub(crate) const fn trivia_mut(&mut self) -> Option<&mut Trivia> {
        match self {
            Self::Value { trivia, .. }
            | Self::Array { trivia, .. }
            | Self::InlineTable { trivia, .. } => Some(trivia),
            _ => None,
        }
    }

    fn unwrap_value(&self) -> Result<Value, ValueError> {
        match self {
            Self::Value { value, .. }
            | Self::Array { value, .. }
            | Self::InlineTable { value, .. } => Ok(value.clone()),
            Self::Table(table) => Ok(unwrap_body(&table.body)?),
            Self::Aot(members) => {
                let items = members
                    .iter()
                    .map(|member| unwrap_body(&member.body))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Array(ArrayValue::new(items)))
            }
            _ => Err(ValueError::new("document selector path is missing")),
        }
    }

    fn as_string(&self) -> String {
        match self {
            Self::Value { raw, .. }
            | Self::Array { raw, .. }
            | Self::InlineTable { raw, .. }
            | Self::Whitespace(raw) => raw.clone(),
            Self::Comment(trivia) => {
                format!(
                    "{}{}{}{}",
                    trivia.indent, trivia.comment_ws, trivia.comment, trivia.trail
                )
            }
            Self::Table(table) => render_table_body(table),
            Self::Aot(members) => members.iter().map(render_table_body).collect::<String>(),
            Self::Null => String::new(),
        }
    }
}

const fn table_of(item: &Item) -> Option<&Table> {
    match item {
        Item::Table(table) => Some(table),
        _ => None,
    }
}

const fn table_of_mut(item: &mut Item) -> Option<&mut Table> {
    match item {
        Item::Table(table) => Some(table),
        _ => None,
    }
}

const fn is_table_item(item: &Item) -> bool {
    matches!(item, Item::Table(_) | Item::Aot(_))
}

fn dup_error(name: &str) -> ValueError {
    ValueError::new(format!("Key \"{name}\" already exists."))
}

fn redefinition_error() -> ValueError {
    ValueError::new("Redefinition of an existing table")
}

fn control_error(code: u8, context: &str) -> ValueError {
    ValueError::new(format!(
        "Control characters (codes less than 0x1f and 0x7f) are not allowed in {context}, use \\u{code:04x} instead"
    ))
}

// ------------------------------------------------------------------ parsing

struct Scanner<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    mark: usize,
    line: usize,
    line_start: usize,
}

impl<'a> Scanner<'a> {
    const fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            mark: 0,
            line: 1,
            line_start: 0,
        }
    }

    const fn end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn cur(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn step(&mut self) {
        if let Some(byte) = self.cur() {
            self.pos += 1;
            if byte == b'\n' {
                self.line += 1;
                self.line_start = self.pos;
            }
        }
    }

    fn slice(&self, start: usize, end: usize) -> String {
        self.src.get(start..end).unwrap_or_default().to_owned()
    }

    fn extract(&self) -> String {
        self.slice(self.mark, self.pos)
    }

    const fn mark(&mut self) {
        self.mark = self.pos;
    }

    fn parse_error(&self, message: &str) -> ValueError {
        ValueError::new(format!(
            "{message} at line {} col {}",
            self.line,
            self.pos - self.line_start
        ))
    }

    fn advance_spaces(&mut self) {
        while matches!(self.cur(), Some(b' ' | b'\t')) {
            self.step();
        }
    }
}

pub fn parse_document(src: &str) -> Result<Container, ValueError> {
    let mut scanner = Scanner::new(src);
    let mut root = Container::default();
    while !scanner.end() {
        if scanner.cur() == Some(b'[') {
            break;
        }
        let (start, line, line_start) = (scanner.pos, scanner.line, scanner.line_start);
        let Some((key, item)) = parse_statement(&mut scanner)? else {
            break;
        };
        let dotted = key.as_ref().is_some_and(|key| key.dotted);
        if dotted || !merge_whitespace(&mut root.body, &item) {
            let key = key.clone();
            let item = item.clone();
            if let Err(error) = root.append(key, item, true) {
                scanner.pos = start;
                scanner.line = line;
                scanner.line_start = line_start;
                return Err(scanner.parse_error(error.message()));
            }
        }
    }
    while !scanner.end() {
        let (start, line, line_start) = (scanner.pos, scanner.line, scanner.line_start);
        let (key, item) = parse_table(&mut scanner, None, &mut Vec::new())?;
        let item = match &item {
            Item::Table(table) if table.aot_element => {
                let members = bundle_aot(&mut scanner, table.clone(), &key)?;
                Item::Aot(members)
            }
            _ => item,
        };
        if let Err(error) = root.append(Some(key), item, true) {
            scanner.pos = start;
            scanner.line = line;
            scanner.line_start = line_start;
            return Err(scanner.parse_error(error.message()));
        }
    }
    Ok(root)
}

fn merge_whitespace(body: &mut [Entry], item: &Item) -> bool {
    let Item::Whitespace(raw) = item else {
        return false;
    };
    let Some(last) = body.last_mut() else {
        return false;
    };
    let Item::Whitespace(existing) = &last.item else {
        return false;
    };
    last.item = Item::Whitespace(format!("{existing}{raw}"));
    true
}

fn parse_statement(scanner: &mut Scanner<'_>) -> Result<Option<(Option<Key>, Item)>, ValueError> {
    scanner.mark();
    loop {
        match scanner.cur() {
            None | Some(b'[') => return Ok(None),
            Some(b'\n') => {
                scanner.step();
                return Ok(Some((
                    None,
                    Item::Whitespace(scanner.slice(scanner.mark, scanner.pos)),
                )));
            }
            Some(b' ' | b'\t') => {
                scanner.step();
                if scanner.end() {
                    return Ok(Some((
                        None,
                        Item::Whitespace(scanner.slice(scanner.mark, scanner.pos)),
                    )));
                }
            }
            Some(b'\r') => {
                if scanner.at(1) != Some(b'\n') {
                    return Err(control_error(0x0d, "documents"));
                }
                scanner.step();
            }
            Some(b'#') => {
                let indent = scanner.extract();
                let mut trivia = scanner.comment_trail(true)?;
                trivia.indent = indent;
                return Ok(Some((None, Item::Comment(trivia))));
            }
            _ => break,
        }
    }
    let indent = scanner.extract();
    let (key, mut item) = parse_key_value(scanner, true, 0)?;
    if let Some(trivia) = item.trivia_mut() {
        trivia.indent = indent;
    }
    Ok(Some((Some(key), item)))
}

impl Scanner<'_> {
    fn comment_trail(&mut self, parse_trail: bool) -> Result<Trivia, ValueError> {
        let mut trivia = Trivia {
            trail: String::new(),
            ..Trivia::default()
        };
        if self.end() {
            return Ok(trivia);
        }
        self.mark();
        loop {
            match self.cur() {
                None | Some(b'\n') => break,
                Some(b'#') => {
                    trivia.comment_ws = self.extract();
                    self.mark();
                    self.step();
                    while !self.end() && !matches!(self.cur(), Some(b'\n' | b'\r')) {
                        match self.cur() {
                            Some(byte) if byte == 0x7f || (byte < 0x1f && byte != b'\t') => {
                                return Err(control_error(byte, "comments"));
                            }
                            _ => self.step(),
                        }
                    }
                    trivia.comment = self.extract();
                    self.mark();
                    break;
                }
                Some(b' ' | b'\t') => self.step(),
                Some(b'\r') => {
                    if self.at(1) != Some(b'\n') {
                        return Err(ValueError::new(format!(
                            "{} at line {} col 0",
                            control_error(0x0d, "comments").message(),
                            self.line
                        )));
                    }
                    self.step();
                }
                Some(byte) => {
                    return Err(
                        self.parse_error(&format!("Unexpected character: {:?}", byte as char))
                    );
                }
            }
            if self.end() {
                break;
            }
        }
        if parse_trail {
            let mark = self.mark;
            self.advance_spaces();
            if self.cur() == Some(b'\r') {
                if self.at(1) != Some(b'\n') {
                    return Err(control_error(0x0d, "documents"));
                }
                self.step();
            }
            if self.cur() == Some(b'\n') {
                self.step();
            }
            if self.pos != mark {
                trivia.trail = self.extract();
            }
        }
        Ok(trivia)
    }
}

fn parse_key_value(
    scanner: &mut Scanner<'_>,
    parse_comment: bool,
    depth: usize,
) -> Result<(Key, Item), ValueError> {
    scanner.mark();
    scanner.advance_spaces();
    let indent = scanner.extract();
    let mut key = parse_key(scanner)?;
    scanner.mark();
    let mut found_equals = scanner.cur() == Some(b'=');
    while scanner
        .cur()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'='))
    {
        scanner.step();
        if scanner.cur() == Some(b'=') {
            if found_equals {
                return Err(scanner.parse_error("Unexpected character: '='"));
            }
            found_equals = true;
        }
    }
    if !found_equals {
        let current = scanner.cur().map_or_else(
            || String::from("'\\x00'"),
            |byte| format!("{:?}", byte as char),
        );
        return Err(scanner.parse_error(&format!("Unexpected character: {current}")));
    }
    key.sep = scanner.extract();
    let combined_depth = depth + key.parts.len().saturating_sub(1);
    if combined_depth > MAX_VALUE_DEPTH {
        return Err(scanner.parse_error("TOML value nested more than 100 levels deep"));
    }
    let mut item = parse_value(scanner, combined_depth)?;
    if let Some(trivia) = item.trivia_mut() {
        if parse_comment {
            let trail = scanner.comment_trail(true)?;
            if trivia.comment_ws.is_empty() {
                trivia.comment_ws = trail.comment_ws;
            }
            trivia.comment = trail.comment;
            trivia.trail = trail.trail;
        } else {
            trivia.trail = String::new();
        }
        trivia.indent = indent;
    }
    Ok((key, item))
}

fn parse_key(scanner: &mut Scanner<'_>) -> Result<Key, ValueError> {
    let first = parse_simple_key(scanner)?;
    let mut parts = vec![first];
    while scanner.cur() == Some(b'.') {
        if parts.len() + 1 > MAX_KEY_DEPTH {
            return Err(scanner.parse_error(&format!(
                "TOML key nested more than {MAX_KEY_DEPTH} levels deep"
            )));
        }
        scanner.step();
        parts.push(parse_simple_key(scanner)?);
    }
    let dotted = parts.len() > 1;
    Ok(Key {
        parts,
        dotted,
        sep: String::new(),
    })
}

fn parse_simple_key(scanner: &mut Scanner<'_>) -> Result<KeyPart, ValueError> {
    scanner.mark();
    scanner.advance_spaces();
    let raw_start = scanner.mark;
    let quoted = matches!(scanner.cur(), Some(b'"' | b'\''));
    if quoted {
        let quote = scanner.cur().unwrap_or(b'"');
        scanner.step();
        let mut name = String::new();
        loop {
            let Some(byte) = scanner.cur() else {
                return Err(scanner.parse_error("Unexpected end of file"));
            };
            match byte {
                b'\n' => return Err(scanner.parse_error("Unexpected character: '\\n'")),
                byte if byte == quote => {
                    scanner.step();
                    scanner.advance_spaces();
                    return Ok(KeyPart {
                        name,
                        raw: scanner.slice(raw_start, scanner.pos),
                        quoted: true,
                    });
                }
                b'\\' if quote == b'"' => {
                    scanner.step();
                    name.push(parse_escape(scanner)?);
                }
                byte => push_byte(&mut name, byte, scanner),
            }
        }
    }
    let start = scanner.pos;
    while let Some(byte) = scanner.cur() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b' ' | b'\t') {
            scanner.step();
        } else {
            break;
        }
    }
    let name = scanner.slice(start, scanner.pos);
    let name = name.trim();
    if name.is_empty() {
        return Err(scanner.parse_error(&format!(
            "Empty key at line {} col {}",
            scanner.line,
            start - scanner.line_start
        )));
    }
    Ok(KeyPart {
        name: name.to_owned(),
        raw: scanner.slice(raw_start, scanner.pos),
        quoted: false,
    })
}

fn push_byte(value: &mut String, byte: u8, scanner: &mut Scanner<'_>) {
    let start = scanner.pos;
    scanner.step();
    if byte.is_ascii() {
        value.push(byte as char);
    } else {
        value.push_str(&scanner.slice(start, scanner.pos));
    }
}

fn parse_escape(scanner: &mut Scanner<'_>) -> Result<char, ValueError> {
    let Some(byte) = scanner.cur() else {
        return Err(scanner.parse_error("Unexpected end of file"));
    };
    let decoded = match byte {
        b'b' => '\u{8}',
        b't' => '\t',
        b'n' => '\n',
        b'f' => '\u{c}',
        b'r' => '\r',
        b'"' => '"',
        b'\\' => '\\',
        b'u' | b'U' => {
            let start = scanner.pos;
            scanner.step();
            return parse_unicode_escape(scanner, if byte == b'u' { 4 } else { 8 }).map_err(|_| {
                scanner.pos = start;
                scanner.parse_error("Invalid unicode value")
            });
        }
        other => {
            return Err(
                scanner.parse_error(&format!("Invalid character '{}' in string", other as char))
            );
        }
    };
    scanner.step();
    Ok(decoded)
}

fn parse_unicode_escape(scanner: &mut Scanner<'_>, digits: usize) -> Result<char, ValueError> {
    let start = scanner.pos;
    for _ in 0..digits {
        match scanner.cur() {
            Some(byte) if byte.is_ascii_hexdigit() => scanner.step(),
            _ => return Err(scanner.parse_error("Invalid unicode escape sequence")),
        }
    }
    let text = scanner.slice(start, scanner.pos);
    let code = u32::from_str_radix(&text, 16)
        .map_err(|_| ValueError::new("Invalid unicode escape sequence"))?;
    char::from_u32(code).ok_or_else(|| ValueError::new("Invalid unicode escape sequence"))
}

fn parse_value(scanner: &mut Scanner<'_>, depth: usize) -> Result<Item, ValueError> {
    let start = scanner.pos;
    match scanner.cur() {
        Some(b'"' | b'\'') => {
            let quote = scanner.cur().unwrap_or(b'"');
            let multiline = scanner.at(1) == Some(quote) && scanner.at(2) == Some(quote);
            let text = if multiline {
                parse_multiline_string(scanner, quote)?
            } else if quote == b'"' {
                parse_basic_string(scanner)?
            } else {
                parse_literal_string(scanner)?
            };
            Ok(Item::Value {
                raw: scanner.slice(start, scanner.pos),
                value: Value::Scalar(Scalar::text(text)),
                trivia: Trivia::default(),
            })
        }
        Some(b'[') => {
            if depth >= MAX_VALUE_DEPTH {
                return Err(scanner.parse_error(&format!(
                    "TOML value nested more than {MAX_VALUE_DEPTH} levels deep"
                )));
            }
            parse_array(scanner, depth + 1)
        }
        Some(b'{') => {
            if depth >= MAX_VALUE_DEPTH {
                return Err(scanner.parse_error(&format!(
                    "TOML value nested more than {MAX_VALUE_DEPTH} levels deep"
                )));
            }
            parse_inline_table(scanner, depth + 1)
        }
        Some(_) => {
            let raw = scan_bare_value(scanner);
            let value = parse_bare_value(&raw, scanner)?;
            Ok(Item::Value {
                raw,
                value,
                trivia: Trivia::default(),
            })
        }
        None => Err(scanner.parse_error("Unexpected character: '\\x00'")),
    }
}

fn parse_basic_string(scanner: &mut Scanner<'_>) -> Result<String, ValueError> {
    scanner.step();
    let mut value = String::new();
    loop {
        let Some(byte) = scanner.cur() else {
            return Err(scanner.parse_error("Unexpected end of file"));
        };
        match byte {
            b'"' => {
                scanner.step();
                return Ok(value);
            }
            b'\n' => return Err(scanner.parse_error("Unexpected character: '\\n'")),
            b'\\' => {
                scanner.step();
                value.push(parse_escape(scanner)?);
            }
            byte => {
                match byte {
                    0x7f => return Err(control_error(byte, "strings")),
                    b if b < 0x1f && b != b'\t' => {
                        return Err(control_error(byte, "strings"));
                    }
                    _ => {}
                }
                push_byte(&mut value, byte, scanner);
            }
        }
    }
}

fn parse_literal_string(scanner: &mut Scanner<'_>) -> Result<String, ValueError> {
    scanner.step();
    let mut value = String::new();
    loop {
        let Some(byte) = scanner.cur() else {
            return Err(scanner.parse_error("Unexpected end of file"));
        };
        match byte {
            b'\'' => {
                scanner.step();
                return Ok(value);
            }
            b'\n' => return Err(scanner.parse_error("Unexpected character: '\\n'")),
            0x7f => return Err(control_error(byte, "strings")),
            b if b < 0x1f && b != b'\t' => return Err(control_error(byte, "strings")),
            byte => push_byte(&mut value, byte, scanner),
        }
    }
}

fn parse_multiline_string(scanner: &mut Scanner<'_>, quote: u8) -> Result<String, ValueError> {
    scanner.step();
    scanner.step();
    scanner.step();
    let basic = quote == b'"';
    if scanner.cur() == Some(b'\n') {
        scanner.step();
    } else if scanner.cur() == Some(b'\r') && scanner.at(1) == Some(b'\n') {
        scanner.step();
        scanner.step();
    }
    let mut value = String::new();
    loop {
        let Some(byte) = scanner.cur() else {
            return Err(scanner.parse_error("Unexpected end of file"));
        };
        if byte == quote {
            let mut quotes = 0;
            while scanner.cur() == Some(quote) {
                scanner.step();
                quotes += 1;
                if quotes >= 3 {
                    break;
                }
            }
            if quotes >= 3 {
                return Ok(value);
            }
            for _ in 0..quotes {
                value.push(quote as char);
            }
            continue;
        }
        if basic && byte == b'\\' {
            if scanner.at(1) == Some(b'\n') {
                scanner.step();
                scanner.step();
                while matches!(scanner.cur(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    scanner.step();
                }
                continue;
            }
            if scanner.at(1) == Some(b'\r') && scanner.at(2) == Some(b'\n') {
                scanner.step();
                scanner.step();
                scanner.step();
                while matches!(scanner.cur(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    scanner.step();
                }
                continue;
            }
            scanner.step();
            value.push(parse_escape(scanner)?);
            continue;
        }
        push_byte(&mut value, byte, scanner);
    }
}

fn scan_bare_value(scanner: &mut Scanner<'_>) -> String {
    let start = scanner.pos;
    for literal in ["true", "false"] {
        if scanner
            .src
            .get(start..)
            .is_some_and(|text| text.starts_with(literal))
        {
            for _ in literal.bytes() {
                scanner.step();
            }
            return literal.to_owned();
        }
    }
    while scanner.cur().is_some_and(|byte| {
        !byte.is_ascii_whitespace() && !matches!(byte, b',' | b']' | b'}' | b'#')
    }) {
        scanner.step();
    }
    if scanner.pos - start == 10
        && scanner.cur() == Some(b' ')
        && scanner.at(1).is_some_and(|byte| byte.is_ascii_digit())
        && scanner.at(3) == Some(b':')
    {
        scanner.step();
        while scanner.cur().is_some_and(|byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b',' | b']' | b'}' | b'#')
        }) {
            scanner.step();
        }
    }
    scanner.slice(start, scanner.pos)
}

fn parse_bare_value(raw: &str, scanner: &Scanner<'_>) -> Result<Value, ValueError> {
    match raw {
        "true" => return Ok(Value::Scalar(Scalar::boolean(true))),
        "false" => return Ok(Value::Scalar(Scalar::boolean(false))),
        "inf" | "+inf" => return Ok(Value::Scalar(Scalar::raw_float(f64::INFINITY))),
        "-inf" => return Ok(Value::Scalar(Scalar::raw_float(f64::NEG_INFINITY))),
        "nan" | "+nan" | "-nan" => return Ok(Value::Scalar(Scalar::raw_float(f64::NAN))),
        _ => {}
    }
    if let Some(scalar) = parse_number(raw) {
        return Ok(Value::Scalar(scalar));
    }
    if let Some(scalar) = parse_temporal(raw) {
        return Ok(Value::Scalar(scalar));
    }
    let diagnostic = if raw.len() >= 10 && raw.get(4..5) == Some("-") {
        if raw.len() == 10 {
            "Invalid date"
        } else if native_datetime_grammar(raw) {
            "Invalid datetime"
        } else {
            "Invalid number"
        }
    } else if raw.contains(':') {
        "Invalid time"
    } else {
        "Invalid number"
    };
    Err(scanner.parse_error(diagnostic))
}

fn strip_underscores(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'_' {
            let digit = |byte: Option<u8>| {
                byte.is_some_and(|byte| {
                    if raw.starts_with("0x") {
                        byte.is_ascii_hexdigit()
                    } else {
                        byte.is_ascii_digit()
                    }
                })
            };
            if !digit(index.checked_sub(1).and_then(|i| bytes.get(i)).copied())
                || !digit(bytes.get(index + 1).copied())
            {
                return None;
            }
            continue;
        }
        out.push(*byte as char);
    }
    Some(out)
}

fn parse_number(raw: &str) -> Option<Scalar> {
    if raw.is_empty() {
        return None;
    }
    let (sign, body) = match raw.as_bytes().first() {
        Some(b'+') => (1_i64, &raw[1..]),
        Some(b'-') => (-1_i64, &raw[1..]),
        _ => (1_i64, raw),
    };
    if body.is_empty() {
        return None;
    }
    let radix = [("0x", 16_u32), ("0o", 8), ("0b", 2)]
        .iter()
        .find_map(|(prefix, radix)| body.starts_with(prefix).then_some(*radix))
        .unwrap_or(10);
    if radix != 10 {
        if sign < 0 {
            return None;
        }
        let clean = strip_underscores(body)?;
        let clean = clean.get(2..)?;
        let value = i64::from_str_radix(clean, radix).ok()?;
        return Some(Scalar::integer(value));
    }
    let clean = strip_underscores(body)?;
    if clean.is_empty() {
        return None;
    }
    let has_dot = clean.contains('.');
    let has_exp = clean.contains(['e', 'E']);
    if !has_dot && !has_exp {
        if clean.len() > 1 && clean.starts_with('0') {
            return None;
        }
        let value: i64 = clean.parse().ok()?;
        return Some(Scalar::integer(value.checked_mul(sign)?));
    }
    if !clean.starts_with(|first: char| first.is_ascii_digit() || first == '.') {
        return None;
    }
    if clean.ends_with('.')
        || clean.split_once(['e', 'E']).is_some_and(|(_, exp)| {
            let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
            exp.is_empty() || !exp.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return None;
    }
    let mantissa = clean
        .split_once(['e', 'E'])
        .map_or(clean.as_str(), |(m, _)| m);
    if mantissa.matches('.').count() > 1
        || !mantissa
            .strip_prefix('.')
            .unwrap_or(mantissa)
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    if mantissa.len() > 1 && mantissa.starts_with('0') && !mantissa.starts_with("0.") {
        return None;
    }
    let value: f64 = clean.parse().ok()?;
    Some(Scalar::raw_float(if sign < 0 { -value } else { value }))
}

fn parse_temporal(raw: &str) -> Option<Scalar> {
    let looks_like_date = raw.len() >= 10 && raw.get(4..5) == Some("-");
    if looks_like_date {
        let date = raw.get(..10)?.parse::<Date>().ok()?;
        let rest = raw.get(10..)?;
        if rest.is_empty() {
            return Some(Scalar::date(date));
        }
        let (separator, time_text) = match rest.as_bytes().first() {
            Some(b'T') => ('T', rest.get(1..)?),
            Some(b't') => ('t', rest.get(1..)?),
            Some(b' ') => (' ', rest.get(1..)?),
            _ => return None,
        };
        let _ = separator;
        let (time_text, offset) = split_time_offset(time_text)?;
        let time = parse_time_text(time_text)?;
        return Some(Scalar::datetime(build_datetime(date, time, offset)?));
    }
    if raw.contains(':') {
        let time = parse_time_text(raw)?;
        return Some(Scalar::time(time));
    }
    None
}

fn parse_time_text(text: &str) -> Option<Time> {
    let bytes = text.as_bytes();
    if bytes.len() < 8 {
        return None;
    }
    if bytes.get(2) != Some(&b':') || bytes.get(5) != Some(&b':') {
        return None;
    }
    let hour = text.get(..2)?.parse::<u8>().ok()?;
    let minute = text.get(3..5)?.parse::<u8>().ok()?;
    let second = text.get(6..8)?.parse::<u8>().ok()?;
    let mut microsecond = 0;
    match text.get(8..)? {
        "" => {}
        fraction => {
            let fraction = fraction.strip_prefix('.')?;
            if fraction.is_empty() {
                return None;
            }
            if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let fraction = fraction.get(..fraction.len().min(6))?;
            let exponent = u32::try_from(fraction.len()).ok()?;
            microsecond = fraction.parse::<u32>().ok()? * 10_u32.pow(6 - exponent);
        }
    }
    Time::new(hour, minute, second, microsecond).ok()
}

fn split_time_offset(text: &str) -> Option<(&str, Option<&str>)> {
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 7 && matches!(byte, b'Z' | b'z' | b'+' | b'-') {
            return Some((text.get(..index)?, Some(text.get(index..)?)));
        }
    }
    Some((text, None))
}

fn native_datetime_grammar(raw: &str) -> bool {
    let Some(time_text) = raw.get(11..) else {
        return false;
    };
    if !matches!(raw.as_bytes().get(10), Some(b'T' | b't' | b' ')) {
        return false;
    }
    let Some((time, offset)) = split_time_offset(time_text) else {
        return false;
    };
    let base = time.get(..8).unwrap_or("");
    if base.len() != 8
        || !base.bytes().enumerate().all(|(index, byte)| {
            if index == 2 || index == 5 {
                byte == b':'
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return false;
    }
    let fraction = time.get(8..).unwrap_or("");
    if !fraction.is_empty()
        && !fraction.strip_prefix('.').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return false;
    }
    offset.is_none_or(|offset| {
        offset.eq_ignore_ascii_case("z")
            || (offset.len() == 6
                && offset.bytes().enumerate().all(|(index, byte)| match index {
                    0 => matches!(byte, b'+' | b'-'),
                    3 => byte == b':',
                    _ => byte.is_ascii_digit(),
                }))
    })
}

fn build_datetime(date: Date, time: Time, offset: Option<&str>) -> Option<DateTime> {
    match offset {
        None => Some(DateTime::local(date, time)),
        Some(text) if text.eq_ignore_ascii_case("z") => Some(DateTime::coordinated(date, time)),
        Some(text) => {
            if text.len() != 6
                || !matches!(text.as_bytes().first(), Some(b'+' | b'-'))
                || text.as_bytes().get(3) != Some(&b':')
                || !text.get(1..3)?.bytes().all(|byte| byte.is_ascii_digit())
                || !text.get(4..)?.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let sign = if text.starts_with('-') { -1 } else { 1 };
            let body = text.get(1..)?;
            let (hours, minutes) = body.split_once(':')?;
            let hours: i32 = hours.parse().ok()?;
            let minutes: i32 = minutes.parse().ok()?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            DateTime::with_offset(date, time, sign * (hours * 3600 + minutes * 60)).ok()
        }
    }
}

fn parse_array(scanner: &mut Scanner<'_>, depth: usize) -> Result<Item, ValueError> {
    let start = scanner.pos;
    scanner.step();
    let mut values = Vec::new();
    let mut items = Vec::new();
    let mut spans = Vec::new();
    let mut commas = Vec::new();
    loop {
        loop {
            scanner.advance_spaces();
            match scanner.cur() {
                Some(b'\n' | b'\r') => scanner.step(),
                Some(b'#') => {
                    scanner.comment_trail(true)?;
                }
                _ => break,
            }
        }
        match scanner.cur() {
            Some(b']') => {
                scanner.step();
                break;
            }
            None => return Err(scanner.parse_error("Unexpected end of file")),
            Some(b',') => return Err(scanner.parse_error("Unexpected character: ','")),
            _ => {}
        }
        let item_start = scanner.pos;
        let item = parse_value(scanner, depth)?;
        values.push(item.unwrap_value()?);
        items.push(item);
        spans.push((item_start - start, scanner.pos - start));
        loop {
            scanner.advance_spaces();
            match scanner.cur() {
                Some(b'\n' | b'\r') => scanner.step(),
                Some(b'#') => {
                    scanner.comment_trail(true)?;
                }
                _ => break,
            }
        }
        match scanner.cur() {
            Some(b',') => {
                commas.push(scanner.pos - start);
                scanner.step();
            }
            Some(b']') => {}
            None => return Err(scanner.parse_error("Unexpected end of file")),
            Some(byte) => {
                return Err(
                    scanner.parse_error(&format!("Unexpected character: {:?}", byte as char))
                );
            }
        }
    }
    let raw = scanner.slice(start, scanner.pos);
    Ok(Item::Array {
        value: Value::Array(ArrayValue::new(values)),
        raw,
        trivia: Trivia::default(),
        items,
        spans,
        commas,
    })
}

fn parse_inline_table(scanner: &mut Scanner<'_>, depth: usize) -> Result<Item, ValueError> {
    let start = scanner.pos;
    scanner.step();
    let mut entries: Vec<(Key, Item)> = Vec::new();
    loop {
        loop {
            match scanner.cur() {
                Some(b' ' | b'\t' | b'\n') => scanner.step(),
                Some(b'#') => {
                    scanner.comment_trail(true)?;
                }
                _ => break,
            }
        }
        if scanner.cur() == Some(b'}') {
            scanner.step();
            break;
        }
        if entries.is_empty() && scanner.cur() == Some(b',') {
            return Err(scanner.parse_error("Unexpected character: ','"));
        }
        match scanner.cur() {
            None => return Err(scanner.parse_error("Unexpected end of file")),
            Some(b',') => {
                scanner.step();
                continue;
            }
            _ => {}
        }
        let (key, item) = parse_key_value(scanner, false, depth)?;
        entries.push((key, item));
    }
    let raw = scanner.slice(start, scanner.pos);
    let value = Value::Object(ObjectValue::new(
        entries
            .iter()
            .map(|(key, item)| Ok((key.name().to_owned(), item.unwrap_value()?)))
            .collect::<Result<Vec<_>, _>>()?,
    )?);
    Ok(Item::InlineTable {
        value,
        raw,
        trivia: Trivia::default(),
        entries,
    })
}

#[allow(clippy::too_many_lines)]
fn parse_table(
    scanner: &mut Scanner<'_>,
    parent: Option<&Key>,
    aot_stack: &mut Vec<Vec<String>>,
) -> Result<(Key, Item), ValueError> {
    let indent = scanner.slice(scanner.line_start, scanner.pos);
    scanner.step();
    if scanner.end() {
        return Err(scanner.parse_error("Unexpected end of file"));
    }
    let is_aot = scanner.cur() == Some(b'[');
    if is_aot {
        scanner.step();
    }
    let mut key = parse_key(scanner)?;
    if scanner.end() {
        return Err(scanner.parse_error("Unexpected end of file"));
    }
    if scanner.cur() != Some(b']') {
        let current = scanner.cur().map_or_else(
            || String::from("'\\x00'"),
            |byte| format!("{:?}", byte as char),
        );
        return Err(scanner.parse_error(&format!("Unexpected character: {current}")));
    }
    key.sep = String::new();
    let full_key = key.clone();
    let full_name = full_key.as_string();
    if key
        .parts
        .iter()
        .any(|part| !part.quoted && part.name.contains(' '))
    {
        return Err(scanner.parse_error(&format!("Invalid table name \"{full_name}\"")));
    }
    let parent_parts = parent.map_or(0, |key| key.parts.len());
    let relative: Vec<KeyPart> = key.parts.drain(parent_parts..).collect();
    scanner.step();
    if is_aot {
        if scanner.end() {
            return Err(scanner.parse_error("Unexpected end of file"));
        }
        if scanner.cur() != Some(b']') {
            let current = scanner.cur().map_or_else(
                || String::from("'\\x00'"),
                |byte| format!("{:?}", byte as char),
            );
            return Err(scanner.parse_error(&format!("Unexpected character: {current}")));
        }
        scanner.step();
    }
    let mut trivia = scanner.comment_trail(true)?;
    trivia.indent = indent;
    let result_key = relative
        .first()
        .map_or_else(|| full_key.clone(), |first| part_key(first, false));
    let full_names = names_of(&full_key);
    let mut leaf_holder = Table {
        body: Container::default(),
        trivia: trivia.clone(),
        aot_element: is_aot,
        implicit: false,
        display_name: Some(full_name),
    };
    parse_table_body(scanner, &mut leaf_holder, &full_key, aot_stack)?;
    if relative.len() > 1 {
        let rest = relative.get(1..).unwrap_or_default();
        let outer_aot_element = is_aot
            && relative.first().is_some_and(|first| {
                aot_stack.iter().any(|names| {
                    names.len() == 1 && names.first().is_some_and(|name| *name == first.name)
                })
            });
        let mut outer = Table {
            body: Container::default(),
            trivia: Trivia {
                indent: String::new(),
                comment_ws: trivia.comment_ws.clone(),
                comment: trivia.comment.clone(),
                trail: trivia.trail.clone(),
            },
            aot_element: outer_aot_element,
            implicit: true,
            display_name: None,
        };
        let mut node = if is_aot {
            Item::Aot(vec![leaf_holder])
        } else {
            Item::Table(leaf_holder)
        };
        for level_key in rest.iter().skip(1).rev() {
            let mut level = Table {
                body: Container::default(),
                trivia: trivia.clone(),
                aot_element: false,
                implicit: true,
                display_name: None,
            };
            level
                .body
                .raw_append(Some(part_key(level_key, false)), node);
            node = Item::Table(level);
        }
        if let Some(outer_key) = rest.first() {
            outer
                .body
                .raw_append(Some(part_key(outer_key, false)), node);
        }
        return Ok((result_key, Item::Table(outer)));
    }
    if is_aot && aot_stack.last() != Some(&full_names) {
        let members = bundle_aot(scanner, leaf_holder, &full_key)?;
        return Ok((result_key, Item::Aot(members)));
    }
    Ok((result_key, Item::Table(leaf_holder)))
}

fn names_of(key: &Key) -> Vec<String> {
    key.parts.iter().map(|part| part.name.clone()).collect()
}

fn parse_table_body(
    scanner: &mut Scanner<'_>,
    table: &mut Table,
    full_key: &Key,
    aot_stack: &mut Vec<Vec<String>>,
) -> Result<(), ValueError> {
    while !scanner.end() {
        let Some((key, item)) = parse_statement(scanner)? else {
            if scanner.cur() == Some(b'[') {
                let next = peek_table(scanner)?;
                if is_child(full_key, &next) {
                    let (child_key, child_item) = parse_table(scanner, Some(full_key), aot_stack)?;
                    table.body.raw_append(Some(child_key), child_item);
                    continue;
                }
                break;
            }
            return Err(ValueError::new("internal TOML parse state"));
        };
        if !merge_whitespace(&mut table.body.body, &item) {
            table.body.append(key, item, true)?;
        }
    }
    validate_out_of_order(&table.body)
}

fn peek_table(scanner: &mut Scanner<'_>) -> Result<Key, ValueError> {
    let start = scanner.pos;
    let line = scanner.line;
    let line_start = scanner.line_start;
    scanner.step();
    if scanner.cur() == Some(b'[') {
        scanner.step();
    }
    let key = parse_key(scanner);
    scanner.pos = start;
    scanner.line = line;
    scanner.line_start = line_start;
    key
}

fn is_child(parent: &Key, child: &Key) -> bool {
    let parent_names = parent.names();
    let child_names = child.names();
    if parent_names == child_names {
        return false;
    }
    parent_names.len() <= child_names.len()
        && parent_names
            .iter()
            .zip(child_names.iter())
            .all(|(left, right)| left == right)
}

fn bundle_aot(
    scanner: &mut Scanner<'_>,
    first: Table,
    key: &Key,
) -> Result<Vec<Table>, ValueError> {
    let names = names_of(key);
    let mut members = vec![first];
    while !scanner.end() {
        let start = scanner.pos;
        let line = scanner.line;
        let line_start = scanner.line_start;
        scanner.step();
        let is_aot = scanner.cur() == Some(b'[');
        if is_aot {
            scanner.step();
        }
        let next = parse_key(scanner);
        let next_ok = matches!(&next, Ok(key) if is_aot && names_of(key) == names);
        scanner.pos = start;
        scanner.line = line;
        scanner.line_start = line_start;
        if !next_ok {
            break;
        }
        let mut aot_stack = vec![names.clone()];
        let (_, item) = parse_table(scanner, Some(key), &mut aot_stack)?;
        match item {
            Item::Table(table) => members.push(table),
            _ => break,
        }
    }
    Ok(members)
}

// -------------------------------------------------- container append logic

impl Container {
    fn raw_append(&mut self, key: Option<Key>, item: Item) {
        if is_table_item(&item)
            && let Some(key) = &key
        {
            self.table_keys.push(key.clone());
        }
        self.body.push(Entry { key, item });
    }

    fn append(&mut self, key: Option<Key>, item: Item, parsed: bool) -> Result<(), ValueError> {
        if let Some(dotted) = key.as_ref().filter(|key| key.parts.len() > 1) {
            return self.handle_dotted_key(dotted, item, parsed);
        }
        let name = key.as_ref().map(|key| key.name().to_owned());
        if let Some(name) = &name
            && let Some(index) = self
                .body
                .iter()
                .rposition(|entry| entry.key.as_ref().is_some_and(|other| other.name() == name))
        {
            return self.append_existing(key, item, index, parsed);
        }
        if name.is_some()
            && !self.body.is_empty()
            && !parsed
            && let Some(key) = &key
        {
            let table_like = is_table_item(&item) && !key.dotted;
            if !table_like {
                let index = last_index_before_table(&self.body);
                if index < self.body.len() {
                    if index > 0
                        && let Some(entry) = self.body.get_mut(index - 1)
                    {
                        trail_before_insert(&mut entry.item);
                    }
                    self.body.insert(
                        index,
                        Entry {
                            key: Some(key.clone()),
                            item,
                        },
                    );
                    return Ok(());
                }
                if let Some(entry) = self.body.last_mut() {
                    trail_before_append(&mut entry.item);
                }
                self.raw_append(Some(key.clone()), item);
                return Ok(());
            }
        }
        let item = glue_new_item(self, item, parsed);
        self.raw_append(key, item);
        Ok(())
    }

    fn handle_dotted_key(&mut self, key: &Key, item: Item, parsed: bool) -> Result<(), ValueError> {
        if is_table_item(&item) {
            return Err(ValueError::new("Can't add a table to a dotted key"));
        }
        let Some((leaf_part, head)) = key.parts.split_last() else {
            return Err(ValueError::new("Can't add a table to a dotted key"));
        };
        let Some(outer_part) = head.first() else {
            return Err(ValueError::new("Can't add a table to a dotted key"));
        };
        let mut chain = Table {
            body: Self::default(),
            trivia: Trivia::default(),
            aot_element: false,
            implicit: true,
            display_name: None,
        };
        let mut current = &mut chain.body;
        for part in head.iter().skip(1) {
            let nested = Table {
                body: Self::default(),
                trivia: Trivia::default(),
                aot_element: false,
                implicit: true,
                display_name: None,
            };
            let entry_key = part_key(part, true);
            current.raw_append(Some(entry_key), Item::Table(nested));
            let Some(entry) = current.body.last_mut() else {
                return Err(ValueError::new("Can't add a table to a dotted key"));
            };
            let Item::Table(table) = &mut entry.item else {
                return Err(ValueError::new("Can't add a table to a dotted key"));
            };
            current = &mut table.body;
        }
        let mut leaf_key = part_key(leaf_part, false);
        leaf_key.sep.clone_from(&key.sep);
        current.raw_append(Some(leaf_key), item);
        let outer_key = part_key(outer_part, true);
        self.append(Some(outer_key), Item::Table(chain), parsed)
    }

    #[allow(clippy::too_many_lines)]
    fn append_existing(
        &mut self,
        key: Option<Key>,
        item: Item,
        index: usize,
        parsed: bool,
    ) -> Result<(), ValueError> {
        let name = key
            .as_ref()
            .map(|key| key.name().to_owned())
            .unwrap_or_default();
        let (current_kind_table, existing_super, existing_dotted, last_table_key_is_entry) = {
            let Some(entry) = self.body.get(index) else {
                return Err(dup_error(&name));
            };
            (
                is_table_item(&entry.item),
                table_of(&entry.item).is_some_and(|table| table.implicit),
                entry.key.as_ref().is_some_and(|key| key.dotted),
                self.table_keys
                    .last()
                    .zip(entry.key.as_ref())
                    .is_some_and(|(last, key)| last == key),
            )
        };
        match &item {
            Item::Table(table) => {
                if !current_kind_table {
                    return Err(dup_error(&name));
                }
                if table.aot_element {
                    let Some(entry) = self.body.get_mut(index) else {
                        return Err(dup_error(&name));
                    };
                    if let Item::Aot(members) = &mut entry.item {
                        members.push(table.clone());
                        return Ok(());
                    }
                    let current = std::mem::replace(&mut entry.item, Item::Null);
                    let Item::Table(current_table) = current else {
                        return Err(dup_error(&name));
                    };
                    entry.item = Item::Aot(vec![current_table, table.clone()]);
                    return Ok(());
                }
                if let Some(entry) = self.body.get_mut(index)
                    && let Item::Aot(members) = &mut entry.item
                {
                    if table.implicit && !members.is_empty() {
                        let entries = table.body.body.clone();
                        let Some(last_member) = members.last_mut() else {
                            return Err(dup_error(&name));
                        };
                        for entry in entries {
                            last_member.body.append(
                                entry.key.clone(),
                                entry.item.clone(),
                                parsed,
                            )?;
                        }
                        return Ok(());
                    }
                    return Err(dup_error(&name));
                }
                if existing_super {
                    if table.implicit {
                        let split = key.as_ref().is_some_and(|key| key.dotted)
                            || existing_dotted
                            || !last_table_key_is_entry;
                        if split {
                            let dotted = key.as_ref().is_some_and(|key| key.dotted) && !parsed;
                            let position = if dotted {
                                last_index_before_table(&self.body)
                            } else {
                                self.body.len()
                            };
                            let item = glue_new_item(self, item, parsed);
                            if position < self.body.len() {
                                self.body.insert(position, Entry { key, item });
                            } else {
                                self.raw_append(key, item);
                            }
                            return validate_out_of_order(self);
                        }
                        let entries = table.body.body.clone();
                        if let Some(entry) = self.body.get_mut(index)
                            && let Item::Table(existing_table) = &mut entry.item
                        {
                            for entry in entries {
                                existing_table.body.append(
                                    entry.key.clone(),
                                    entry.item.clone(),
                                    parsed,
                                )?;
                            }
                        }
                        return Ok(());
                    }
                    if existing_dotted {
                        return Err(redefinition_error());
                    }
                    if let Some(existing_table) =
                        self.body.get(index).and_then(|entry| table_of(&entry.item))
                    {
                        validate_candidate(existing_table, table)?;
                    }
                    let item = glue_new_item(self, item, parsed);
                    self.raw_append(key, item);
                    return validate_out_of_order(self);
                }
                if !table.implicit {
                    return Err(dup_error(&name));
                }
                if let Some(existing_table) =
                    self.body.get(index).and_then(|entry| table_of(&entry.item))
                {
                    validate_candidate(existing_table, table)?;
                }
                let item = glue_new_item(self, item, parsed);
                self.raw_append(key, item);
                validate_out_of_order(self)
            }
            Item::Aot(members) => {
                let Some(entry) = self.body.get_mut(index) else {
                    return Err(dup_error(&name));
                };
                if let Item::Aot(existing) = &mut entry.item {
                    existing.extend(members.clone());
                    return Ok(());
                }
                Err(dup_error(&name))
            }
            _ => Err(dup_error(&name)),
        }
    }
}

fn glue_new_item(container: &Container, item: Item, parsed: bool) -> Item {
    if parsed || container.body.is_empty() {
        return item;
    }
    let prev_whitespace = ends_with_whitespace(&container.body);
    let mut item = item;
    match &mut item {
        Item::Table(table) => {
            if table.trivia.indent.is_empty() && !prev_whitespace {
                table.trivia.indent = String::from("\n");
            }
            item
        }
        Item::Aot(members) => {
            if let Some(first) = members.first_mut()
                && !first.trivia.indent.contains('\n')
                && !prev_whitespace
            {
                first.trivia.indent = format!("\n{}", first.trivia.indent);
            }
            item
        }
        other => other.clone(),
    }
}

fn ends_with_whitespace(body: &[Entry]) -> bool {
    body.iter()
        .rev()
        .find(|entry| !matches!(entry.item, Item::Null))
        .is_some_and(|entry| item_ends_with_whitespace(&entry.item))
}

fn item_ends_with_whitespace(item: &Item) -> bool {
    match item {
        Item::Whitespace(_) => true,
        Item::Table(table) => ends_with_whitespace(&table.body.body),
        Item::Aot(members) => members
            .last()
            .is_some_and(|member| ends_with_whitespace(&member.body.body)),
        _ => false,
    }
}

const fn any_trivia(item: &Item) -> Option<&Trivia> {
    match item {
        Item::Value { trivia, .. }
        | Item::Array { trivia, .. }
        | Item::InlineTable { trivia, .. }
        | Item::Comment(trivia) => Some(trivia),
        Item::Table(table) => Some(&table.trivia),
        _ => None,
    }
}

const fn any_trivia_mut(item: &mut Item) -> Option<&mut Trivia> {
    match item {
        Item::Value { trivia, .. }
        | Item::Array { trivia, .. }
        | Item::InlineTable { trivia, .. }
        | Item::Comment(trivia) => Some(trivia),
        Item::Table(table) => Some(&mut table.trivia),
        _ => None,
    }
}

fn trail_before_insert(item: &mut Item) {
    if matches!(item, Item::Whitespace(_)) || item_ends_with_whitespace(item) {
        return;
    }
    if let Some(trivia) = any_trivia_mut(item)
        && !trivia.trail.contains('\n')
    {
        trivia.trail.push('\n');
    }
}

fn separate_before_table(item: &mut Item) {
    let Some(trivia) = any_trivia_mut(item) else {
        return;
    };
    let newline = if trivia.trail.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if !trivia.trail.ends_with(&format!("{newline}{newline}")) {
        trivia.trail.push_str(newline);
    }
}

fn trail_before_append(item: &mut Item) {
    if let Item::Table(table) = item
        && table.implicit
        && table.trivia.trail.contains('\n')
    {
        let trail = table.trivia.trail.clone();
        if let Some(entry) = table
            .body
            .body
            .iter_mut()
            .rev()
            .find(|entry| !matches!(entry.item, Item::Null))
            && !matches!(entry.item, Item::Whitespace(_))
        {
            let has_newline =
                any_trivia(&entry.item).is_some_and(|trivia| trivia.trail.contains('\n'));
            if !has_newline && let Some(trivia) = any_trivia_mut(&mut entry.item) {
                trivia.trail.push_str(&trail);
            }
        }
    }
    if matches!(item, Item::Whitespace(_)) || item_ends_with_whitespace(item) {
        return;
    }
    if any_trivia(item).is_some_and(|trivia| trivia.trail.contains('\n')) {
        return;
    }
    if let Some(trivia) = any_trivia_mut(item) {
        trivia.trail.push('\n');
    }
}

fn last_index_before_table(body: &[Entry]) -> usize {
    let mut last_index: Option<usize> = None;
    for (index, entry) in body.iter().enumerate() {
        match &entry.item {
            Item::Null | Item::Whitespace(_) => continue,
            Item::Table(_) | Item::Aot(_) if entry.key.as_ref().is_some_and(|key| !key.dotted) => {
                break;
            }
            Item::Table(table)
                if entry.key.as_ref().is_some_and(|key| key.dotted)
                    && renders_table_header(table) =>
            {
                break;
            }
            _ => {}
        }
        last_index = Some(index);
    }
    last_index.map_or(0, |index| index + 1)
}

fn renders_table_header(table: &Table) -> bool {
    for entry in &table.body.body {
        match &entry.item {
            Item::Aot(_) => return true,
            Item::Table(child) => {
                let dotted = entry.key.as_ref().is_some_and(|key| key.dotted);
                if dotted && child.implicit {
                    if renders_table_header(child) {
                        return true;
                    }
                } else {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn validate_candidate(current: &Table, candidate: &Table) -> Result<(), ValueError> {
    for entry in &candidate.body.body {
        let Some(key) = &entry.key else {
            continue;
        };
        let name = key.name();
        let existing = current
            .body
            .body
            .iter()
            .find(|other| other.key.as_ref().is_some_and(|other| other.name() == name));
        if let Some(found) = existing {
            let kinds_match = is_table_item(&found.item) == is_table_item(&entry.item);
            if !kinds_match {
                return Err(dup_error(name));
            }
            if key.dotted {
                return Err(redefinition_error());
            }
            if let (Item::Table(left), Item::Table(right)) = (&found.item, &entry.item) {
                if !left.implicit && !right.implicit {
                    return Err(dup_error(name));
                }
                validate_candidate(left, right)?;
            }
        } else {
            if !key.dotted {
                let prefixed = current.body.body.iter().any(|other| {
                    other
                        .key
                        .as_ref()
                        .is_some_and(|other| other.dotted && other.name() == name)
                });
                if prefixed {
                    return Err(redefinition_error());
                }
                continue;
            }
            if current
                .body
                .body
                .iter()
                .any(|other| other.key.as_ref().is_some_and(|other| other.name() == name))
            {
                return Err(redefinition_error());
            }
        }
    }
    Ok(())
}

fn validate_out_of_order(container: &Container) -> Result<(), ValueError> {
    for (index, entry) in container.body.iter().enumerate() {
        let Some(key) = &entry.key else {
            continue;
        };
        let indices: Vec<usize> = container
            .body
            .iter()
            .enumerate()
            .filter(|(other, other_entry)| {
                *other != index
                    && other_entry.key.as_ref().is_some_and(|other_key| {
                        other_key.name() == key.name() && other_key.dotted == key.dotted
                    })
            })
            .map(|(other, _)| other)
            .collect();
        if indices.is_empty() {
            continue;
        }
        let mut merged = Container::default();
        let mut parts: Vec<usize> = indices;
        parts.push(index);
        parts.sort_unstable();
        for part in parts {
            if let Some(Item::Table(table)) = container.body.get(part).map(|entry| &entry.item) {
                for entry in &table.body.body {
                    merged.append(entry.key.clone(), entry.item.clone(), true)?;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- rendering

fn render_simple(key: Option<&Key>, item: &Item, prefix: Option<&str>) -> String {
    let Some(key) = key else {
        return item.as_string();
    };
    let key_text = key.as_string();
    let key_text = match prefix {
        Some(prefix) if !key_text.is_empty() => format!("{prefix}.{key_text}"),
        _ => key_text,
    };
    let Some(trivia) = item.trivia() else {
        return item.as_string();
    };
    format!(
        "{}{}{}{}{}{}{}",
        trivia.indent,
        key_text,
        key.sep,
        item.as_string(),
        trivia.comment_ws,
        trivia.comment,
        trivia.trail
    )
}

fn render_table_body(table: &Table) -> String {
    let mut text = String::new();
    for entry in &table.body.body {
        match &entry.item {
            Item::Table(child) => {
                if !text.trim_matches(' ').is_empty()
                    && !text.trim_matches(' ').ends_with('\n')
                    && !child.trivia.indent.contains('\n')
                {
                    text.push('\n');
                }
                text.push_str(&render_table(entry.key.as_ref(), child, None));
            }
            Item::Aot(members) => {
                if !text.trim_matches(' ').is_empty()
                    && !text.trim_matches(' ').ends_with('\n')
                    && !members
                        .first()
                        .is_some_and(|member| member.trivia.indent.contains('\n'))
                {
                    text.push('\n');
                }
                text.push_str(&render_aot(entry.key.as_ref(), members, None));
            }
            item => {
                text.push_str(&render_simple(entry.key.as_ref(), item, None));
            }
        }
    }
    text
}

fn render_table(key: Option<&Key>, table: &Table, prefix: Option<&str>) -> String {
    let mut cur = String::new();
    let dotted = key.is_some_and(|key| key.dotted);
    let key_text = table.display_name.clone().unwrap_or_else(|| {
        let name = key.map_or(String::new(), Key::as_string);
        match prefix {
            Some(prefix) if !name.is_empty() => format!("{prefix}.{name}"),
            _ => name,
        }
    });
    let direct_values = table.body.body.iter().any(|entry| {
        !matches!(
            entry.item,
            Item::Table(_) | Item::Aot(_) | Item::Whitespace(_) | Item::Null
        )
    });
    let nested_dotted = !dotted
        && table.body.body.iter().any(|entry| {
            matches!(entry.item, Item::Table(_)) && entry.key.as_ref().is_some_and(|key| key.dotted)
        });
    if !table.implicit || (direct_values && !dotted) || nested_dotted {
        let (open, close) = if table.aot_element {
            ("[[", "]]")
        } else {
            ("[", "]")
        };
        let newline_in_trivia = if !table.trivia.trail.contains('\n') && !table.body.body.is_empty()
        {
            "\n"
        } else {
            ""
        };
        let header = format!(
            "{}{}{}{}{}{}{}{}",
            table.trivia.indent,
            open,
            key_text,
            close,
            table.trivia.comment_ws,
            table.trivia.comment,
            table.trivia.trail,
            newline_in_trivia
        );
        cur.push_str(&header);
    } else if table.trivia.indent == "\n" {
        cur.push_str(&table.trivia.indent);
    }
    for entry in &table.body.body {
        match &entry.item {
            Item::Table(child) => {
                if !cur.trim_matches(' ').is_empty()
                    && !cur.trim_matches(' ').ends_with('\n')
                    && !child.trivia.indent.contains('\n')
                {
                    cur.push('\n');
                }
                let child_prefix = if child.implicit
                    && entry.key.as_ref().is_some_and(|key| key.dotted)
                    && !dotted
                {
                    None
                } else {
                    Some(key_text.as_str())
                };
                cur.push_str(&render_table(entry.key.as_ref(), child, child_prefix));
            }
            Item::Aot(members) => {
                if !cur.trim_matches(' ').is_empty()
                    && !cur.trim_matches(' ').ends_with('\n')
                    && !members
                        .first()
                        .is_some_and(|member| member.trivia.indent.contains('\n'))
                {
                    cur.push('\n');
                }
                cur.push_str(&render_aot(
                    entry.key.as_ref(),
                    members,
                    Some(key_text.as_str()),
                ));
            }
            item => {
                cur.push_str(&render_simple(
                    entry.key.as_ref(),
                    item,
                    if dotted {
                        Some(key_text.as_str())
                    } else {
                        None
                    },
                ));
            }
        }
    }
    cur
}

fn render_aot(key: Option<&Key>, members: &[Table], prefix: Option<&str>) -> String {
    let name = key.map_or(String::new(), Key::as_string);
    let name = match prefix {
        Some(prefix) if !name.is_empty() => format!("{prefix}.{name}"),
        _ => name,
    };
    let mut cur = String::new();
    for member in members {
        cur.push_str(&render_aot_table(member, &name));
    }
    cur
}

fn render_aot_table(table: &Table, key_text: &str) -> String {
    let mut cur = format!(
        "{}[[{}]]{}{}{}",
        table.trivia.indent,
        key_text,
        table.trivia.comment_ws,
        table.trivia.comment,
        table.trivia.trail
    );
    for entry in &table.body.body {
        match &entry.item {
            Item::Table(child) => {
                let child_prefix =
                    if child.implicit && entry.key.as_ref().is_some_and(|key| key.dotted) {
                        None
                    } else {
                        Some(key_text)
                    };
                cur.push_str(&render_table(entry.key.as_ref(), child, child_prefix));
            }
            Item::Aot(members) => {
                cur.push_str(&render_aot(entry.key.as_ref(), members, Some(key_text)));
            }
            item => {
                cur.push_str(&render_simple(entry.key.as_ref(), item, None));
            }
        }
    }
    cur
}

fn render_root(root: &Container) -> String {
    let mut text = String::new();
    for entry in &root.body {
        match &entry.item {
            Item::Table(table) => {
                if !text.trim_matches(' ').is_empty()
                    && !text.trim_matches(' ').ends_with('\n')
                    && !table.trivia.indent.contains('\n')
                {
                    text.push('\n');
                }
                text.push_str(&render_table(entry.key.as_ref(), table, None));
            }
            Item::Aot(members) => {
                if !text.trim_matches(' ').is_empty()
                    && !text.trim_matches(' ').ends_with('\n')
                    && !members
                        .first()
                        .is_some_and(|member| member.trivia.indent.contains('\n'))
                {
                    text.push('\n');
                }
                text.push_str(&render_aot(entry.key.as_ref(), members, None));
            }
            item => {
                text.push_str(&render_simple(entry.key.as_ref(), item, None));
            }
        }
    }
    text
}

// ---------------------------------------------------------------- fragment

fn render_toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn render_inline_value(value: &Value) -> Result<String, ValueError> {
    match value {
        Value::Scalar(scalar) => render_toml_scalar(scalar),
        Value::Array(array) => {
            let members = array
                .items()
                .iter()
                .map(render_inline_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("[{}]", members.join(", ")))
        }
        Value::Object(object) => {
            let members = object
                .items()
                .iter()
                .filter(|(_, value)| !is_table_value(value))
                .chain(
                    object
                        .items()
                        .iter()
                        .filter(|(_, value)| is_table_value(value)),
                )
                .map(|(key, item)| {
                    Ok(format!(
                        "{} = {}",
                        render_toml_key(key),
                        render_inline_value(item)?
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{{{}}}", members.join(", ")))
        }
    }
}

fn is_table_value(value: &Value) -> bool {
    match value {
        Value::Object(_) => true,
        Value::Array(array) => {
            !array.items().is_empty()
                && array
                    .items()
                    .iter()
                    .all(|item| matches!(item, Value::Object(_)))
        }
        Value::Scalar(_) => false,
    }
}

fn validate_selected_floats(node: &Item) -> Result<(), ValueError> {
    let value = node.unwrap_value()?;
    let mut pending = vec![&value];
    while let Some(node) = pending.pop() {
        match node {
            Value::Scalar(scalar) => {
                if let ScalarKind::Float(number) = scalar.kind()
                    && !number.is_finite()
                {
                    return Err(ValueError::new("floats must be finite"));
                }
            }
            Value::Object(object) => pending.extend(object.items().iter().map(|(_, child)| child)),
            Value::Array(array) => pending.extend(array.items()),
        }
    }
    Ok(())
}

fn render_toml_scalar(scalar: &Scalar) -> Result<String, ValueError> {
    match scalar.kind() {
        ScalarKind::Null => Err(ValueError::new(
            "Unable to convert an object of <class 'NoneType'> to a TOML item",
        )),
        ScalarKind::Boolean(value) => Ok(if *value { "true" } else { "false" }.to_owned()),
        ScalarKind::Integer(value) => Ok(value.to_string()),
        ScalarKind::Float(value) => Ok(toml_float_repr(*value)),
        ScalarKind::Text(value) => Ok(render_toml_string(value)),
        ScalarKind::Date(value) => Ok(value.to_string()),
        ScalarKind::Time(value) => Ok(render_time(*value)),
        ScalarKind::DateTime(value) => Ok(render_datetime(value)),
    }
}

fn toml_float_repr(value: f64) -> String {
    if value.is_nan() {
        return String::from("nan");
    }
    if value.is_infinite() {
        return String::from(if value < 0.0 { "-inf" } else { "inf" });
    }
    python_float_repr(value)
}

fn render_time(time: Time) -> String {
    if time.microsecond() == 0 {
        format!(
            "{:02}:{:02}:{:02}",
            time.hour(),
            time.minute(),
            time.second()
        )
    } else {
        format!(
            "{:02}:{:02}:{:02}.{:06}",
            time.hour(),
            time.minute(),
            time.second(),
            time.microsecond()
        )
    }
}

fn render_datetime(datetime: &DateTime) -> String {
    let time = render_time(datetime.time());
    if datetime.is_coordinated() {
        format!("{}T{}Z", datetime.date(), time)
    } else {
        format!("{}T{}", datetime.date(), time)
    }
}

fn render_toml_key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if bare {
        name.to_owned()
    } else {
        render_toml_string(name)
    }
}

fn generated_fragment(value: &Value) -> Result<String, ValueError> {
    let mut body = Container::default();
    let entry_key = single_key("value", false);
    let item = generated_item(value)?;
    body.append(Some(entry_key), item, false)?;
    Ok(render_root(&body))
}

fn generated_item(value: &Value) -> Result<Item, ValueError> {
    match value {
        Value::Scalar(scalar) => Ok(Item::Value {
            raw: render_toml_scalar(scalar)?,
            value: value.clone(),
            trivia: Trivia {
                trail: String::from("\n"),
                ..Trivia::default()
            },
        }),
        Value::Array(array) => {
            let all_objects = array
                .items()
                .iter()
                .all(|item| matches!(item, Value::Object(_)));
            if all_objects && !array.items().is_empty() {
                let mut members = Vec::new();
                for item in array.items() {
                    members.push(generated_table(item)?);
                }
                Ok(Item::Aot(members))
            } else {
                Ok(Item::Value {
                    raw: render_inline_value(value)?,
                    value: value.clone(),
                    trivia: Trivia {
                        trail: String::from("\n"),
                        ..Trivia::default()
                    },
                })
            }
        }
        Value::Object(_) => Ok(Item::Table(generated_table(value)?)),
    }
}

fn generated_table(value: &Value) -> Result<Table, ValueError> {
    let Value::Object(object) = value else {
        return Err(ValueError::new(
            "Unable to convert an object of <class 'NoneType'> to a TOML item",
        ));
    };
    let mut table = Table {
        body: Container::default(),
        trivia: Trivia {
            trail: String::from("\n"),
            ..Trivia::default()
        },
        aot_element: false,
        implicit: false,
        display_name: None,
    };
    for (name, item) in object.items() {
        let key = single_key(name, false);
        table.body.append(Some(key), generated_item(item)?, false)?;
    }
    if let Some(index) = table
        .body
        .body
        .iter()
        .position(|entry| is_table_item(&entry.item))
        && index > 0
        && table.body.body.get(index).is_some_and(|entry| {
            !matches!(entry.item, Item::Aot(_))
                && any_trivia(&entry.item).is_none_or(|trivia| !trivia.indent.contains('\n'))
        })
        && let Some(entry) = table.body.body.get_mut(index - 1)
    {
        separate_before_table(&mut entry.item);
    }
    Ok(table)
}

// ----------------------------------------------------------------- coalesce

pub fn coalesce(container: Container) -> Container {
    let mut result = Container::default();
    for mut entry in container.body {
        match &mut entry.item {
            Item::Table(table) => {
                let taken = std::mem::take(&mut table.body);
                table.body = coalesce(taken);
            }
            Item::Aot(members) => {
                for member in members.iter_mut() {
                    let taken = std::mem::take(&mut member.body);
                    member.body = coalesce(taken);
                }
            }
            _ => {}
        }
        let dotted = entry.key.as_ref().is_some_and(|key| key.dotted);
        if !result.body.is_empty() && dotted && matches!(entry.item, Item::Table(_)) {
            let mut index = result.body.len() - 1;
            while index > 0
                && result.body.get(index).is_some_and(|entry| {
                    matches!(entry.item, Item::Comment(_) | Item::Whitespace(_))
                })
            {
                index -= 1;
            }
            let mergeable = result.body.get(index).is_some_and(|previous_entry| {
                previous_entry
                    .key
                    .as_ref()
                    .zip(entry.key.as_ref())
                    .is_some_and(|(previous, key)| {
                        previous.dotted && previous.as_string() == key.as_string()
                    })
                    && matches!(previous_entry.item, Item::Table(_))
            });
            if mergeable {
                let Some(previous) = result.body.get_mut(index) else {
                    continue;
                };
                let Item::Table(previous_table) = &mut previous.item else {
                    continue;
                };
                let mut merged = std::mem::take(previous_table);
                let gaps = result.body.drain(index + 1..).collect::<Vec<Entry>>();
                let mut merged_body = std::mem::take(&mut merged.body);
                merged_body.body.extend(gaps);
                let Item::Table(incoming) = entry.item else {
                    if let Some(target) = result.body.get_mut(index) {
                        target.item = Item::Table(merged);
                    }
                    continue;
                };
                merged_body.body.extend(incoming.body.body);
                merged.body = coalesce(merged_body);
                if let Some(target) = result.body.get_mut(index) {
                    target.item = Item::Table(merged);
                }
                continue;
            }
        }
        result.raw_append(entry.key, entry.item);
    }
    result
}

// ------------------------------------------------------------- value views

fn unwrap_body(body: &Container) -> Result<Value, ValueError> {
    let mut pairs: Vec<(String, Value)> = Vec::new();
    for entry in &body.body {
        let Some(key) = &entry.key else {
            continue;
        };
        let name = key.name().to_owned();
        let value = entry.item.unwrap_value()?;
        match pairs.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing)) => merge_values(existing, &value)?,
            None => pairs.push((name, value)),
        }
    }
    Ok(Value::Object(ObjectValue::new(pairs)?))
}

fn merge_values(target: &mut Value, incoming: &Value) -> Result<(), ValueError> {
    match (&mut *target, incoming) {
        (Value::Object(existing), Value::Object(fresh)) => {
            let mut pairs: Vec<(String, Value)> = existing.items().to_vec();
            for (name, value) in fresh.items() {
                match pairs.iter().position(|(key, _)| key == name) {
                    Some(index) => {
                        if let Some(entry) = pairs.get_mut(index) {
                            let mut merged = entry.1.clone();
                            merge_values(&mut merged, value)?;
                            entry.1 = merged;
                        }
                    }
                    None => pairs.push((name.clone(), value.clone())),
                }
            }
            *existing = ObjectValue::new(pairs)?;
            Ok(())
        }
        (Value::Array(existing), Value::Array(fresh)) => {
            let mut items: Vec<Value> = existing.items().to_vec();
            items.extend(fresh.items().iter().cloned());
            *existing = ArrayValue::new(items);
            Ok(())
        }
        _ => Ok(()),
    }
}

enum Located<'a> {
    Item(&'a Item),
    Table(&'a Table),
    Tables(Vec<&'a Table>),
    Aot(Vec<&'a Table>),
}

fn locate<'a>(root: &'a Container, path: &[PathPart]) -> Result<Located<'a>, ValueError> {
    let mut current: Option<Located<'a>> = None;
    for part in path {
        match part {
            PathPart::Key(name) => {
                let found = search_key(current.as_ref(), root, name)?;
                current = Some(found);
            }
            PathPart::Index(index) => {
                let found = search_index(current.as_ref(), root, *index)?;
                current = Some(found);
            }
        }
    }
    current.ok_or_else(|| ValueError::new("document selector path is missing"))
}

const fn located_body<'a>(located: &Located<'a>) -> Option<&'a Container> {
    match located {
        Located::Table(table) => Some(&table.body),
        Located::Item(Item::Table(table)) => Some(&table.body),
        _ => None,
    }
}

fn search_key<'a>(
    current: Option<&Located<'a>>,
    root: &'a Container,
    name: &str,
) -> Result<Located<'a>, ValueError> {
    let bodies: Vec<&'a Container> = match current {
        None => vec![root],
        Some(Located::Item(Item::InlineTable { entries, .. })) => {
            let matches: Vec<&'a Item> = entries
                .iter()
                .filter(|(key, _)| key.name() == name)
                .map(|(_, item)| item)
                .collect();
            return resolve_entries(matches);
        }
        Some(Located::Tables(tables)) => tables.iter().map(|table| &table.body).collect(),
        Some(located) => {
            let Some(body) = located_body(located) else {
                return Err(ValueError::new("TOML selector crosses a scalar"));
            };
            vec![body]
        }
    };
    let mut found_items: Vec<&'a Item> = Vec::new();
    let mut found_tables: Vec<&'a Table> = Vec::new();
    let mut found_aots: Vec<&'a Table> = Vec::new();
    for body in bodies {
        for entry in &body.body {
            let Some(key) = &entry.key else {
                continue;
            };
            if key.name() == name {
                match &entry.item {
                    Item::Table(table) => found_tables.push(table),
                    Item::Aot(members) => found_aots.extend(members),
                    item => found_items.push(item),
                }
            }
        }
    }
    if !found_aots.is_empty() {
        return Ok(Located::Aot(found_aots));
    }
    if found_tables.is_empty() {
        found_items
            .first()
            .map(|item| Located::Item(item))
            .ok_or_else(|| ValueError::new("TOML selector path is missing"))
    } else if found_tables.len() == 1 && found_items.is_empty() {
        found_tables
            .first()
            .map(|table| Located::Table(table))
            .ok_or_else(|| ValueError::new("TOML selector path is missing"))
    } else {
        Ok(Located::Tables(found_tables))
    }
}

fn resolve_entries<'a>(items: Vec<&'a Item>) -> Result<Located<'a>, ValueError> {
    let mut found_items: Vec<&'a Item> = Vec::new();
    let mut found_tables: Vec<&'a Table> = Vec::new();
    let mut found_aots: Vec<&'a Table> = Vec::new();
    for item in items {
        match item {
            Item::Table(table) => found_tables.push(table),
            Item::Aot(members) => found_aots.extend(members),
            other => found_items.push(other),
        }
    }
    if !found_aots.is_empty() {
        return Ok(Located::Aot(found_aots));
    }
    if found_tables.is_empty() {
        found_items
            .first()
            .map(|item| Located::Item(item))
            .ok_or_else(|| ValueError::new("TOML selector path is missing"))
    } else if found_tables.len() == 1 && found_items.is_empty() {
        found_tables
            .first()
            .map(|table| Located::Table(table))
            .ok_or_else(|| ValueError::new("TOML selector path is missing"))
    } else {
        Ok(Located::Tables(found_tables))
    }
}

fn search_index<'a>(
    current: Option<&Located<'a>>,
    root: &'a Container,
    index: usize,
) -> Result<Located<'a>, ValueError> {
    let _ = root;
    match current {
        Some(Located::Item(Item::Aot(members))) => members
            .get(index)
            .map(Located::Table)
            .ok_or_else(|| ValueError::new("document selector path is missing")),
        Some(Located::Aot(members)) => members
            .get(index)
            .map(|member| Located::Table(member))
            .ok_or_else(|| ValueError::new("document selector path is missing")),
        Some(Located::Item(Item::Array { items, .. })) => items
            .get(index)
            .map(Located::Item)
            .ok_or_else(|| ValueError::new("document selector path is missing")),
        _ => Err(ValueError::new("TOML selector crosses a scalar")),
    }
}

fn native_at(root: &Container, path: &[PathPart]) -> Result<Value, ValueError> {
    let value = unwrap_body(root)?;
    let mut current = value;
    for part in path {
        match part {
            PathPart::Key(name) => {
                let Value::Object(object) = current else {
                    return Err(ValueError::new("TOML selector crosses a scalar"));
                };
                current = object
                    .get(name)
                    .cloned()
                    .ok_or_else(|| ValueError::new("document selector path is missing"))?;
            }
            PathPart::Index(index) => {
                let Value::Array(array) = current else {
                    return Err(ValueError::new("TOML selector crosses a scalar"));
                };
                current = array
                    .items()
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| ValueError::new("document selector path is missing"))?;
            }
        }
    }
    Ok(current)
}

// -------------------------------------------------------------- public type

#[derive(Clone, Debug)]
pub struct TomlDocument {
    text: String,
    newline: String,
    root: Container,
}

impl TomlDocument {
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let root = coalesce(parse_document(text)?);
        let mut document = Self {
            text: String::new(),
            newline: newline.to_owned(),
            root,
        };
        document.text = render_root(&document.root);
        Ok(document)
    }

    fn render(&self) -> String {
        render_root(&self.root)
    }

    fn native_root(&self) -> Result<Value, ValueError> {
        unwrap_body(&self.root)
    }

    fn contiguous(&self, path: &[PathPart]) -> Result<(), ValueError> {
        let mut events: Vec<Vec<PathPart>> = Vec::new();
        let mut current: Vec<PathPart> = Vec::new();
        for entry in &self.root.body {
            if let Some(key) = &entry.key {
                current.push(PathPart::Key(key.name().to_owned()));
                collect_events(Located::Item(&entry.item), &mut current, &mut events);
                current.pop();
            }
        }
        let selected: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.starts_with(path))
            .map(|(index, _)| index)
            .collect();
        let contiguous = selected
            .last()
            .is_some_and(|last| selected.first() == Some(&(last + 1 - selected.len())));
        if !selected.is_empty() && !contiguous {
            return Err(ValueError::new(
                "a noncontiguous TOML table cannot be an owned unit",
            ));
        }
        Ok(())
    }

    fn trailing_container_mut(node: &mut Item) -> Option<&mut Container> {
        match node {
            Item::Table(table) => Self::table_trailing_container(table),
            Item::Aot(members) => members.last_mut().and_then(Self::table_trailing_container),
            _ => None,
        }
    }

    fn table_trailing_container(table: &mut Table) -> Option<&mut Container> {
        let tail_is_table = table
            .body
            .body
            .last()
            .is_some_and(|entry| is_table_item(&entry.item));
        if tail_is_table {
            let last = table.body.body.last_mut()?;
            return Self::trailing_container_mut(&mut last.item);
        }
        Some(&mut table.body)
    }

    fn tables_of(located: Located<'_>) -> Vec<&Table> {
        match located {
            Located::Table(table) | Located::Item(Item::Table(table)) => vec![table],
            Located::Tables(tables) => tables,
            Located::Aot(members) => members,
            Located::Item(Item::Aot(members)) => members.iter().collect(),
            Located::Item(_) => Vec::new(),
        }
    }

    fn strip_gap(node: &mut Item) -> String {
        let mut gap = String::new();
        if let Some(container) = Self::trailing_container_mut(node) {
            while container.body.last().is_some_and(|entry| {
                matches!(
                    entry.item,
                    Item::Comment(_) | Item::Whitespace(_) | Item::Null
                )
            }) {
                if let Some(entry) = container.body.pop() {
                    gap = format!("{}{}", entry.item.as_string(), gap);
                }
            }
        }
        gap
    }

    fn append_gap(node: &mut Item, gap: &str) {
        if gap.is_empty() {
            return;
        }
        if let Some(container) = Self::trailing_container_mut(node) {
            container.raw_append(None, Item::Whitespace(gap.to_owned()));
        }
    }

    fn invalidate(node: &mut Item) {
        match node {
            Item::Table(table) => {
                table.display_name = None;
                for entry in &mut table.body.body {
                    Self::invalidate(&mut entry.item);
                }
            }
            Item::Aot(members) => {
                for member in members.iter_mut() {
                    member.display_name = None;
                    for entry in &mut member.body.body {
                        Self::invalidate(&mut entry.item);
                    }
                }
            }
            _ => {}
        }
    }

    fn strip_table_gap(table: &mut Table) -> String {
        let mut gap = String::new();
        if let Some(container) = Self::table_trailing_container(table) {
            while container.body.last().is_some_and(|entry| {
                matches!(
                    entry.item,
                    Item::Comment(_) | Item::Whitespace(_) | Item::Null
                )
            }) {
                if let Some(entry) = container.body.pop() {
                    gap = format!("{}{}", entry.item.as_string(), gap);
                }
            }
        }
        gap
    }

    fn preserve_header(node: &mut Item, current: &[Table]) {
        let Some(live_trivia) = current
            .iter()
            .find(|table| !table.implicit)
            .map(|table| (table.trivia.clone(), table.display_name.clone()))
        else {
            return;
        };
        let index = Self::tables_ref(node)
            .iter()
            .position(|table| !table.implicit)
            .unwrap_or(0);
        if let Some(header) = Self::tables_mut(node).get_mut(index) {
            header.implicit = false;
            header.trivia = live_trivia.0;
            header.display_name = live_trivia.1;
        }
    }

    fn tables_ref(node: &Item) -> Vec<&Table> {
        match node {
            Item::Table(table) => vec![table],
            Item::Aot(members) => members.iter().collect(),
            _ => Vec::new(),
        }
    }

    fn tables_mut(node: &mut Item) -> Vec<&mut Table> {
        match node {
            Item::Table(table) => vec![table],
            Item::Aot(members) => members.iter_mut().collect(),
            _ => Vec::new(),
        }
    }
}

// ------------------------------------------------------- backend read side

fn syntax_fragment(
    document: &TomlDocument,
    path: &[PathPart],
    synthetic: bool,
) -> Result<Vec<u8>, ValueError> {
    let located = locate(&document.root, path)?;
    let mut node = copy_located(&located);
    validate_selected_floats(&node)?;
    if matches!(located, Located::Tables(_)) {
        document.contiguous(path)?;
    }
    TomlDocument::strip_gap(&mut node);
    if let Item::Value { trivia, .. }
    | Item::Array { trivia, .. }
    | Item::InlineTable { trivia, .. } = &mut node
    {
        trivia.indent = String::new();
        trivia.comment_ws = String::new();
        trivia.comment = String::new();
        trivia.trail.clone_from(&document.newline);
    }
    if synthetic && let Item::Table(table) = &mut node {
        table.trivia = Trivia {
            trail: document.newline.clone(),
            ..Trivia::default()
        };
    }
    if let Located::Tables(_) = located {
        let tables = TomlDocument::tables_of(located);
        let mut body = Container::default();
        let count = tables.len();
        for (index, table) in tables.into_iter().enumerate() {
            let mut copy = table.clone();
            if index + 1 == count {
                TomlDocument::strip_table_gap(&mut copy);
            }
            copy.trivia.indent = String::new();
            copy.trivia.comment_ws = String::new();
            copy.trivia.comment = String::new();
            copy.trivia.trail.clone_from(&document.newline);
            let mut item = Item::Table(copy);
            TomlDocument::invalidate(&mut item);
            body.raw_append(Some(single_key("value", false)), item);
        }
        Ok(render_root(&body).into_bytes())
    } else {
        if synthetic {
            TomlDocument::invalidate(&mut node);
        } else if let Item::Table(table) = &mut node {
            table.display_name = None;
        }
        let mut body = Container::default();
        body.raw_append(Some(single_key("value", false)), node);
        Ok(render_root(&body).into_bytes())
    }
}

fn copy_located(located: &Located<'_>) -> Item {
    match located {
        Located::Item(item) => (*item).clone(),
        Located::Table(table) => Item::Table((*table).clone()),
        Located::Tables(tables) => {
            let mut body = Container::default();
            for table in tables {
                body.raw_append(
                    Some(single_key("value", false)),
                    Item::Table((*table).clone()),
                );
            }
            let mut merged = tables
                .first()
                .map_or_else(Table::default, |table| (*table).clone());
            merged.body = body;
            Item::Table(merged)
        }
        Located::Aot(members) => Item::Aot(members.iter().map(|t| (*t).clone()).collect()),
    }
}

fn collect_events(
    located: Located<'_>,
    current: &mut Vec<PathPart>,
    events: &mut Vec<Vec<PathPart>>,
) {
    let tables: Vec<&Table> = match located {
        Located::Table(table) | Located::Item(Item::Table(table)) => {
            if !table.implicit {
                events.push(current.clone());
            }
            vec![table]
        }
        Located::Tables(tables) => {
            if tables.first().is_some_and(|table| !table.implicit) {
                events.push(current.clone());
            }
            tables
        }
        Located::Aot(members) => {
            for (index, member) in members.iter().enumerate() {
                current.push(PathPart::Index(index));
                collect_events(Located::Table(member), current, events);
                current.pop();
            }
            return;
        }
        Located::Item(Item::Aot(members)) => {
            for (index, member) in members.iter().enumerate() {
                current.push(PathPart::Index(index));
                collect_events(Located::Table(member), current, events);
                current.pop();
            }
            return;
        }
        Located::Item(_) => {
            events.push(current.clone());
            return;
        }
    };
    for entry in tables.iter().flat_map(|table| table.body.body.iter()) {
        if let Some(key) = &entry.key {
            current.push(PathPart::Key(key.name().to_owned()));
            collect_events(Located::Item(&entry.item), current, events);
            current.pop();
        }
    }
}

type ArrayParts = (String, Vec<(usize, usize)>, Vec<usize>);

fn array_parts(item: &Item) -> Result<ArrayParts, ValueError> {
    let Item::Array {
        raw, spans, commas, ..
    } = item
    else {
        return Err(ValueError::new("inline array edit requires an array"));
    };
    Ok((raw.clone(), spans.clone(), commas.clone()))
}

// ------------------------------------------------------------- backend ops

impl TomlDocument {
    fn restore_layout(&mut self, path: &[PathPart], layout: &[u8]) -> Result<(), ValueError> {
        for (relative, name) in layout_headers(layout)? {
            let mut expected: Vec<String> = Vec::new();
            for part in path {
                if let PathPart::Key(name) = part {
                    expected.push(name.clone());
                }
            }
            for part in &relative {
                if let LayoutPart::Field(name) = part {
                    expected.push(name.clone());
                }
            }
            if header_path(&name)? != expected {
                return Err(ValueError::new(
                    "TOML restoration header does not match its destination",
                ));
            }
            let mut target = path.to_vec();
            for part in &relative {
                target.push(match part {
                    LayoutPart::Field(name) => PathPart::Key(name.clone()),
                    LayoutPart::Index(index) => PathPart::Index(
                        usize::try_from(*index)
                            .map_err(|_| ValueError::new("invalid TOML restoration path"))?,
                    ),
                });
            }
            let located = locate(&self.root, &target)
                .map_err(|_| ValueError::new("invalid TOML restoration path"))?;
            let Some(last) = target.last().cloned() else {
                return Err(ValueError::new("invalid TOML restoration path"));
            };
            match last {
                PathPart::Key(entry_name) => {
                    let valid = matches!(
                        located,
                        Located::Table(_) | Located::Tables(_) | Located::Aot(_)
                    );
                    if !valid {
                        return Err(ValueError::new("invalid TOML restoration header"));
                    }
                    let container = physical_parent_mut(&mut self.root, &target)?;
                    let positions: Vec<usize> = container
                        .body
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| {
                            entry
                                .key
                                .as_ref()
                                .is_some_and(|key| key.name() == entry_name)
                        })
                        .map(|(index, _)| index)
                        .collect();
                    let chosen = positions
                        .iter()
                        .copied()
                        .find(|index| {
                            container.body.get(*index).is_some_and(|entry| {
                                matches!(&entry.item, Item::Table(table) if !table.implicit)
                            })
                        })
                        .or_else(|| positions.first().copied())
                        .ok_or_else(|| ValueError::new("invalid TOML restoration header"))?;
                    let Some(Item::Table(chosen)) =
                        container.body.get_mut(chosen).map(|entry| &mut entry.item)
                    else {
                        return Err(ValueError::new("invalid TOML restoration header"));
                    };
                    chosen.display_name = Some(name);
                }
                PathPart::Index(index) => {
                    let container = physical_parent_mut(&mut self.root, &target)?;
                    let table = container
                        .body
                        .iter_mut()
                        .find_map(|entry| match &mut entry.item {
                            Item::Aot(members) => members.get_mut(index),
                            _ => None,
                        })
                        .ok_or_else(|| ValueError::new("invalid TOML restoration header"))?;
                    table.display_name = Some(name);
                }
            }
        }
        Ok(())
    }

    fn publish(&mut self) -> Result<(), ValueError> {
        let expected = self.native_root()?;
        let rendered = self.render();
        let parsed = parse_document(&rendered)
            .map_err(|_| ValueError::new("TOML layout cannot preserve valid syntax"))?;
        let root = coalesce(parsed);
        let actual = unwrap_body(&root)?;
        if actual.semantic_bytes() != expected.semantic_bytes() {
            return Err(ValueError::new(
                "TOML layout cannot preserve the selected and foreign values",
            ));
        }
        self.root = root;
        self.text = rendered;
        Ok(())
    }

    fn is_dotted_target(&self, path: &[PathPart]) -> bool {
        let Some((PathPart::Key(name), parent)) = path.split_last() else {
            return false;
        };
        let bodies: Vec<&Container> = if parent.is_empty() {
            vec![&self.root]
        } else {
            match locate(&self.root, parent) {
                Ok(Located::Table(table)) => vec![&table.body],
                Ok(Located::Tables(tables)) => tables.iter().map(|table| &table.body).collect(),
                _ => return false,
            }
        };
        bodies.iter().any(|body| {
            body.body.iter().any(|entry| {
                entry
                    .key
                    .as_ref()
                    .is_some_and(|key| key.dotted && key.name() == name)
            })
        })
    }

    fn inline_item(node: Item) -> Result<Item, ValueError> {
        match node {
            Item::Table(table) => {
                let value = unwrap_body(&table.body)?;
                let entries = table
                    .body
                    .body
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .key
                            .as_ref()
                            .map(|key| (key.clone(), entry.item.clone()))
                    })
                    .collect::<Vec<_>>();
                Ok(Item::InlineTable {
                    raw: render_inline_value(&value)?,
                    value,
                    trivia: Trivia::default(),
                    entries,
                })
            }
            other => Ok(other),
        }
    }

    fn generated(
        &self,
        value: &Value,
        syntax: Option<&[u8]>,
        element: bool,
    ) -> Result<(Item, bool), ValueError> {
        let (node, fragmented) = if let Some(syntax) = syntax {
            let text =
                std::str::from_utf8(syntax).map_err(|_| ValueError::new("syntax must be UTF-8"))?;
            let root = parse_document(text)?;
            let parts: Vec<Item> = root
                .body
                .iter()
                .filter(|entry| entry.key.as_ref().is_some_and(|key| key.name() == "value"))
                .map(|entry| entry.item.clone())
                .collect();
            match parts.len() {
                0 => return Err(ValueError::new("syntax fragment requires a value key")),
                1 => {
                    let Some(part) = parts.into_iter().next() else {
                        return Err(ValueError::new("syntax fragment requires a value key"));
                    };
                    (part, false)
                }
                _ => {
                    let mut body = Container::default();
                    for part in parts {
                        body.raw_append(Some(single_key("value", false)), part);
                    }
                    (
                        Item::Table(Table {
                            body,
                            ..Table::default()
                        }),
                        true,
                    )
                }
            }
        } else {
            let text = generated_fragment(value)?;
            let text = text.replace('\n', &self.newline);
            let root = parse_document(&text)?;
            let item = root
                .body
                .iter()
                .find(|entry| entry.key.as_ref().is_some_and(|key| key.name() == "value"))
                .map(|entry| entry.item.clone())
                .ok_or_else(|| ValueError::new("syntax fragment requires a value key"))?;
            (item, false)
        };
        let mut node = node;
        if element
            && let Item::Aot(members) = &node
            && members.len() == 1
            && let Some(member) = members.first()
        {
            node = Item::Table(member.clone());
        }
        Self::invalidate(&mut node);
        Ok((node, fragmented))
    }
}

fn cross_parents() -> ValueError {
    ValueError::new("TOML fragments cross separate parent table representations")
}

fn container_contains(container: &Container, last: &PathPart) -> bool {
    match last {
        PathPart::Key(name) => container
            .body
            .iter()
            .any(|entry| entry.key.as_ref().is_some_and(|key| key.name() == *name)),
        PathPart::Index(index) => container
            .body
            .iter()
            .any(|entry| matches!(&entry.item, Item::Aot(members) if members.len() > *index)),
    }
}

#[derive(Clone, Copy)]
enum TraceStep {
    Entry(usize),
    Member(usize),
}

fn trace_item(
    item: &Item,
    walk: &[PathPart],
    last: &PathPart,
) -> Result<Option<Vec<TraceStep>>, ValueError> {
    match item {
        Item::Table(table) => trace_container(&table.body, walk, last),
        Item::Aot(members) => match walk.split_first() {
            Some((PathPart::Index(index), rest)) => {
                let Some(member) = members.get(*index) else {
                    return Ok(None);
                };
                Ok(trace_container(&member.body, rest, last)?.map(|mut trace| {
                    trace.insert(0, TraceStep::Member(*index));
                    trace
                }))
            }
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

fn trace_container(
    container: &Container,
    walk: &[PathPart],
    last: &PathPart,
) -> Result<Option<Vec<TraceStep>>, ValueError> {
    match walk.split_first() {
        None => Ok(container_contains(container, last).then(Vec::new)),
        Some((PathPart::Key(name), rest)) => {
            let mut hits: Vec<Vec<TraceStep>> = Vec::new();
            for (index, entry) in container.body.iter().enumerate() {
                if entry.key.as_ref().is_none_or(|key| key.name() != *name) {
                    continue;
                }
                if let Some(mut trace) = trace_item(&entry.item, rest, last)? {
                    trace.insert(0, TraceStep::Entry(index));
                    hits.push(trace);
                }
            }
            if hits.len() > 1 {
                return Err(cross_parents());
            }
            Ok(hits.pop())
        }
        Some((PathPart::Index(_), _)) => Ok(None),
    }
}

fn replay_trace<'a>(
    root: &'a mut Container,
    trace: &[TraceStep],
) -> Result<&'a mut Container, ValueError> {
    let mut container: Option<&'a mut Container> = Some(root);
    let mut aot: Option<&'a mut Vec<Table>> = None;
    for step in trace {
        match step {
            TraceStep::Entry(index) => {
                let current = container.take().ok_or_else(cross_parents)?;
                let entry = current.body.get_mut(*index).ok_or_else(cross_parents)?;
                match &mut entry.item {
                    Item::Table(table) => container = Some(&mut table.body),
                    Item::Aot(members) => aot = Some(members),
                    _ => return Err(cross_parents()),
                }
            }
            TraceStep::Member(index) => {
                let members = aot.take().ok_or_else(cross_parents)?;
                let member = members.get_mut(*index).ok_or_else(cross_parents)?;
                container = Some(&mut member.body);
            }
        }
    }
    container.ok_or_else(cross_parents)
}

fn physical_parent_mut<'a>(
    root: &'a mut Container,
    path: &[PathPart],
) -> Result<&'a mut Container, ValueError> {
    let Some((last, walk)) = path.split_last() else {
        return Ok(root);
    };
    if matches!(last, PathPart::Index(_)) {
        return physical_parent_mut(root, walk);
    }
    let trace = trace_container(root, walk, last)?.ok_or_else(cross_parents)?;
    replay_trace(root, &trace)
}

fn body_container_mut<'a>(
    root: &'a mut Container,
    path: &[PathPart],
) -> Result<&'a mut Container, ValueError> {
    let mut container: &mut Container = root;
    for part in path {
        match part {
            PathPart::Key(name) => {
                let positions: Vec<usize> = container
                    .body
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.key.as_ref().is_some_and(|key| key.name() == *name))
                    .map(|(index, _)| index)
                    .collect();
                if positions.len() != 1 {
                    return Err(cross_parents());
                }
                let Some(position) = positions.first() else {
                    return Err(cross_parents());
                };
                let Some(entry) = container.body.get_mut(*position) else {
                    return Err(cross_parents());
                };
                match &mut entry.item {
                    Item::Table(table) => container = &mut table.body,
                    _ => return Err(cross_parents()),
                }
            }
            PathPart::Index(index) => {
                let member = container
                    .body
                    .iter_mut()
                    .find_map(|entry| match &mut entry.item {
                        Item::Aot(members) => members.get_mut(*index),
                        _ => None,
                    })
                    .ok_or_else(cross_parents)?;
                container = &mut member.body;
            }
        }
    }
    Ok(container)
}

// ------------------------------------------------------------ backend trait

impl DocumentBackend for TomlDocument {
    fn text(&self) -> &str {
        &self.text
    }

    fn native(&self, path: &[PathPart]) -> Result<Value, ValueError> {
        native_at(&self.root, path)
    }

    fn syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        syntax_fragment(self, path, true)
    }

    fn owned_syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        let located = locate(&self.root, path)?;
        if matches!(located, Located::Tables(_)) {
            self.contiguous(path)?;
        }
        let mut node = copy_located(&located);
        Self::strip_gap(&mut node);
        match located {
            Located::Tables(_) => {
                let tables = Self::tables_of(located);
                let mut body = Container::default();
                let count = tables.len();
                for (index, table) in tables.into_iter().enumerate() {
                    let mut copy = table.clone();
                    if index + 1 == count {
                        Self::strip_table_gap(&mut copy);
                    }
                    copy.trivia.indent = String::new();
                    copy.trivia.comment_ws = String::new();
                    copy.trivia.comment = String::new();
                    copy.trivia.trail.clone_from(&self.newline);
                    copy.display_name = None;
                    body.raw_append(Some(single_key("value", false)), Item::Table(copy));
                }
                Ok(render_root(&body).into_bytes())
            }
            Located::Item(_) => {
                let raw = node.as_string();
                Ok(raw.into_bytes())
            }
            Located::Table(_) => {
                let Item::Table(table) = &node else {
                    return Err(ValueError::new(
                        "a noncontiguous TOML table cannot be an owned unit",
                    ));
                };
                Ok(render_table_body(table).into_bytes())
            }
            Located::Aot(_) => {
                let mut body = Container::default();
                body.raw_append(Some(single_key("value", false)), node);
                Ok(render_root(&body).into_bytes())
            }
        }
    }

    fn layout(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        let located = locate(&self.root, path)?;
        let mut headers: Vec<(LayoutPath, String)> = Vec::new();
        collect_headers(located, &mut Vec::new(), &mut headers);
        if headers.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = headers
            .into_iter()
            .map(|(relative, name)| {
                let parts: Vec<Json> = relative
                    .iter()
                    .map(|part| match part {
                        LayoutPart::Field(field) => Json::Str(field.clone()),
                        LayoutPart::Index(index) => Json::Int((*index).cast_signed()),
                    })
                    .collect();
                Json::Array(vec![Json::Array(parts), Json::Str(name)])
            })
            .collect::<Vec<Json>>();
        let record = Json::Object(vec![
            (String::from("version"), Json::Int(1)),
            (String::from("headers"), Json::Array(encoded)),
        ]);
        let mut out = String::new();
        record.encode(&mut out);
        Ok(out.into_bytes())
    }

    fn position_prefix(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        let Some((last, parent)) = path.split_last() else {
            return Ok(Vec::new());
        };
        let PathPart::Index(index) = last else {
            return Ok(Vec::new());
        };
        let Ok(located) = locate(&self.root, parent) else {
            return Ok(Vec::new());
        };
        let Located::Item(Item::Array { .. }) = &located else {
            return Ok(Vec::new());
        };
        let (raw, spans, commas) = match &located {
            Located::Item(item) => array_parts(item)?,
            _ => return Ok(Vec::new()),
        };
        let start = if *index == 0 {
            1
        } else {
            commas.get(index - 1).copied().unwrap_or(0) + 1
        };
        let Some((span_start, _)) = spans.get(*index) else {
            return Err(ValueError::new("document selector path is missing"));
        };
        Ok(raw
            .get(start..*span_start)
            .unwrap_or_default()
            .as_bytes()
            .to_vec())
    }

    fn empty_syntax(&self, path: &[PathPart]) -> Result<Vec<u8>, ValueError> {
        let located = locate(&self.root, path)?;
        if let Located::Tables(_) = located {
            return Err(ValueError::new(
                "a noncontiguous TOML table cannot be pruned",
            ));
        }
        let mut raw = match &located {
            Located::Item(item) => item.as_string(),
            Located::Table(table) => render_table_body(table),
            Located::Aot(members) | Located::Tables(members) => members
                .iter()
                .map(|table| render_table_body(table))
                .collect::<String>(),
        };
        if let Located::Table(table) = &located {
            raw.push_str(&table.trivia.comment_ws);
            raw.push_str(&table.trivia.comment);
        }
        Ok(raw.into_bytes())
    }

    fn put(
        &mut self,
        path: &[PathPart],
        value: &Value,
        syntax: Option<&[u8]>,
        layout: &[u8],
    ) -> Result<(), ValueError> {
        self.put_path(path, value, syntax)?;
        self.restore_layout(path, layout)?;
        self.publish()
    }

    fn insert(
        &mut self,
        path: &[PathPart],
        index: usize,
        value: &Value,
        syntax: Option<&[u8]>,
        prefix: Option<&[u8]>,
        layout: &[u8],
    ) -> Result<(), ValueError> {
        let (node, fragmented) = self.generated(value, syntax, true)?;
        if fragmented {
            return Err(ValueError::new(
                "TOML table fragments cannot become an array member",
            ));
        }
        let parent_kind = {
            let located = locate(&self.root, path)?;
            match &located {
                Located::Item(Item::Array { .. }) => 0,
                Located::Item(Item::Aot(_)) | Located::Aot(_) => 1,
                _ => 2,
            }
        };
        match parent_kind {
            0 => {
                let inline = Self::inline_item(node)?;
                let rendered = inline.as_string();
                self.array_edit(path, index, Some(&rendered), false, prefix)?;
            }
            1 => {
                let Item::Table(table) = node else {
                    return Err(ValueError::new("TOML member selector requires an array"));
                };
                let length = {
                    let located = locate(&self.root, path)?;
                    let Some(members) = aot_members(&located) else {
                        return Err(ValueError::new("TOML member selector requires an array"));
                    };
                    members.len()
                };
                let insert_at = index.min(length);
                let container = aot_container_mut(&mut self.root, path)?;
                container.insert(insert_at, table);
                let mut full = path.to_vec();
                full.push(PathPart::Index(insert_at));
                self.restore_layout(&full, layout)?;
            }
            _ => return Err(ValueError::new("TOML member selector requires an array")),
        }
        self.publish()
    }

    #[allow(clippy::too_many_lines)]
    fn delete(&mut self, path: &[PathPart]) -> Result<(), ValueError> {
        let Some((last, parent)) = path.split_last() else {
            return Err(ValueError::new("TOML selector crosses a scalar"));
        };
        if let PathPart::Key(name) = last
            && matches!(
                locate(&self.root, parent),
                Ok(Located::Item(Item::InlineTable { .. }))
            )
        {
            self.inline_field_edit(parent, name, None)?;
            return self.publish();
        }
        match last {
            PathPart::Index(index) => {
                let parent_kind = {
                    let located = locate(&self.root, parent)?;
                    match &located {
                        Located::Item(Item::Array { .. }) => 0,
                        Located::Item(Item::Aot(_)) | Located::Aot(_) => 1,
                        _ => 2,
                    }
                };
                match parent_kind {
                    0 => {
                        let position = *index;
                        self.array_edit(parent, position, None, false, None)?;
                    }
                    1 => {
                        let position = *index;
                        let comment = {
                            let located = locate(&self.root, parent)?;
                            let Some(members) = aot_members(&located) else {
                                return Err(ValueError::new("TOML selector crosses a scalar"));
                            };
                            let table = members
                                .get(position)
                                .map(|table| (*table).clone())
                                .ok_or_else(|| {
                                    ValueError::new("document selector path is missing")
                                })?;
                            let mut copy = table;
                            let mut comment = header_comment(&copy);
                            comment.push_str(&Self::strip_table_gap(&mut copy));
                            comment
                        };
                        let aot = aot_container_mut(&mut self.root, parent)?;
                        if aot.is_empty() {
                            return Err(ValueError::new("document selector path is missing"));
                        }
                        let position = position.min(aot.len() - 1);
                        aot.remove(position);
                        if !comment.is_empty() {
                            if let Some(member) = aot.get_mut(position) {
                                member.trivia.indent =
                                    format!("{}{}", comment, member.trivia.indent);
                            } else if let Some(member) = aot.last_mut() {
                                member
                                    .body
                                    .raw_append(None, Item::Whitespace(comment.clone()));
                            }
                        }
                        if aot.is_empty() {
                            self.remove_empty_aot(parent, &comment)?;
                        }
                    }
                    _ => return Err(ValueError::new("TOML selector crosses a scalar")),
                }
            }
            PathPart::Key(name) => {
                let name = name.clone();
                let (target_proxy, parent_proxy) = {
                    let target = locate(&self.root, path).ok();
                    let parent = locate(&self.root, parent).ok();
                    (
                        matches!(target, Some(Located::Tables(_))),
                        matches!(parent, Some(Located::Tables(_))),
                    )
                };
                if target_proxy {
                    self.contiguous(path)?;
                    let (total, first) = {
                        let located = locate(&self.root, path)?;
                        let tables = Self::tables_of(located);
                        let header = tables
                            .iter()
                            .find(|table| !table.implicit)
                            .or_else(|| tables.first())
                            .copied()
                            .ok_or_else(|| ValueError::new("document selector path is missing"))?;
                        let mut header = header.clone();
                        let gap = Self::strip_table_gap(&mut header);
                        let mut total = if header.trivia.comment.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "{}{}{}{}",
                                header.trivia.indent,
                                header.trivia.comment_ws,
                                header.trivia.comment,
                                header.trivia.trail
                            )
                        };
                        total.push_str(&gap);
                        let container = physical_parent_mut(&mut self.root, path)?;
                        let first = container
                            .body
                            .iter()
                            .position(|entry| {
                                entry.key.as_ref().is_some_and(|key| key.name() == name)
                            })
                            .ok_or_else(|| ValueError::new("document selector path is missing"))?;
                        (total, first)
                    };
                    let container = physical_parent_mut(&mut self.root, path)?;
                    let positions: Vec<usize> = container
                        .body
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| {
                            entry.key.as_ref().is_some_and(|key| key.name() == name)
                        })
                        .map(|(index, _)| index)
                        .collect();
                    for position in positions.iter().rev() {
                        if *position < container.body.len() {
                            container.body.remove(*position);
                        }
                    }
                    if !total.is_empty() && first <= container.body.len() {
                        container.body.insert(
                            first,
                            Entry {
                                key: None,
                                item: Item::Whitespace(total),
                            },
                        );
                    }
                } else {
                    let (total, node_kind) = {
                        let located = locate(&self.root, path)?;
                        match &located {
                            Located::Item(item) => {
                                let mut copy = (*item).clone();
                                let gap = Self::strip_gap(&mut copy);
                                let mut total = match &copy {
                                    Item::Value { trivia, .. }
                                    | Item::Array { trivia, .. }
                                    | Item::InlineTable { trivia, .. }
                                        if !trivia.comment.is_empty() =>
                                    {
                                        format!(
                                            "{}{}{}{}",
                                            trivia.indent,
                                            trivia.comment_ws,
                                            trivia.comment,
                                            trivia.trail
                                        )
                                    }
                                    _ => String::new(),
                                };
                                total.push_str(&gap);
                                (total, copy)
                            }
                            Located::Table(table) => {
                                let mut copy = Item::Table((*table).clone());
                                let gap = Self::strip_gap(&mut copy);
                                let mut total = match &copy {
                                    Item::Table(table) if !table.trivia.comment.is_empty() => {
                                        format!(
                                            "{}{}{}{}",
                                            table.trivia.indent,
                                            table.trivia.comment_ws,
                                            table.trivia.comment,
                                            table.trivia.trail
                                        )
                                    }
                                    _ => String::new(),
                                };
                                total.push_str(&gap);
                                (total, copy)
                            }
                            _ => return Err(ValueError::new("document selector path is missing")),
                        }
                    };
                    if parent_proxy && !total.is_empty() {
                        return Err(ValueError::new("cannot safely detach this TOML comment"));
                    }
                    if parent_proxy {
                        let container = physical_parent_mut(&mut self.root, parent)?;
                        let part_positions: Vec<usize> = container
                            .body
                            .iter()
                            .enumerate()
                            .filter(|(_, entry)| {
                                entry
                                    .key
                                    .as_ref()
                                    .is_some_and(|key| key.name() == parent_name(parent))
                            })
                            .map(|(index, _)| index)
                            .collect();
                        let mut removed_parts: Vec<usize> = Vec::new();
                        for position in &part_positions {
                            if let Some(Item::Table(part)) = container
                                .body
                                .get_mut(*position)
                                .map(|entry| &mut entry.item)
                            {
                                let member_positions: Vec<usize> = part
                                    .body
                                    .body
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, entry)| {
                                        entry.key.as_ref().is_some_and(|key| key.name() == name)
                                    })
                                    .map(|(index, _)| index)
                                    .collect();
                                for member_position in member_positions.iter().rev() {
                                    part.body.body.remove(*member_position);
                                }
                                if part.body.body.is_empty() && part_positions.len() > 1 {
                                    removed_parts.push(*position);
                                }
                            }
                        }
                        for position in removed_parts.iter().rev() {
                            if *position < container.body.len() {
                                container.body.remove(*position);
                            }
                        }
                    } else {
                        let body = body_container_mut(&mut self.root, parent)?;
                        let mut position = body
                            .body
                            .iter()
                            .position(|entry| {
                                entry.key.as_ref().is_some_and(|key| key.name() == name)
                            })
                            .ok_or_else(|| ValueError::new("document selector path is missing"))?;
                        if matches!(
                            &node_kind,
                            Item::Table(table) if table.trivia.comment.is_empty()
                        ) && position > 0
                            && body.body.get(position - 1).is_some_and(|entry| {
                                entry.key.is_none() && matches!(entry.item, Item::Whitespace(_))
                            })
                        {
                            body.body.remove(position - 1);
                            position -= 1;
                        }
                        body.body.remove(position);
                        if !total.is_empty() && position <= body.body.len() {
                            body.body.insert(
                                position,
                                Entry {
                                    key: None,
                                    item: Item::Whitespace(total),
                                },
                            );
                        }
                    }
                }
                let newline = self.newline.clone();
                Self::retain_empty_tables(&mut self.root, &newline);
            }
        }
        self.publish()
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
        let Some((last, parent_path)) = path.split_last() else {
            return Err(ValueError::new("TOML selector crosses a scalar"));
        };
        let aot_member = matches!(last, PathPart::Index(_)) && {
            let parent = locate(&self.root, parent_path)?;
            matches!(parent, Located::Item(Item::Aot(_)) | Located::Aot(_))
        };
        if aot_member {
            let PathPart::Index(position) = last else {
                return Err(ValueError::new("TOML selector crosses a scalar"));
            };
            let insert_at = {
                let container = aot_container_mut(&mut self.root, parent_path)?;
                let table = container.remove(*position);
                let insert_at = index.min(container.len());
                container.insert(insert_at, table);
                insert_at
            };
            let mut target = parent_path.to_vec();
            target.push(PathPart::Index(insert_at));
            self.put_path(&target, value, syntax)?;
            self.restore_layout(&target, layout)?;
        } else {
            self.delete(path)?;
            self.insert(parent_path, index, value, syntax, prefix, layout)?;
        }
        self.publish()
    }
}

fn collect_headers(
    located: Located<'_>,
    relative: &mut LayoutPath,
    headers: &mut Vec<(LayoutPath, String)>,
) {
    match located {
        Located::Table(table) | Located::Item(Item::Table(table)) => {
            walk_layout(&[table], relative, headers);
        }
        Located::Tables(tables) => {
            walk_layout(&tables, relative, headers);
        }
        Located::Aot(members) => {
            for (index, member) in members.iter().enumerate() {
                relative.push(LayoutPart::Index(index as u64));
                collect_headers(Located::Table(member), relative, headers);
                relative.pop();
            }
        }
        Located::Item(Item::Aot(members)) => {
            for (index, member) in members.iter().enumerate() {
                relative.push(LayoutPart::Index(index as u64));
                collect_headers(Located::Table(member), relative, headers);
                relative.pop();
            }
        }
        Located::Item(_) => {}
    }
}

fn walk_layout(
    tables: &[&Table],
    relative: &mut LayoutPath,
    headers: &mut Vec<(LayoutPath, String)>,
) {
    for table in tables {
        if !relative.is_empty()
            && !table.implicit
            && let Some(name) = &table.display_name
        {
            headers.push((relative.clone(), name.clone()));
        }
        for entry in &table.body.body {
            if let Some(key) = &entry.key {
                relative.push(LayoutPart::Field(key.dotted_string()));
                collect_headers(Located::Item(&entry.item), relative, headers);
                relative.pop();
            }
        }
    }
}

fn header_comment(table: &Table) -> String {
    let trivia = &table.trivia;
    if trivia.comment.is_empty() {
        return trivia.indent.clone();
    }
    format!(
        "{}{}{}{}",
        trivia.indent, trivia.comment_ws, trivia.comment, trivia.trail
    )
}

fn aot_container_mut<'a>(
    root: &'a mut Container,
    path: &[PathPart],
) -> Result<&'a mut Vec<Table>, ValueError> {
    let container = physical_parent_mut(root, path)?;
    container
        .body
        .iter_mut()
        .filter(|entry| path.last().is_some_and(|part| matches!(part, PathPart::Key(name) if entry.key.as_ref().is_some_and(|key| key.name() == name))))
        .find_map(|entry| match &mut entry.item {
            Item::Aot(members) => Some(members),
            _ => None,
        })
        .ok_or_else(|| ValueError::new("document selector path is missing"))
}

impl TomlDocument {
    fn retain_empty_tables(container: &mut Container, newline: &str) {
        for entry in &mut container.body {
            if let Item::Table(table) = &mut entry.item {
                if table.body.body.is_empty() && table.implicit {
                    table.implicit = false;
                    table.trivia = Trivia {
                        trail: newline.to_owned(),
                        ..Trivia::default()
                    };
                }
                Self::retain_empty_tables(&mut table.body, newline);
            } else if let Item::Aot(members) = &mut entry.item {
                for member in members.iter_mut() {
                    Self::retain_empty_tables(&mut member.body, newline);
                }
            }
        }
    }

    fn remove_empty_aot(&mut self, path: &[PathPart], comment: &str) -> Result<(), ValueError> {
        let container = physical_parent_mut(&mut self.root, path)?;
        let Some(PathPart::Key(name)) = path.last() else {
            return Err(ValueError::new(
                "cannot safely detach this TOML array header comment",
            ));
        };
        let position = container
            .body
            .iter()
            .position(|entry| entry.key.as_ref().is_some_and(|key| key.name() == *name))
            .ok_or_else(|| ValueError::new("document selector path is missing"))?;
        container.body.remove(position);
        if !comment.is_empty() {
            if position < container.body.len() {
                container.body.insert(
                    position,
                    Entry {
                        key: None,
                        item: Item::Whitespace(comment.to_owned()),
                    },
                );
            } else {
                container.body.push(Entry {
                    key: None,
                    item: Item::Whitespace(comment.to_owned()),
                });
            }
        }
        Ok(())
    }

    fn array_edit(
        &mut self,
        path: &[PathPart],
        index: usize,
        raw: Option<&str>,
        replace: bool,
        prefix: Option<&[u8]>,
    ) -> Result<(), ValueError> {
        let located = locate(&self.root, path)?;
        let Located::Item(item) = &located else {
            return Err(ValueError::new("inline array edit requires an array"));
        };
        let (text, spans, commas) = array_parts(item)?;
        let span = |index: usize| {
            spans
                .get(index)
                .copied()
                .ok_or_else(|| ValueError::new("inline array edit requires an array member"))
        };
        let mut patches: Vec<(usize, usize, String)> = Vec::new();
        match raw {
            None => {
                let (start, end) = span(index)?;
                patches.push((start, end, String::new()));
                if !commas.is_empty() {
                    let comma = commas
                        .get(index.min(commas.len() - 1))
                        .copied()
                        .unwrap_or_default();
                    patches.push((comma, comma + 1, String::new()));
                }
            }
            Some(raw) if replace => {
                let (start, end) = span(index)?;
                patches.push((start, end, raw.to_owned()));
            }
            Some(raw) if index < spans.len() => {
                let start = if index == 0 {
                    1
                } else {
                    commas.get(index - 1).copied().unwrap_or(0) + 1
                };
                let (entry_start, _) = span(index)?;
                let start = prefixed(&text, start, entry_start, prefix)?;
                patches.push((start, start, format!("{raw},")));
            }
            Some(raw) => {
                let mut start = text.len().saturating_sub(1);
                if prefix.is_some() {
                    let boundary = if !commas.is_empty() && commas.len() == spans.len() {
                        commas.last().copied().unwrap_or_default() + 1
                    } else if !spans.is_empty() {
                        spans.last().map_or(1, |(_, end)| *end)
                    } else {
                        1
                    };
                    start = prefixed(&text, boundary, start, prefix)?;
                }
                patches.push((start, start, raw.to_owned()));
                if !spans.is_empty() && commas.len() < spans.len() {
                    let end = spans.last().map_or(start, |(_, end)| *end);
                    patches.push((end, end, String::from(",")));
                } else if !commas.is_empty()
                    && let Some(first) = patches.first_mut()
                {
                    first.2 = format!("{raw},");
                }
            }
        }
        patches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        let mut text = text;
        for (start, end, replacement) in &patches {
            let start = (*start).min(text.len());
            let end = (*end).min(text.len()).max(start);
            text.replace_range(start..end, replacement);
        }
        let fragment = format!("value = {text}");
        self.put_path(
            path,
            &Value::Scalar(Scalar::null()),
            Some(fragment.as_bytes()),
        )?;
        Ok(())
    }

    fn inline_field_edit(
        &mut self,
        path: &[PathPart],
        name: &str,
        value: Option<&str>,
    ) -> Result<(), ValueError> {
        let Located::Item(Item::InlineTable { raw, .. }) = locate(&self.root, path)? else {
            return Err(ValueError::new("TOML selector crosses a scalar"));
        };
        let mut scanner = Scanner::new(raw);
        scanner.step();
        let mut fields = Vec::new();
        let mut commas = Vec::new();
        loop {
            scanner.advance_spaces();
            if scanner.cur() == Some(b'}') {
                break;
            }
            let start = scanner.pos;
            let (key, _) = parse_key_value(&mut scanner, false, 0)?;
            let value_start = start + key.as_string().len() + key.sep.len();
            fields.push((key.name().to_owned(), start, value_start, scanner.pos));
            scanner.advance_spaces();
            if scanner.cur() == Some(b',') {
                commas.push(scanner.pos);
                scanner.step();
            } else {
                break;
            }
        }
        let index = fields.iter().position(|(key, _, _, _)| key == name);
        let mut patches = Vec::new();
        match (index, value) {
            (Some(index), Some(value)) => {
                let (_, _, start, end) = fields.get(index).ok_or_else(cross_parents)?;
                patches.push((*start, *end, value.to_owned()));
            }
            (Some(index), None) => {
                let (_, start, _, end) = fields.get(index).ok_or_else(cross_parents)?;
                patches.push((*start, *end, String::new()));
                if let Some(comma) = commas.get(index.min(commas.len().saturating_sub(1))) {
                    patches.push((*comma, *comma + 1, String::new()));
                }
            }
            (None, Some(value)) => {
                patches.push((
                    raw.len() - 1,
                    raw.len() - 1,
                    format!("{} = {value}", render_toml_key(name)),
                ));
                if let Some((_, _, _, end)) = fields.last() {
                    patches.push((*end, *end, String::from(",")));
                }
            }
            (None, None) => return Err(ValueError::new("document selector path is missing")),
        }
        let mut candidate = raw.clone();
        patches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
        for (start, end, replacement) in patches {
            candidate.replace_range(start..end, &replacement);
        }
        let syntax = format!("value = {candidate}");
        self.put_path(
            path,
            &Value::Scalar(Scalar::null()),
            Some(syntax.as_bytes()),
        )
    }

    #[allow(clippy::too_many_lines)]
    fn put_path(
        &mut self,
        path: &[PathPart],
        value: &Value,
        syntax: Option<&[u8]>,
    ) -> Result<(), ValueError> {
        let Some((last, parent_path)) = path.split_last() else {
            return Err(ValueError::new("TOML selector crosses a scalar"));
        };
        let element = matches!(last, PathPart::Index(_));
        let (mut node, fragmented) = self.generated(value, syntax, element)?;
        let parent_kind = locate(&self.root, parent_path).ok();
        let current = locate(&self.root, path).ok();
        let current_proxy = matches!(current, Some(Located::Tables(_)));
        let node_proxy = fragmented;
        if current_proxy || node_proxy {
            let current_kind = match &current {
                Some(Located::Tables(_)) => 2_u8,
                Some(Located::Table(_)) => 1,
                Some(_) => 3,
                None => 0,
            };
            let tables: Vec<Table> = current
                .map(|located| Self::tables_of(located).into_iter().cloned().collect())
                .unwrap_or_default();
            return self.put_fragments(path, node, current_kind, fragmented, &tables);
        }
        match last {
            PathPart::Key(name) => {
                let parent_ok = parent_path.is_empty()
                    || matches!(
                        parent_kind,
                        Some(
                            Located::Table(_)
                                | Located::Tables(_)
                                | Located::Item(Item::Table(_) | Item::InlineTable { .. })
                        )
                    );
                if !parent_ok {
                    return Err(ValueError::new("TOML selector crosses a scalar"));
                }
                let exists = locate(&self.root, path).is_ok();
                if !exists && !self.text.is_empty() && !self.text.ends_with(['\r', '\n']) {
                    let repaired = format!("{}{}", self.text, self.newline);
                    let root = coalesce(parse_document(&repaired).map_err(|_| {
                        ValueError::new("TOML layout cannot preserve valid syntax")
                    })?);
                    self.root = root;
                    self.text = repaired;
                }
                if exists {
                    let current_item = match locate(&self.root, path)? {
                        Located::Item(item) => (*item).clone(),
                        Located::Table(table) => Item::Table((*table).clone()),
                        Located::Aot(members) => Item::Aot(members.into_iter().cloned().collect()),
                        Located::Tables(_) => {
                            return Err(ValueError::new(
                                "TOML table representation changes cannot preserve foreign layout",
                            ));
                        }
                    };
                    if is_table_item(&current_item)
                        && std::mem::discriminant(&current_item) != std::mem::discriminant(&node)
                    {
                        return Err(ValueError::new(
                            "TOML table representation changes cannot preserve foreign layout",
                        ));
                    }
                    if !is_table_item(&current_item) {
                        node = Self::inline_item(node)?;
                    } else if let Item::Table(table) = &mut node
                        && self.is_dotted_target(path)
                    {
                        table.implicit = true;
                        if syntax.is_none() {
                            let keys: Vec<Key> = table
                                .body
                                .body
                                .iter()
                                .filter(|entry| matches!(entry.item, Item::Table(_)))
                                .filter_map(|entry| entry.key.clone())
                                .collect();
                            for key in keys {
                                let position = table.body.body.iter().position(|entry| {
                                    entry.key.as_ref() == Some(&key)
                                        && matches!(entry.item, Item::Table(_))
                                });
                                let Some(position) = position else {
                                    continue;
                                };
                                let Some(entry) = table.body.body.get_mut(position) else {
                                    continue;
                                };
                                let child = std::mem::replace(&mut entry.item, Item::Null);
                                let inlined = Self::inline_item(child.clone())?;
                                entry.item = child;
                                replace_entry_at(&mut table.body, position, inlined, Some(key))?;
                            }
                            let doubled = format!("{}{}", self.newline, self.newline);
                            for entry in &mut table.body.body {
                                if let Some(trivia) = any_trivia_mut(&mut entry.item)
                                    && trivia.trail.ends_with(&doubled)
                                {
                                    let length = trivia.trail.len() - self.newline.len();
                                    trivia.trail.truncate(length);
                                }
                            }
                            let mut item = Item::Table(std::mem::take(table));
                            Self::strip_gap(&mut item);
                            if let Item::Table(taken) = item {
                                *table = taken;
                            }
                        }
                    }
                    if let (Item::Table(node_table), Item::Table(current_table)) =
                        (&mut node, &current_item)
                        && current_table.implicit
                    {
                        node_table.implicit = true;
                        node_table.display_name = None;
                    }
                    if let (Item::Table(_), _) = (&node, &current_item) {
                        let live: Vec<Table> = Self::tables_of(Located::Item(&current_item))
                            .into_iter()
                            .cloned()
                            .collect();
                        Self::preserve_header(&mut node, &live);
                    }
                    let mut current_copy = current_item.clone();
                    let gap = Self::strip_gap(&mut current_copy);
                    Self::append_gap(&mut node, &gap);
                    let indent = current_item
                        .trivia()
                        .map(|trivia| trivia.indent.clone())
                        .unwrap_or_default();
                    if let Some(trivia) = node.trivia_mut() {
                        trivia.indent = indent;
                    }
                } else if self.is_dotted_target(parent_path) {
                    node = Self::inline_item(node)?;
                }
                self.assign(parent_path, name, node)?;
            }
            PathPart::Index(index) => match parent_kind {
                Some(Located::Item(Item::Array { .. })) => {
                    let inline = Self::inline_item(node)?;
                    let rendered = inline.as_string();
                    self.array_edit(parent_path, *index, Some(&rendered), true, None)?;
                }
                Some(Located::Item(Item::Aot(_)) | Located::Aot(_)) => {
                    let position = *index;
                    let old = {
                        let members = aot_container_mut(&mut self.root, parent_path)?;
                        members
                            .get(position)
                            .cloned()
                            .ok_or_else(|| ValueError::new("document selector path is missing"))?
                    };
                    let trivia = old.trivia.clone();
                    let mut old_item = Item::Table(old);
                    let gap = Self::strip_gap(&mut old_item);
                    Self::append_gap(&mut node, &gap);
                    let members = aot_container_mut(&mut self.root, parent_path)?;
                    if let Item::Table(table) = &mut node {
                        table.trivia = trivia;
                    }
                    if let Some(member) = members.get_mut(position)
                        && let Item::Table(table) = &mut node
                    {
                        let _ = std::mem::replace(member, std::mem::take(table));
                    }
                }
                _ => return Err(ValueError::new("TOML selector crosses a scalar")),
            },
        }
        Ok(())
    }

    fn assign(
        &mut self,
        parent_path: &[PathPart],
        name: &str,
        node: Item,
    ) -> Result<Option<usize>, ValueError> {
        if parent_path.is_empty() {
            return assign_in(&mut self.root, name, node).map(Some);
        }
        let located = locate(&self.root, parent_path)?;
        match &located {
            Located::Table(_) => {
                let mut target = parent_path.to_vec();
                target.push(PathPart::Key(name.to_owned()));
                let exists = locate(&self.root, &target).is_ok();
                let container = if exists {
                    physical_parent_mut(&mut self.root, &target)?
                } else {
                    body_container_mut(&mut self.root, parent_path)?
                };
                assign_in(container, name, node).map(Some)
            }
            Located::Tables(_) => {
                let mut target = parent_path.to_vec();
                target.push(PathPart::Key(name.to_owned()));
                if locate(&self.root, &target).is_ok() {
                    let container = physical_parent_mut(&mut self.root, &target)?;
                    assign_in(container, name, node).map(Some)
                } else {
                    self.assign_proxy(parent_path, name, node)?;
                    Ok(None)
                }
            }
            Located::Item(Item::InlineTable { .. }) => {
                if is_table_item(&node) {
                    return Err(ValueError::new("Inline tables cannot contain a table"));
                }
                self.inline_field_edit(parent_path, name, Some(&node.as_string()))?;
                Ok(None)
            }
            _ => Err(ValueError::new("TOML selector crosses a scalar")),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn assign_proxy(
        &mut self,
        parent_path: &[PathPart],
        name: &str,
        node: Item,
    ) -> Result<(), ValueError> {
        let part_name = parent_name(parent_path).to_owned();
        if part_name.is_empty() {
            return Err(ValueError::new("TOML selector crosses a scalar"));
        }
        let (part_positions, holders, has_plain, first_concrete) = {
            let container = physical_parent_mut(&mut self.root, parent_path)?;
            proxy_layout(container, &part_name, name)
        };
        if part_positions.is_empty() {
            return Err(ValueError::new("document selector path is missing"));
        }
        let key = single_key(name, false);
        if let Some(first_holder) = holders.first().copied() {
            let Some(target) = part_positions.get(first_holder).copied() else {
                return Err(ValueError::new("document selector path is missing"));
            };
            let removed: Vec<usize> = part_positions
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != first_holder)
                .map(|(_, position)| *position)
                .collect();
            let old = {
                let container = physical_parent_mut(&mut self.root, parent_path)?;
                let entry = container
                    .body
                    .get(target)
                    .map(|entry| &entry.item)
                    .ok_or_else(|| ValueError::new("document selector path is missing"))?;
                match entry {
                    Item::Table(table) => table
                        .body
                        .body
                        .iter()
                        .find(|entry| entry.key.as_ref().is_some_and(|key| key.name() == name)),
                    _ => None,
                }
                .map(|entry| entry.item.clone())
                .ok_or_else(|| ValueError::new("document selector path is missing"))?
            };
            let container = physical_parent_mut(&mut self.root, parent_path)?;
            let mut ordered = removed.clone();
            ordered.sort_unstable_by(|left, right| right.cmp(left));
            for position in ordered {
                if position < container.body.len() {
                    container.body.remove(position);
                }
            }
            let shifted = removed
                .iter()
                .filter(|position| **position < target)
                .count();
            let target = target.saturating_sub(shifted);
            let Some(Item::Table(part)) =
                container.body.get_mut(target).map(|entry| &mut entry.item)
            else {
                return Err(ValueError::new("document selector path is missing"));
            };
            let entry = part
                .body
                .body
                .iter()
                .position(|entry| entry.key.as_ref().is_some_and(|key| key.name() == name))
                .ok_or_else(|| ValueError::new("document selector path is missing"))?;
            if is_table_item(&old) && !is_table_item(&node) {
                let Some(entry) = part.body.body.get_mut(entry) else {
                    return Err(ValueError::new("document selector path is missing"));
                };
                entry.key = None;
                entry.item = Item::Null;
                let container = physical_parent_mut(&mut self.root, parent_path)?;
                return append_into_part(container, &part_name, name, node);
            }
            return replace_entry_at(&mut part.body, entry, node, None).map(|_| ());
        }
        let container = physical_parent_mut(&mut self.root, parent_path)?;
        if is_table_item(&node) {
            let position = part_positions.first().copied().unwrap_or_default();
            let first_indent = aot_first_indent(&node);
            let Some(part) = container
                .body
                .get_mut(position)
                .and_then(|entry| table_of_mut(&mut entry.item))
            else {
                return Err(ValueError::new("document selector path is missing"));
            };
            part.body.append(Some(key), node, false)?;
            if let Some(last) = part.body.body.last_mut() {
                set_first_indent(&mut last.item, first_indent);
            }
            return Ok(());
        }
        let chosen = has_plain.or(first_concrete).unwrap_or_default();
        let position = part_positions
            .get(chosen)
            .or_else(|| part_positions.first())
            .copied()
            .unwrap_or_default();
        let first_indent = aot_first_indent(&node);
        let Some(part) = container
            .body
            .get_mut(position)
            .and_then(|entry| table_of_mut(&mut entry.item))
        else {
            return Err(ValueError::new("document selector path is missing"));
        };
        part.body.append(Some(key), node, false)?;
        if let Some(last) = part.body.body.last_mut() {
            set_first_indent(&mut last.item, first_indent);
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn put_fragments(
        &mut self,
        path: &[PathPart],
        node: Item,
        current_kind: u8,
        fragmented: bool,
        current_tables: &[Table],
    ) -> Result<(), ValueError> {
        if current_kind == 0 {
            let container = physical_parent_mut(&mut self.root, path)?;
            let Some(PathPart::Key(name)) = path.last() else {
                return Err(ValueError::new("TOML selector crosses a scalar"));
            };
            let position = container
                .body
                .iter()
                .position(|entry| entry.key.as_ref().is_some_and(|key| key.name() == *name))
                .ok_or_else(|| ValueError::new("document selector path is missing"))?;
            let mut node = node;
            Self::strip_gap(&mut node);
            let entry = container
                .body
                .get_mut(position)
                .ok_or_else(|| ValueError::new("document selector path is missing"))?;
            entry.item = node;
            return Ok(());
        }
        if !matches!(current_kind, 1 | 2) {
            return Err(ValueError::new(
                "TOML table representation changes cannot preserve foreign layout",
            ));
        }
        if !matches!(node, Item::Table(_)) {
            return Err(ValueError::new(
                "TOML table representation changes cannot preserve foreign layout",
            ));
        }
        self.contiguous(path)?;
        let mut tables: Vec<Table> = match &node {
            Item::Table(table) if fragmented => table
                .body
                .body
                .iter()
                .filter_map(|entry| table_of(&entry.item).cloned())
                .collect(),
            Item::Table(table) => vec![table.clone()],
            _ => {
                return Err(ValueError::new(
                    "TOML table representation changes cannot preserve foreign layout",
                ));
            }
        };
        let mut node = node;
        let selected: Vec<String> = path
            .iter()
            .filter_map(|part| match part {
                PathPart::Key(name) => Some(name.clone()),
                PathPart::Index(_) => None,
            })
            .collect();
        let live = current_tables
            .iter()
            .filter(|table| {
                !table.implicit
                    && table
                        .display_name
                        .as_deref()
                        .and_then(|name| header_path(name).ok())
                        .is_some_and(|header| header == selected)
            })
            .cloned()
            .collect::<Vec<_>>();
        if fragmented {
            if let Some((trivia, display_name)) = live
                .first()
                .map(|table| (table.trivia.clone(), table.display_name.clone()))
            {
                let index = tables
                    .iter()
                    .rposition(|table| !table.implicit)
                    .unwrap_or_else(|| tables.len().saturating_sub(1));
                if let Some(header) = tables.get_mut(index) {
                    header.implicit = false;
                    header.trivia = trivia;
                    header.display_name = display_name;
                }
            }
        } else {
            Self::preserve_header(&mut node, &live);
        }
        let mut last = current_tables.last().cloned().unwrap_or_default();
        let gap = Self::strip_table_gap(&mut last);
        if fragmented {
            if let Some(table) = tables.last_mut() {
                table.body.raw_append(None, Item::Whitespace(gap));
            }
        } else {
            Self::append_gap(&mut node, &gap);
        }
        let Some(PathPart::Key(name)) = path.last() else {
            return Err(ValueError::new("TOML selector crosses a scalar"));
        };
        let container = physical_parent_mut(&mut self.root, path)?;
        let positions: Vec<usize> = container
            .body
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.key.as_ref().is_some_and(|key| key.name() == *name))
            .map(|(index, _)| index)
            .collect();
        if positions.is_empty() {
            return Err(ValueError::new("document selector path is missing"));
        }
        if !fragmented {
            let Item::Table(head) = node else {
                return Err(ValueError::new(
                    "TOML table representation changes cannot preserve foreign layout",
                ));
            };
            if let Some(first) = tables.first_mut() {
                *first = head;
            }
        }
        let first = positions.first().copied().unwrap_or_default();
        let replacement_key = container
            .body
            .get(first)
            .and_then(|entry| entry.key.clone())
            .ok_or_else(|| ValueError::new("document selector path is missing"))?;
        let replacement: Vec<Entry> = tables
            .into_iter()
            .map(|table| Entry {
                key: Some(replacement_key.clone()),
                item: Item::Table(table),
            })
            .collect();
        let last = positions.last().copied().unwrap_or(first);
        container.body.splice(first..=last, replacement);
        Ok(())
    }
}

fn assign_in(container: &mut Container, name: &str, node: Item) -> Result<usize, ValueError> {
    let key = single_key(name, false);
    let position = container
        .body
        .iter()
        .position(|entry| entry.key.as_ref().is_some_and(|key| key.name() == name));
    if let Some(index) = position {
        replace_entry_at(container, index, node, None)
    } else {
        let table_indent = match &node {
            Item::Table(table) => Some(table.trivia.indent.clone()),
            _ => None,
        };
        let first_indent = aot_first_indent(&node);
        let body_length = match &node {
            Item::Table(table) => table.body.body.len(),
            _ => 0,
        };
        container.append(Some(key), node, false)?;
        let landing = container.body.len() - 1;
        if let Some(last) = container.body.get_mut(landing) {
            if let Item::Table(table) = &mut last.item {
                if let Some(indent) = table_indent {
                    table.trivia.indent = indent;
                }
                while table.body.body.len() > body_length
                    && matches!(
                        table.body.body.last().map(|entry| &entry.item),
                        Some(Item::Whitespace(_))
                    )
                {
                    table.body.body.pop();
                }
            }
            set_first_indent(&mut last.item, first_indent);
        }
        Ok(landing)
    }
}

fn parent_name(path: &[PathPart]) -> &str {
    path.last()
        .and_then(|part| match part {
            PathPart::Key(name) => Some(name.as_str()),
            PathPart::Index(_) => None,
        })
        .unwrap_or_default()
}

fn aot_first_indent(node: &Item) -> Option<String> {
    match node {
        Item::Aot(members) => members.first().map(|member| member.trivia.indent.clone()),
        _ => None,
    }
}

fn set_first_indent(node: &mut Item, indent: Option<String>) {
    if let Item::Aot(members) = node
        && let Some(member) = members.first_mut()
        && let Some(indent) = indent
    {
        member.trivia.indent = indent;
    }
}

fn aot_members<'a>(located: &Located<'a>) -> Option<Vec<&'a Table>> {
    match located {
        Located::Item(Item::Aot(members)) => Some(members.iter().collect()),
        Located::Aot(members) | Located::Tables(members) => Some(members.clone()),
        _ => None,
    }
}

fn container_insert_at(
    container: &mut Container,
    index: usize,
    key: Option<Key>,
    item: Item,
) -> Result<(), ValueError> {
    if container.body.is_empty() || index > container.body.len() - 1 {
        return Err(ValueError::new(format!(
            "Unable to insert at position {index}"
        )));
    }
    container.body.insert(index, Entry { key, item });
    Ok(())
}

fn previous_item_index(container: &Container, before: Option<usize>) -> Option<usize> {
    let end = before
        .unwrap_or(container.body.len())
        .min(container.body.len());
    (0..end).rev().find(|index| {
        container
            .body
            .get(*index)
            .is_some_and(|entry| !matches!(entry.item, Item::Null))
    })
}

#[allow(clippy::too_many_lines)]
fn replace_entry_at(
    container: &mut Container,
    index: usize,
    node: Item,
    new_key: Option<Key>,
) -> Result<usize, ValueError> {
    if index >= container.body.len() {
        return Err(ValueError::new("document selector path is missing"));
    }
    let Some(old_key) = container
        .body
        .get(index)
        .and_then(|entry| entry.key.clone())
    else {
        return Err(ValueError::new("document selector path is missing"));
    };
    let Some(old_item) = container.body.get(index).map(|entry| entry.item.clone()) else {
        return Err(ValueError::new("document selector path is missing"));
    };
    let old_is_table = is_table_item(&old_item);
    let new_is_table = is_table_item(&node);
    let dotted_to_header = old_key.dotted
        && match &node {
            Item::Aot(_) => true,
            Item::Table(table) => !table.implicit,
            _ => false,
        };
    let trailing = container.body.get(index + 1..).unwrap_or_default();
    let reposition_dotted = dotted_to_header
        && trailing.iter().any(|entry| {
            !matches!(entry.item, Item::Null | Item::Whitespace(_))
                && (!is_table_item(&entry.item) || entry.key.as_ref().is_some_and(|key| key.dotted))
        });
    let new_key = match new_key {
        Some(key) => key,
        None if new_is_table != old_is_table || dotted_to_header => {
            single_key(old_key.name(), false)
        }
        None => old_key,
    };
    let body_length = match &node {
        Item::Table(table) => table.body.body.len(),
        _ => 0,
    };
    let landing = if new_is_table != old_is_table || reposition_dotted {
        let Some(entry) = container.body.get_mut(index) else {
            return Err(ValueError::new("document selector path is missing"));
        };
        entry.key = None;
        entry.item = Item::Null;
        let target = if new_is_table {
            let length = container.body.len();
            (index..length).find(|position| {
                container.body.get(*position).is_some_and(|entry| {
                    is_table_item(&entry.item) && !entry.key.as_ref().is_some_and(|key| key.dotted)
                })
            })
        } else {
            None
        };
        if let Some(position) = target {
            container_insert_at(container, position, Some(new_key), node)?;
            position
        } else {
            container.append(Some(new_key), node, false)?;
            container.body.len() - 1
        }
    } else {
        let mut assigned = node;
        if !matches!(assigned, Item::Whitespace(_) | Item::Aot(_))
            && let (Some(trivia), Some(live)) = (assigned.trivia_mut(), old_item.trivia())
        {
            trivia.indent.clone_from(&live.indent);
            if trivia.comment_ws.is_empty() {
                trivia.comment_ws.clone_from(&live.comment_ws);
            }
            if trivia.comment.is_empty() {
                trivia.comment.clone_from(&live.comment);
            }
            trivia.trail.clone_from(&live.trail);
        }
        let Some(entry) = container.body.get_mut(index) else {
            return Err(ValueError::new("document selector path is missing"));
        };
        entry.key = Some(new_key);
        entry.item = assigned;
        index
    };
    let landing_is_table = container
        .body
        .get(landing)
        .is_some_and(|entry| matches!(entry.item, Item::Table(_)));
    if landing_is_table {
        let last = previous_item_index(container, None).unwrap_or(landing);
        let replace_has_ws = matches!(&old_item, Item::Table(table) if table
            .body
            .body
            .last()
            .is_some_and(|entry| matches!(entry.item, Item::Whitespace(_))));
        let next_ws = landing < last
            && container
                .body
                .get(landing + 1)
                .is_some_and(|entry| matches!(entry.item, Item::Whitespace(_)));
        let ends_with_ws = container
            .body
            .get(landing)
            .is_some_and(|entry| item_ends_with_whitespace(&entry.item));
        if (landing < last || replace_has_ws)
            && !(next_ws || ends_with_ws)
            && let Some(Item::Table(table)) =
                container.body.get_mut(landing).map(|entry| &mut entry.item)
        {
            table
                .body
                .raw_append(None, Item::Whitespace(String::from("\n")));
        }
        if let Some(Item::Table(table)) =
            container.body.get_mut(landing).map(|entry| &mut entry.item)
        {
            while table.body.body.len() > body_length
                && matches!(
                    table.body.body.last().map(|entry| &entry.item),
                    Some(Item::Whitespace(_))
                )
            {
                table.body.body.pop();
            }
        }
    }
    Ok(landing)
}

fn proxy_layout(
    container: &Container,
    part_name: &str,
    name: &str,
) -> (Vec<usize>, Vec<usize>, Option<usize>, Option<usize>) {
    let part_positions: Vec<usize> = container
        .body
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry
                .key
                .as_ref()
                .is_some_and(|key| key.name() == part_name)
        })
        .map(|(index, _)| index)
        .collect();
    let mut holders: Vec<usize> = Vec::new();
    let mut has_plain: Option<usize> = None;
    let mut first_concrete: Option<usize> = None;
    for (part, position) in part_positions.iter().enumerate() {
        let Some(Item::Table(table)) = container.body.get(*position).map(|entry| &entry.item)
        else {
            continue;
        };
        if table
            .body
            .body
            .iter()
            .any(|entry| entry.key.as_ref().is_some_and(|key| key.name() == name))
        {
            holders.push(part);
        }
        if has_plain.is_none()
            && table
                .body
                .body
                .iter()
                .any(|entry| entry.key.is_some() && !is_table_item(&entry.item))
        {
            has_plain = Some(part);
        }
        if first_concrete.is_none() && !table.implicit {
            first_concrete = Some(part);
        }
    }
    (part_positions, holders, has_plain, first_concrete)
}

fn append_into_part(
    container: &mut Container,
    part_name: &str,
    name: &str,
    node: Item,
) -> Result<(), ValueError> {
    let positions: Vec<usize> = container
        .body
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry
                .key
                .as_ref()
                .is_some_and(|key| key.name() == part_name)
        })
        .map(|(index, _)| index)
        .collect();
    let mut chosen: Option<usize> = None;
    for position in &positions {
        if let Some(Item::Table(table)) = container.body.get(*position).map(|entry| &entry.item)
            && table
                .body
                .body
                .iter()
                .any(|entry| entry.key.is_some() && !is_table_item(&entry.item))
        {
            chosen = Some(*position);
            break;
        }
    }
    if chosen.is_none() {
        for position in &positions {
            if let Some(Item::Table(table)) = container.body.get(*position).map(|entry| &entry.item)
                && !table.implicit
            {
                chosen = Some(*position);
                break;
            }
        }
    }
    let Some(position) = chosen.or_else(|| positions.first().copied()) else {
        return Err(ValueError::new("document selector path is missing"));
    };
    let first_indent = aot_first_indent(&node);
    let Some(Item::Table(part)) = container
        .body
        .get_mut(position)
        .map(|entry| &mut entry.item)
    else {
        return Err(ValueError::new("document selector path is missing"));
    };
    part.body
        .append(Some(single_key(name, false)), node, false)?;
    if let Some(last) = part.body.body.last_mut() {
        set_first_indent(&mut last.item, first_indent);
    }
    Ok(())
}

fn prefixed(
    text: &str,
    start: usize,
    end: usize,
    prefix: Option<&[u8]>,
) -> Result<usize, ValueError> {
    let raw = prefix.unwrap_or_default();
    let raw = std::str::from_utf8(raw).unwrap_or("");
    if !text
        .get(start..end)
        .is_some_and(|slice| slice.starts_with(raw))
    {
        return Err(ValueError::new("original array position trivia changed"));
    }
    Ok(start + raw.len())
}

#[cfg(test)]
#[path = "toml_document_regression_tests.rs"]
mod regression_tests;
