use crate::values::ValueError;
use unicode_normalization::UnicodeNormalization as _;

const RESERVED_NAMES: [&str; 30] = [
    "con", "prn", "aux", "nul", "conin$", "conout$", "com1", "com2", "com3", "com4", "com5",
    "com6", "com7", "com8", "com9", "com¹", "com²", "com³", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5",
    "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³",
];
const INVALID_PATH_CHARACTERS: [char; 8] = ['<', '>', ':', '"', '\\', '|', '?', '*'];

pub fn validate_identifier(value: &str, label: &str) -> Result<(), ValueError> {
    let length = value.chars().count();
    let shaped = (1..=64).contains(&length)
        && value.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if !shaped {
        return Err(ValueError::new(format!(
            "{label} must be 1-64 lowercase ASCII letters/digits with internal hyphens"
        )));
    }
    if RESERVED_NAMES.contains(&value) {
        return Err(ValueError::new(format!(
            "{label} is a reserved Windows name"
        )));
    }
    Ok(())
}

pub fn validate_version(value: &str) -> Result<(), ValueError> {
    if value.chars().all(is_python_space) {
        return Err(ValueError::new("version must be nonblank"));
    }
    Ok(())
}

pub fn validate_relative_path(value: &str, label: &str) -> Result<(), ValueError> {
    if value.is_empty() || value.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err(ValueError::new(format!(
            "{label} must be an unambiguous relative path"
        )));
    }
    if value
        .chars()
        .any(|character| INVALID_PATH_CHARACTERS.contains(&character) || is_control(character))
    {
        return Err(ValueError::new(format!(
            "{label} contains a nonportable character"
        )));
    }
    if !value.chars().eq(value.nfc()) {
        return Err(ValueError::new(format!(
            "{label} must contain NFC-normalized Unicode scalar values"
        )));
    }
    for part in value.split('/') {
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(ValueError::new(format!(
                "{label} contains a component ending in a dot or space"
            )));
        }
        let stem = part.split_once('.').map_or(part, |(stem, _)| stem);
        let folded = caseless::default_case_fold_str(stem.trim_end_matches(' '));
        if RESERVED_NAMES.contains(&folded.as_str()) {
            return Err(ValueError::new(format!(
                "{label} contains a reserved Windows name"
            )));
        }
    }
    Ok(())
}

pub fn portable_path_key(value: &str) -> String {
    caseless::default_case_fold_str(value).nfc().collect()
}

pub fn ancestors(value: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut path = String::new();
    for part in value.split('/') {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        paths.push(path.clone());
    }
    paths
}

const fn is_control(character: char) -> bool {
    matches!(character, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}')
}

pub const fn is_python_space(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n'
            | '\u{B}'
            | '\u{C}'
            | '\r'
            | '\u{1C}'..='\u{1F}'
            | ' '
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

pub fn is_python_blank(value: &str) -> bool {
    value.chars().all(is_python_space)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::{portable_path_key, validate_identifier, validate_relative_path, validate_version};

    #[test]
    fn accepts_hyphenated_lowercase_identifiers() {
        for value in ["a", "team-notes", "notes2", "a-b-c", &"x".repeat(64)] {
            assert!(
                validate_identifier(value, "bundle id").is_ok(),
                "must accept {value}"
            );
        }
    }

    #[test]
    fn rejects_malformed_identifiers() {
        for value in [
            "",
            "-a",
            "a-",
            "a--b",
            "A",
            "a_b",
            "a b",
            "ä",
            &"x".repeat(65),
        ] {
            assert!(
                validate_identifier(value, "bundle id").is_err(),
                "must reject {value}"
            );
        }
    }

    #[test]
    fn rejects_reserved_windows_identifiers() {
        for value in ["con", "prn", "aux", "nul", "com1", "lpt9"] {
            let failure = validate_identifier(value, "bundle id").unwrap_err();
            assert_eq!(failure.message(), "bundle id is a reserved Windows name");
        }
    }

    #[test]
    fn requires_nonblank_versions() {
        assert!(validate_version("1.0.0").is_ok());
        assert!(validate_version("  1 ").is_ok());
        for value in ["", " ", "\t\n", "\u{1c}", "\u{a0}"] {
            assert!(validate_version(value).is_err(), "must reject {value:?}");
        }
    }

    #[test]
    fn accepts_portable_relative_paths() {
        for value in ["skills/notes.md", "a/b/c.txt", "café.md", "naïve/file.json"] {
            assert!(
                validate_relative_path(value, "path").is_ok(),
                "must accept {value}"
            );
        }
    }

    #[test]
    fn rejects_nonportable_relative_paths() {
        for value in [
            "",
            "/absolute.md",
            "a//b.md",
            "./a.md",
            "../a.md",
            "a/../b.md",
            "a\\b.md",
            "a:b.md",
            "a<b.md",
            "a?b.md",
            "a|b.md",
            "a\"b.md",
            "a*b.md",
            "a\u{1}b.md",
            "a\u{7f}b.md",
            "trailing./b.md",
            "trailing /b.md",
            "b.md ",
            "con/file.md",
            "a/NUL.txt",
            "a/nul .txt",
            "a/cafe\u{301}.md",
        ] {
            assert!(
                validate_relative_path(value, "path").is_err(),
                "must reject {value:?}"
            );
        }
    }

    #[test]
    fn folds_collision_keys_like_python_casefold() {
        assert_eq!(portable_path_key("Skills/Notes.MD"), "skills/notes.md");
        assert_eq!(portable_path_key("STRASSE"), "strasse");
        assert_eq!(portable_path_key("straße"), "strasse");
        assert_eq!(portable_path_key("ΣΊΣΥΦΟΣ"), "σίσυφοσ");
        assert_eq!(portable_path_key("ﬁle"), "file");
        assert_eq!(portable_path_key("cafe\u{301}"), "café");
    }
}
