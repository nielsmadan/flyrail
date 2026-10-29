#![allow(clippy::expect_used)]

use super::*;

fn path(names: &[&str]) -> Vec<PathPart> {
    names
        .iter()
        .map(|name| PathPart::Key((*name).to_owned()))
        .collect()
}

#[test]
fn malformed_native_tokens_report_reference_diagnostics() {
    let cases = [
        ("x={}\nx.y=2\n", "Key \"x\" already exists. at line 2 col 0"),
        ("x=1_e2\n", "Invalid number at line 1 col 6"),
        ("x=1e_2\n", "Invalid number at line 1 col 6"),
        ("x=\"\\u+123\"\n", "Invalid unicode value at line 1 col 4"),
        (
            "x=2000-01-01T12:34:56+01\n",
            "Invalid number at line 1 col 24",
        ),
        ("x=[1 2]", "Unexpected character: '2' at line 1 col 5"),
        ("x=[1,,2]", "Unexpected character: ',' at line 1 col 5"),
        ("x=01", "Invalid number at line 1 col 4"),
        ("x=1.", "Invalid number at line 1 col 4"),
        ("x=1e", "Invalid number at line 1 col 4"),
        ("x=\"abc", "Unexpected end of file at line 1 col 6"),
        (
            "x=\"a\\q\"",
            "Invalid character 'q' in string at line 1 col 5",
        ),
        ("x", "Unexpected character: '\\x00' at line 1 col 1"),
        ("[a", "Unexpected end of file at line 1 col 2"),
        ("x=", "Unexpected character: '\\x00' at line 1 col 2"),
        ("x=truex", "Unexpected character: 'x' at line 1 col 6"),
        ("x=2026-02-30", "Invalid date at line 1 col 12"),
    ];
    for (source, diagnostic) in cases {
        assert_eq!(
            TomlDocument::parse(source)
                .expect_err("malformed TOML")
                .message(),
            diagnostic
        );
    }
}

#[test]
fn compounded_inline_dotted_depth_is_bounded() {
    let prefix = format!("{{{}=", vec!["a"; 100].join("."));
    let source = format!("x={}1{}\n", prefix.repeat(100), "}".repeat(100));
    assert!(
        TomlDocument::parse(&source)
            .expect_err("bounded compound depth")
            .message()
            .contains("nested more than 100 levels deep")
    );
}

#[test]
fn native_time_precision_preserves_bytes_and_truncates_values() {
    for (native, canonical) in [
        ("00:00:00.123456789", "00:00:00.123456"),
        (
            "2000-01-01T00:00:00.123456789",
            "2000-01-01T00:00:00.123456",
        ),
        (
            "2000-01-01T00:00:00.123456789Z",
            "2000-01-01T00:00:00.123456Z",
        ),
        (
            "2000-01-01 01:00:00.123456789+01:00",
            "2000-01-01T00:00:00.123456Z",
        ),
    ] {
        let source = format!("x={native}\n");
        let mut document = TomlDocument::parse(&source).expect("native precision");
        assert_eq!(
            document.native(&path(&["x"])).expect("value"),
            parse_temporal(canonical)
                .map(Value::Scalar)
                .expect("canonical value")
        );
        document
            .put(&path(&["y"]), &Value::Scalar(Scalar::integer(1)), None, &[])
            .expect("unrelated edit");
        assert_eq!(document.text(), format!("{source}y = 1\n"));
        document.delete(&path(&["y"])).expect("retire");
        assert_eq!(document.text(), source);
    }
}

#[test]
fn inline_field_insertion_round_trips_closing_trivia() {
    for gap in ["", " ", "   ", "\t \t"] {
        let source = format!("x={{a=1{gap}}}\nforeign=2\n");
        let mut document = TomlDocument::parse(&source).expect("inline table");
        document
            .put(
                &path(&["x", "b"]),
                &Value::Scalar(Scalar::integer(3)),
                None,
                &[],
            )
            .expect("insert field");
        assert_eq!(
            document.text(),
            format!("x={{a=1,{gap}b = 3}}\nforeign=2\n")
        );
        document.delete(&path(&["x", "b"])).expect("retire field");
        assert_eq!(document.text(), source);
    }
}

#[test]
fn selected_table_syntax_strips_only_its_header_trivia() {
    let source = "  [x] # header\ncommand = 1\n[x . env] # nested\nX=2\n";
    let document = TomlDocument::parse(source).expect("table");
    assert_eq!(
        document.syntax(&path(&["x"])).expect("syntax"),
        b"[value]\ncommand = 1\n[value. env] # nested\nX=2\n"
    );
    let dotted = TomlDocument::parse("x.command = 1\nx.other = 2\n").expect("dotted table");
    assert_eq!(
        dotted.owned_syntax(&path(&["x"])).expect("owned syntax"),
        b"command = 1\nother = 2\n"
    );
}

#[test]
fn detached_comments_keep_original_indentation() {
    for source in ["  x=1 # foreign\n", "  [x] # foreign\na=1\n"] {
        let mut document = TomlDocument::parse(source).expect("indented declaration");
        assert_eq!(document.text(), source);
        document.delete(&path(&["x"])).expect("retire");
        assert_eq!(document.text(), "   # foreign\n");
    }
}

#[test]
fn foreign_nonfinite_values_remain_editable_but_unownable() {
    let mut document = TomlDocument::parse("x=1\nforeign=inf\n").expect("native TOML");
    document
        .put(&path(&["x"]), &Value::Scalar(Scalar::integer(2)), None, &[])
        .expect("edit finite value");
    assert_eq!(document.text(), "x=2\nforeign=inf\n");
    assert_eq!(
        document
            .syntax(&path(&["foreign"]))
            .expect_err("nonfinite selection")
            .message(),
        "floats must be finite"
    );
}
