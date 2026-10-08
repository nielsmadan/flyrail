use crate::json::Json;
use crate::values::ValueError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutPart {
    Field(String),
    Index(u64),
}

pub type LayoutPath = Vec<LayoutPart>;

fn invalid_header() -> ValueError {
    ValueError::new("invalid TOML restoration header")
}

pub fn header_path(name: &str) -> Result<Vec<String>, ValueError> {
    let mut parts = Vec::new();
    let mut rest = skip_blanks(name);
    loop {
        let (part, remaining) = header_key(rest)?;
        parts.push(part);
        rest = skip_blanks(remaining);
        let Some(after_dot) = rest.strip_prefix('.') else {
            return if rest.is_empty() {
                Ok(parts)
            } else {
                Err(invalid_header())
            };
        };
        rest = skip_blanks(after_dot);
    }
}

fn skip_blanks(text: &str) -> &str {
    let mut rest = text;
    while let Some(next) = rest.strip_prefix([' ', '\t']) {
        rest = next;
    }
    rest
}

fn header_key(text: &str) -> Result<(String, &str), ValueError> {
    let first = text.chars().next().ok_or_else(invalid_header)?;
    if first == '\'' {
        return literal_key(text);
    }
    if first == '"' {
        return basic_key(text);
    }
    if is_bare_key_character(first) {
        let end = text
            .find(|character: char| !is_bare_key_character(character))
            .unwrap_or(text.len());
        let key = text.get(0..end).ok_or_else(invalid_header)?;
        return Ok((key.to_owned(), text.get(end..).unwrap_or("")));
    }
    Err(invalid_header())
}

const fn is_bare_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '-'
}

fn literal_key(text: &str) -> Result<(String, &str), ValueError> {
    let mut end = None;
    for (index, character) in text.char_indices().skip(1) {
        if character == '\'' {
            end = Some(index);
            break;
        }
        if u32::from(character) < 0x20 || character == '\u{7f}' {
            return Err(invalid_header());
        }
    }
    let end = end.ok_or_else(invalid_header)?;
    let key = text.get(1..end).ok_or_else(invalid_header)?;
    Ok((
        key.to_owned(),
        text.get(end + 1..).ok_or_else(invalid_header)?,
    ))
}

fn basic_key(text: &str) -> Result<(String, &str), ValueError> {
    let mut out = String::new();
    let mut rest = text.get(1..).ok_or_else(invalid_header)?;
    while let Some(character) = rest.chars().next() {
        if character == '"' {
            return Ok((out, rest.get(1..).unwrap_or("")));
        }
        if u32::from(character) < 0x20 || character == '\u{7f}' {
            return Err(invalid_header());
        }
        if character != '\\' {
            out.push(character);
            rest = rest
                .get(character.len_utf8()..)
                .ok_or_else(invalid_header)?;
            continue;
        }
        rest = rest.get(1..).ok_or_else(invalid_header)?;
        let escape = rest.chars().next().ok_or_else(invalid_header)?;
        if escape == '\r' || escape == '\n' {
            return Err(invalid_header());
        }
        rest = rest.get(escape.len_utf8()..).ok_or_else(invalid_header)?;
        match escape {
            'b' => out.push('\u{8}'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'f' => out.push('\u{c}'),
            'r' => out.push('\r'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'u' => rest = unicode_escape(rest, 4, &mut out)?,
            'U' => rest = unicode_escape(rest, 8, &mut out)?,
            _ => return Err(invalid_header()),
        }
    }
    Err(invalid_header())
}

fn unicode_escape<'a>(
    rest: &'a str,
    digits: usize,
    out: &mut String,
) -> Result<&'a str, ValueError> {
    let hex = rest.get(0..digits).ok_or_else(invalid_header)?;
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_header());
    }
    let code = u32::from_str_radix(hex, 16).map_err(|_| invalid_header())?;
    let decoded = char::from_u32(code).ok_or_else(invalid_header)?;
    out.push(decoded);
    rest.get(digits..).ok_or_else(invalid_header)
}

pub fn layout_headers(layout: &[u8]) -> Result<Vec<(LayoutPath, String)>, ValueError> {
    if layout.is_empty() {
        return Ok(Vec::new());
    }
    let invalid_layout = || ValueError::new("invalid TOML restoration layout");
    let text = std::str::from_utf8(layout).map_err(|_| invalid_layout())?;
    let record = Json::parse(text).map_err(|_| invalid_layout())?;
    let items = record.as_object().ok_or_else(invalid_layout)?;
    if items.len() != 2
        || items
            .iter()
            .any(|(key, _)| key != "version" && key != "headers")
    {
        return Err(invalid_layout());
    }
    if record.get("version").and_then(Json::as_int) != Some(1) {
        return Err(invalid_layout());
    }
    let headers = record
        .get("headers")
        .and_then(Json::as_array)
        .ok_or_else(invalid_layout)?;
    let mut result = Vec::new();
    let mut seen: Vec<LayoutPath> = Vec::new();
    let invalid_entry = || ValueError::new("invalid TOML restoration header entry");
    let invalid_path = || ValueError::new("invalid TOML restoration path");
    for entry in headers {
        let pair = entry.as_array().ok_or_else(invalid_entry)?;
        if pair.len() != 2 {
            return Err(invalid_entry());
        }
        let relative = pair
            .first()
            .and_then(Json::as_array)
            .ok_or_else(invalid_path)?;
        if relative.is_empty() {
            return Err(invalid_path());
        }
        let mut path: LayoutPath = Vec::new();
        for part in relative {
            path.push(match part {
                Json::Str(field) => LayoutPart::Field(field.clone()),
                Json::Int(index) if *index >= 0 => {
                    LayoutPart::Index(u64::try_from(*index).map_err(|_| invalid_path())?)
                }
                _ => return Err(invalid_path()),
            });
        }
        if seen.contains(&path) {
            return Err(ValueError::new("duplicate TOML restoration path"));
        }
        seen.push(path.clone());
        let name = pair
            .get(1)
            .and_then(Json::as_str)
            .ok_or_else(|| ValueError::new("invalid TOML restoration header"))?;
        header_path(name)?;
        result.push((path, name.to_owned()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(layout: &[u8]) -> String {
        match layout_headers(layout) {
            Ok(headers) => format!("{headers:?}"),
            Err(error) => String::from(error.message()),
        }
    }

    #[test]
    fn layout_failures_report_the_python_messages() {
        let cases = [
            (
                &br#"{"version":1,"headers":[5]}"#[..],
                "invalid TOML restoration header entry",
            ),
            (
                &br#"{"version":1,"headers":[[1,2,3]]}"#[..],
                "invalid TOML restoration header entry",
            ),
            (
                &br#"{"version":1,"headers":[[5,"a"]]}"#[..],
                "invalid TOML restoration path",
            ),
            (
                &br#"{"version":1,"headers":[[[],"a"]]}"#[..],
                "invalid TOML restoration path",
            ),
            (
                &br#"{"version":1,"headers":[[[1.5],"a"]]}"#[..],
                "invalid TOML restoration path",
            ),
            (
                &br#"{"version":1,"headers":[[[-1],"a"]]}"#[..],
                "invalid TOML restoration path",
            ),
            (
                &br#"{"version":1,"headers":[[[true],"a"]]}"#[..],
                "invalid TOML restoration path",
            ),
            (
                &br#"{"version":1,"headers":[[["a"],5]]}"#[..],
                "invalid TOML restoration header",
            ),
            (
                &br#"{"version":1,"headers":[[["a"],"a."]]}"#[..],
                "invalid TOML restoration header",
            ),
            (
                &br#"{"version":1,"headers":[[["a"],"a"],[["a"],"b"]]}"#[..],
                "duplicate TOML restoration path",
            ),
            (
                &br#"{"version":2,"headers":[]}"#[..],
                "invalid TOML restoration layout",
            ),
            (
                &br#"{"version":true,"headers":[]}"#[..],
                "invalid TOML restoration layout",
            ),
            (
                &br#"{"version":1000000000000000000000,"headers":[]}"#[..],
                "invalid TOML restoration layout",
            ),
            (&b"not json"[..], "invalid TOML restoration layout"),
        ];
        for (layout, expected) in cases {
            assert_eq!(
                outcome(layout),
                expected,
                "layout {}",
                String::from_utf8_lossy(layout)
            );
        }
    }

    #[test]
    fn layout_accepts_field_and_index_paths() {
        assert_eq!(
            layout_headers(br#"{"version":1,"headers":[]}"#),
            Ok(Vec::new())
        );
        assert_eq!(
            layout_headers(br#"{"version":1,"headers":[[["a"],"a.b"],[["a",0],"a.b"]]}"#),
            Ok(vec![
                (
                    vec![LayoutPart::Field(String::from("a"))],
                    String::from("a.b")
                ),
                (
                    vec![LayoutPart::Field(String::from("a")), LayoutPart::Index(0)],
                    String::from("a.b")
                ),
            ])
        );
    }
}
