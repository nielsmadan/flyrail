use crate::values::ValueError;
use std::fmt::Write as _;

const MAX_DEPTH: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    BigInt(String),
    Float(f64),
    Str(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            pos: 0,
        };
        let value = parser.value(0)?;
        parser.skip_whitespace();
        if parser.pos != parser.bytes.len() {
            return Err(ValueError::new("JSON has trailing content"));
        }
        Ok(value)
    }

    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(items) => items
                .iter()
                .find(|(existing, _)| existing == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn at(&self, index: usize) -> Option<&Self> {
        match self {
            Self::Array(items) => items.get(index),
            _ => None,
        }
    }

    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    #[allow(dead_code)]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub const fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub const fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Object(items) => Some(items),
            _ => None,
        }
    }

    pub fn encode(&self, out: &mut String) {
        self.encode_with(out, ",", ":", false);
    }

    pub(crate) fn encode_python(&self, out: &mut String) {
        self.encode_with(out, ", ", ": ", true);
    }

    pub(crate) fn encode_python_compact(&self, out: &mut String) {
        self.encode_with(out, ",", ":", true);
    }

    fn encode_with(
        &self,
        out: &mut String,
        list_separator: &str,
        pair_separator: &str,
        ensure_ascii: bool,
    ) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Int(value) => {
                let _ = write!(out, "{value}");
            }
            Self::BigInt(text) => out.push_str(text),
            Self::Float(value) => out.push_str(&python_float_repr(*value)),
            Self::Str(value) => write_escaped_with(value, out, ensure_ascii),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(list_separator);
                    }
                    item.encode_with(out, list_separator, pair_separator, ensure_ascii);
                }
                out.push(']');
            }
            Self::Object(items) => {
                out.push('{');
                for (index, (key, value)) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(list_separator);
                    }
                    write_escaped_with(key, out, ensure_ascii);
                    out.push_str(pair_separator);
                    value.encode_with(out, list_separator, pair_separator, ensure_ascii);
                }
                out.push('}');
            }
        }
    }
}

pub fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return String::from("NaN");
    }
    if value.is_infinite() {
        return String::from(if value < 0.0 { "-Infinity" } else { "Infinity" });
    }
    if value == 0.0 {
        return String::from(if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        });
    }
    let shortest = format!("{value:e}");
    let (mantissa, exponent) = shortest.split_once('e').unwrap_or((shortest.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or_default();
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let point = exponent + 1;
    let sign = if negative { "-" } else { "" };
    let length = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if point <= -4 || point > 16 {
        let lead = digits.get(0..1).unwrap_or("0");
        let rest = digits.get(1..).unwrap_or_default();
        let tail = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let scale = point - 1;
        let scale_sign = if scale < 0 { '-' } else { '+' };
        return format!("{sign}{lead}{tail}e{scale_sign}{:02}", scale.abs());
    }
    if point <= 0 {
        let zeros = "0".repeat(usize::try_from(point.unsigned_abs()).unwrap_or_default());
        return format!("{sign}0.{zeros}{digits}");
    }
    if point >= length {
        let zeros =
            "0".repeat(usize::try_from((point - length).unsigned_abs()).unwrap_or_default());
        return format!("{sign}{digits}{zeros}.0");
    }
    let split = usize::try_from(point.unsigned_abs()).unwrap_or_default();
    let whole = digits.get(0..split).unwrap_or(digits);
    let fraction = digits.get(split..).unwrap_or_default();
    format!("{sign}{whole}.{fraction}")
}

pub fn write_escaped(value: &str, out: &mut String) {
    write_escaped_with(value, out, false);
}

fn write_escaped_with(value: &str, out: &mut String, ensure_ascii: bool) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if u32::from(other) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(other));
            }
            other if ensure_ascii && !other.is_ascii() => {
                let scalar = u32::from(other);
                if scalar <= 0xffff {
                    let _ = write!(out, "\\u{scalar:04x}");
                } else {
                    let adjusted = scalar - 0x10000;
                    let high = 0xd800 + (adjusted >> 10);
                    let low = 0xdc00 + (adjusted & 0x3ff);
                    let _ = write!(out, "\\u{high:04x}\\u{low:04x}");
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ValueError> {
        if depth > MAX_DEPTH {
            return Err(ValueError::new("JSON nesting is too deep"));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'N') => {
                if self.bytes.get(self.pos..self.pos + 3) == Some(b"NaN") {
                    Err(ValueError::new("non-JSON numeric constant: NaN"))
                } else {
                    Err(ValueError::new("JSON contains an unexpected character"))
                }
            }
            Some(b'I') => {
                if self.bytes.get(self.pos..self.pos + 8) == Some(b"Infinity") {
                    Err(ValueError::new("non-JSON numeric constant: Infinity"))
                } else {
                    Err(ValueError::new("JSON contains an unexpected character"))
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(ValueError::new("JSON contains an unexpected character")),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, ValueError> {
        let end = self.pos + word.len();
        if self.bytes.get(self.pos..end) == Some(word.as_bytes()) {
            self.pos = end;
            Ok(value)
        } else {
            Err(ValueError::new("JSON contains an invalid literal"))
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, ValueError> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(ValueError::new("JSON array is missing a separator")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ValueError> {
        self.pos += 1;
        let mut items: Vec<(String, Json)> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(items));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(ValueError::new("JSON object keys must be strings"));
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(ValueError::new("JSON object is missing a colon"));
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.value(depth + 1)?;
            if items.iter().any(|(existing, _)| *existing == key) {
                return Err(ValueError::new(format!("duplicate JSON field: {key}")));
            }
            items.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(items));
                }
                _ => return Err(ValueError::new("JSON object is missing a separator")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ValueError> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| ValueError::new("JSON string is unterminated"))?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    self.escape(&mut out)?;
                }
                0x00..=0x1F => {
                    return Err(ValueError::new("JSON string contains a control character"));
                }
                _ => {
                    let length = utf8_length(byte)
                        .ok_or_else(|| ValueError::new("JSON string contains invalid UTF-8"))?;
                    let end = self.pos + length;
                    let slice = self
                        .bytes
                        .get(self.pos..end)
                        .and_then(|slice| std::str::from_utf8(slice).ok())
                        .ok_or_else(|| ValueError::new("JSON string contains invalid UTF-8"))?;
                    out.push_str(slice);
                    self.pos = end;
                }
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), ValueError> {
        let byte = self
            .peek()
            .ok_or_else(|| ValueError::new("JSON escape is unterminated"))?;
        self.pos += 1;
        match byte {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{0008}'),
            b'f' => out.push('\u{000C}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let first = self.hex4()?;
                let character = if (0xD800..=0xDBFF).contains(&first) {
                    if self.bytes.get(self.pos..self.pos + 2) != Some(b"\\u") {
                        return Err(ValueError::new(
                            "JSON escape contains an unpaired surrogate",
                        ));
                    }
                    self.pos += 2;
                    let second = self.hex4()?;
                    if !(0xDC00..=0xDFFF).contains(&second) {
                        return Err(ValueError::new(
                            "JSON escape contains an unpaired surrogate",
                        ));
                    }
                    let combined =
                        0x10000 + (u32::from(first - 0xD800) << 10) + u32::from(second - 0xDC00);
                    char::from_u32(combined)
                        .ok_or_else(|| ValueError::new("JSON escape encodes an invalid scalar"))?
                } else if (0xDC00..=0xDFFF).contains(&first) {
                    return Err(ValueError::new(
                        "JSON escape contains an unpaired surrogate",
                    ));
                } else {
                    char::from_u32(u32::from(first))
                        .ok_or_else(|| ValueError::new("JSON escape encodes an invalid scalar"))?
                };
                out.push(character);
            }
            _ => return Err(ValueError::new("JSON contains an invalid escape")),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u16, ValueError> {
        let digits = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| ValueError::new("JSON unicode escape is truncated"))?;
        let mut value: u16 = 0;
        for &digit in digits {
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return Err(ValueError::new("JSON unicode escape is not hexadecimal")),
            };
            value = value * 16 + u16::from(nibble);
        }
        self.pos += 4;
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, ValueError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
            if self.bytes.get(self.pos..self.pos + 8) == Some(b"Infinity") {
                return Err(ValueError::new("non-JSON numeric constant: -Infinity"));
            }
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(ValueError::new("JSON number has a leading zero"));
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(ValueError::new("JSON number is malformed")),
        }
        let mut floating = false;
        if self.peek() == Some(b'.') {
            floating = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(ValueError::new("JSON number has an empty fraction"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            floating = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(ValueError::new("JSON number has an empty exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = self
            .bytes
            .get(start..self.pos)
            .and_then(|slice| std::str::from_utf8(slice).ok())
            .ok_or_else(|| ValueError::new("JSON number is malformed"))?;
        if floating {
            let value: f64 = text
                .parse()
                .map_err(|_| ValueError::new("JSON number is out of range"))?;
            Ok(Json::Float(value))
        } else {
            Ok(text
                .parse::<i64>()
                .map_or_else(|_| Json::BigInt(String::from(text)), Json::Int))
        }
    }
}

const fn utf8_length(lead: u8) -> Option<usize> {
    match lead {
        0x00..=0x7F => Some(1),
        0xC0..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF7 => Some(4),
        _ => None,
    }
}

pub fn python_repr(value: &Json) -> String {
    match value {
        Json::Null => String::from("None"),
        Json::Bool(true) => String::from("True"),
        Json::Bool(false) => String::from("False"),
        Json::Int(item) => item.to_string(),
        Json::BigInt(text) => text.clone(),
        Json::Float(item) => python_float_repr(*item),
        Json::Str(text) => python_str_repr(text),
        Json::Array(items) => {
            let members: Vec<String> = items.iter().map(python_repr).collect();
            format!("[{}]", members.join(", "))
        }
        Json::Object(items) => {
            let members: Vec<String> = items
                .iter()
                .map(|(key, item)| format!("{}: {}", python_str_repr(key), python_repr(item)))
                .collect();
            format!("{{{}}}", members.join(", "))
        }
    }
}

pub fn python_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if u32::from(other) < 0x20 || other == '\u{7f}' => {
                let _ = write!(out, "\\x{:02x}", u32::from(other));
            }
            other if other == quote => {
                out.push('\\');
                out.push(other);
            }
            other => out.push(other),
        }
    }
    out.push(quote);
    out
}

#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn parses_and_reencodes_nested_documents() {
        let text = r#"{"a":[1,2.5,"x",null,true,{"b":"é"}],"c":{}}"#;
        let value = Json::parse(text).unwrap();
        let mut encoded = String::new();
        value.encode(&mut encoded);
        assert_eq!(encoded, text);
        let reparsed = Json::parse(&encoded).unwrap();
        assert_eq!(reparsed, value);
    }

    #[test]
    fn rejects_duplicate_object_keys() {
        assert!(Json::parse(r#"{"a":1,"a":2}"#).is_err());
        assert!(Json::parse(r#"{"a":{"b":1,"b":2}}"#).is_err());
    }

    #[test]
    fn rejects_malformed_documents() {
        let bad = [
            "",
            "{",
            "[1,]",
            "{\"a\"}",
            "01",
            "1.",
            "1e",
            "tru",
            "nul",
            "NaN",
            "Infinity",
            "\"a\" \"b\"",
            "[1] x",
            "\"\u{1}\"",
        ];
        for text in bad {
            assert!(Json::parse(text).is_err(), "must reject {text}");
        }
    }

    #[test]
    fn rejects_unpaired_surrogate_escapes() {
        assert!(Json::parse(r#""\ud800""#).is_err());
        assert!(Json::parse(r#""\udc00""#).is_err());
        assert!(Json::parse(r#""\ud800\ud800""#).is_err());
    }

    #[test]
    fn pairs_surrogate_escapes_into_scalars() {
        let value = Json::parse(r#""😀""#).unwrap();
        assert_eq!(value.as_str(), Some("\u{1F600}"));
    }

    #[test]
    fn parses_overflowing_exponents_to_infinity() {
        let value = Json::parse("[1e400]").unwrap();
        let number = value.at(0).and_then(Json::as_float).unwrap();
        assert!(number.is_infinite());
    }

    #[test]
    fn retains_integers_beyond_signed_64_bits() {
        let beyond = Json::parse("[9223372036854775808]").unwrap();
        assert_eq!(
            beyond.at(0).and_then(Json::as_int),
            None,
            "out-of-range integers are not i64"
        );
        let mut encoded = String::new();
        beyond.encode(&mut encoded);
        assert_eq!(encoded, "[9223372036854775808]");
        let negative = Json::parse("[-9223372036854775809]").unwrap();
        let mut encoded = String::new();
        negative.encode(&mut encoded);
        assert_eq!(encoded, "[-9223372036854775809]");
        assert_eq!(
            Json::parse("[9223372036854775807]")
                .unwrap()
                .at(0)
                .and_then(Json::as_int),
            Some(i64::MAX)
        );
    }

    #[test]
    fn rejects_non_json_constants_with_reference_messages() {
        let cases = [
            ("[NaN]", "non-JSON numeric constant: NaN"),
            ("[Infinity]", "non-JSON numeric constant: Infinity"),
            ("[-Infinity]", "non-JSON numeric constant: -Infinity"),
        ];
        for (text, message) in cases {
            assert_eq!(Json::parse(text).unwrap_err().message(), message);
        }
    }

    #[test]
    fn rejects_excessive_nesting() {
        let mut text = String::new();
        for _ in 0..600 {
            text.push('[');
        }
        for _ in 0..600 {
            text.push(']');
        }
        assert!(Json::parse(&text).is_err());
    }

    #[test]
    fn writes_floats_like_python_repr() {
        let cases = [
            ("0x0.0p+0", "0.0"),
            ("-0x0.0p+0", "-0.0"),
            ("0x1.0000000000000p+0", "1.0"),
            ("-0x1.0000000000000p+0", "-1.0"),
            ("0x1.4000000000000p+1", "2.5"),
            ("0x1.999999999999ap-4", "0.1"),
            ("0x1.5555555555555p-2", "0.3333333333333333"),
            ("0x1.c6bf526340000p+49", "1000000000000000.0"),
            ("0x1.1c37937e08000p+53", "1e+16"),
            ("0x1.6345785d8a000p+56", "1e+17"),
            ("0x1.a36e2eb1c432dp-14", "0.0001"),
            ("0x1.4f8b588e368f1p-17", "1e-05"),
            ("0x1.ad7f29abcaf48p-24", "1e-07"),
            ("0x1.b69b4ba630f35p+56", "1.2345678901234568e+17"),
            ("0x0.0000000000001p-1022", "5e-324"),
            ("0x1.fffffffffffffp+1023", "1.7976931348623157e+308"),
            ("0x1.0000000000000p-1022", "2.2250738585072014e-308"),
            ("0x1.9000000000000p+6", "100.0"),
            ("0x1.0000000000000p-1", "0.5"),
            ("0x1.249ad2594c37dp+332", "1e+100"),
            ("-0x1.bff2ee48e0530p-333", "-1e-100"),
            ("0x1.2d687e4189375p+20", "1234567.891"),
            ("0x1.b1ae4d6e2ef50p+69", "1e+21"),
            ("0x1.921fb54442d11p+1", "3.14159265358979"),
        ];
        for (hex, expected) in cases {
            let number = crate::values::float_from_hex(hex).unwrap();
            let mut encoded = String::new();
            Json::Float(number).encode(&mut encoded);
            assert_eq!(encoded, expected, "python repr mismatch for {hex}");
        }
    }

    #[test]
    fn escapes_control_characters_with_lowercase_hex() {
        let value = Json::Str(String::from("a\u{1}b"));
        let mut encoded = String::new();
        value.encode(&mut encoded);
        assert_eq!(encoded, "\"a\\u0001b\"");
    }
}
