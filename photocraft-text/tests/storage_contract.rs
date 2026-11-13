//! Small, fixture-free contracts for the two editable-text storage codecs.

use photocraft_text::Error;
use photocraft_text::engine_data::{self, Value as EngineValue};
use photocraft_text::txt2::{self, Kind, Step, Value};

const ESCAPED_TEXT: &str = "\0(\\)\u{2800}\u{2900}\u{5c00}\u{1d11e}";
const ESCAPED_UTF16: &[u8] = b"\xfe\xff\x00\x00\x00\x5c\x28\x00\x5c\x5c\x00\x5c\x29\x5c\x28\x00\x5c\x29\x00\x5c\x5c\x00\xd8\x34\xdd\x1e\x00\x0d";
const CACHE_PATH: &[Step<'static>] = &[
    Step::Key("1"),
    Step::Key("1"),
    Step::Index(0),
    Step::Key("1"),
    Step::Key("2"),
];
const STYLE_TABLE_PATH: &[Step<'static>] = &[
    Step::Key("1"),
    Step::Key("1"),
    Step::Index(0),
    Step::Key("0"),
    Step::Key("6"),
    Step::Key("0"),
];

fn editable_block(text: &str) -> Vec<u8> {
    let units = text.encode_utf16().count() + 1;
    let mut block = b"\t/0 << /1 << /0 [ ] >> >>\n/1 << /1 [ << /0 << /0 (".to_vec();
    block.extend_from_slice(&txt2::utf16(text));
    block.extend_from_slice(
        format!(
            ") /5 << /0 [ << /0 << /0 << /5 << /0 0 /7 1.2 >> >> >> /1 {units} >> ] >> \
             /6 << /0 [ << /0 << /0 << /6 << /0 0 /1 48.0 /8 25 >> >> >> /1 {units} >> ] >> \
             >> /1 << /0 [ << /0 0 >> ] /2 [ << /99 /PC /1 [ 12.5 8.0 ] >> ] >> >> ] >>\n\
             /98 << /0 (opaque\\(data\\)) /1 /nil >>\r\n"
        )
        .as_bytes(),
    );
    block
}

fn run_sheet_path(run: usize) -> Vec<Step<'static>> {
    let mut path = txt2::run_path_for(run);
    path.extend_from_slice(&[Step::Key("0"), Step::Key("0"), Step::Key("6")]);
    path
}

fn assert_splice(before: &[u8], after: &[u8], start: usize, end: usize, value: &[u8]) {
    let mut expected = before[..start].to_vec();
    expected.extend_from_slice(value);
    expected.extend_from_slice(&before[end..]);
    assert_eq!(after, expected, "bytes outside the edited span changed");
}

#[test]
fn engine_data_canonical_bytes_keep_entry_order_and_scalar_types() {
    let value = EngineValue::dict([
        ("99", EngineValue::Name("Kind".into())),
        (
            "1",
            EngineValue::Array(vec![
                EngineValue::Int(-7),
                EngineValue::Float(2.0),
                EngineValue::Float(0.125),
                EngineValue::Float(-0.25),
                EngineValue::Bool(true),
                EngineValue::Bool(false),
            ]),
        ),
        ("0", EngineValue::Array(vec![])),
        ("Child", EngineValue::dict([("A", EngineValue::Int(3))])),
    ]);
    let expected = b"\n\n<<\n\t/99 /Kind\n\t/1 [ -7 2.0 .125 -.25 true false ]\n\t/0 [ ]\n\t/Child\n\t<<\n\t\t/A 3\n\t>>\n>>";
    assert_eq!(engine_data::write(&value), expected);
    assert_eq!(engine_data::parse(expected).unwrap(), value);
}

#[test]
fn engine_data_utf16_escapes_both_bytes_of_each_code_unit() {
    let value = EngineValue::dict([("Text", EngineValue::Str(format!("{ESCAPED_TEXT}\r")))]);
    let mut expected = b"\n\n<<\n\t/Text (".to_vec();
    expected.extend_from_slice(ESCAPED_UTF16);
    expected.extend_from_slice(b")\n>>");
    assert_eq!(engine_data::write(&value), expected);
    assert_eq!(engine_data::parse(&expected).unwrap(), value);
    assert_eq!(
        engine_data::write(&engine_data::parse(&expected).unwrap()),
        expected
    );
}

#[test]
fn engine_data_accepts_a_prefix_and_balanced_literal_parentheses() {
    let raw = b"\x00prefix\r\n<< /Text (outer(inner)tail) /Utf8 (caf\xc3\xa9) >>";
    let parsed = engine_data::parse(raw).unwrap();
    assert_eq!(
        parsed.get("Text").and_then(EngineValue::as_str),
        Some("outer(inner)tail")
    );
    assert_eq!(
        parsed.get("Utf8").and_then(EngineValue::as_str),
        Some("café")
    );
    assert_eq!(
        engine_data::parse(&engine_data::write(&parsed)).unwrap(),
        parsed
    );
}

#[test]
fn engine_data_rejects_malformed_values_and_nested_truncation() {
    let cases: &[(&str, &[u8])] = &[
        ("invalid dictionary key", b"<< 1 >>"),
        ("missing value", b"<< /K >>"),
        ("single angle opener", b"<< /K < >>"),
        ("bad boolean", b"<< /K truth >>"),
        ("bad integer", b"<< /K --1 >>"),
        ("bad real", b"<< /K 1.2.3 >>"),
        ("stray array token", b"<< /K [ 1 @ ] >>"),
        ("truncated array", b"<< /K [ 1"),
        ("truncated nested dictionary", b"<< /K << /V 1"),
        ("truncated string escape", b"<< /K (unfinished\\"),
    ];
    for (name, bytes) in cases {
        assert!(
            matches!(engine_data::parse(bytes), Err(Error::EngineData(_))),
            "{name}"
        );
    }
}

#[test]
fn txt2_tokens_keep_raw_strings_kinds_and_all_separators() {
    let block = b"\0\t/0 <<\r\n/1 [ -2 0.125 true false /nil /Named (outer(inner)\\)\\\\) ] /2 << >> >>\r\n\x0c\0";
    let tokens = txt2::tokens(block).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|token| token.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            Kind::Name("0".into()),
            Kind::DictOpen,
            Kind::Name("1".into()),
            Kind::ArrayOpen,
            Kind::Int(-2),
            Kind::Real(0.125),
            Kind::Bool(true),
            Kind::Bool(false),
            Kind::Nil,
            Kind::Name("Named".into()),
            Kind::Str(b"outer(inner)\\)\\\\".to_vec()),
            Kind::ArrayClose,
            Kind::Name("2".into()),
            Kind::DictOpen,
            Kind::DictClose,
            Kind::DictClose,
        ]
    );
    let mut previous_end = 0;
    for token in &tokens {
        assert_eq!(token.sep_start, previous_end);
        assert!(token.sep_start <= token.start && token.start < token.end);
        assert!(token.end <= block.len());
        previous_end = token.end;
    }
    assert_eq!(
        txt2::dict_entries(&tokens, 1),
        Some(vec![("1".into(), 3), ("2".into(), 13)])
    );
    assert_eq!(txt2::skip_value(&tokens, 1), Some(tokens.len()));
    assert_eq!(txt2::skip_value(&tokens, tokens.len()), None);
    assert_eq!(txt2::untokenize(block, &tokens), block);
}

#[test]
fn txt2_rejects_malformed_lexical_tokens() {
    let cases: &[(&str, &[u8])] = &[
        ("unterminated string", b"(abc"),
        ("unterminated nested string", b"(a(b)c"),
        ("trailing escape", b"(abc\\"),
        ("single angle opener", b"<"),
        ("single angle closer", b">"),
        ("stray parenthesis", b")"),
        ("bad boolean", b"/0 tru"),
        ("bad real", b"/0 1.2.3"),
        ("integer overflow", b"/0 9223372036854775808"),
        ("non-UTF-8 scalar", b"/0 \xff"),
    ];
    for (name, bytes) in cases {
        assert!(matches!(txt2::tokens(bytes), Err(Error::Txt2(_))), "{name}");
        assert!(txt2::read_path(bytes, &[Step::Key("0")]).is_err(), "{name}");
    }
}

#[test]
fn txt2_value_navigation_reports_incomplete_containers_and_missing_paths() {
    for block in [b"[ /0 << /1 2 >>".as_slice(), b"<< /0 [ 1 ]"] {
        let tokens = txt2::tokens(block).unwrap();
        assert_eq!(txt2::skip_value(&tokens, 0), None);
    }
    for block in [b"<< 4 5 >>".as_slice(), b"<< /0"] {
        let tokens = txt2::tokens(block).unwrap();
        assert_eq!(txt2::dict_entries(&tokens, 0), None);
    }
    let block = b"/0 << /1 [ 7 ] >>";
    let paths: &[&[Step<'_>]] = &[
        &[],
        &[Step::Index(0)],
        &[Step::Key("missing")],
        &[Step::Key("0"), Step::Key("missing")],
        &[Step::Key("0"), Step::Key("1"), Step::Index(1)],
        &[
            Step::Key("0"),
            Step::Key("1"),
            Step::Index(0),
            Step::Key("0"),
        ],
    ];
    for path in paths {
        assert_eq!(txt2::read_path(block, path).unwrap(), None, "{path:?}");
        assert!(txt2::value_range(block, path).is_err(), "{path:?}");
        assert!(
            txt2::replace_value(block, path, &Value::Int(3)).is_err(),
            "{path:?}"
        );
    }
}

#[test]
fn txt2_ensure_value_orders_numeric_keys_without_reformatting_neighbors() {
    let block = b"/0 <<\n /2 2.0\t/11 false\r\n>>\0";
    let cases: &[(&str, Value, &[u8])] = &[
        (
            "1",
            Value::Int(9),
            b"/0 <<\n /1 9 /2 2.0\t/11 false\r\n>>\0",
        ),
        (
            "6",
            Value::Bool(true),
            b"/0 <<\n /2 2.0\t/6 true /11 false\r\n>>\0",
        ),
        (
            "12",
            Value::Real(1.25),
            b"/0 <<\n /2 2.0\t/11 false\r\n/12 1.25 >>\0",
        ),
        ("2", Value::Int(-5), b"/0 <<\n /2 -5\t/11 false\r\n>>\0"),
    ];
    for (key, value, expected) in cases {
        let path = [Step::Key("0"), Step::Key(key)];
        let out = txt2::ensure_value(block, &path, value).unwrap();
        assert_eq!(out, *expected, "key {key}");
        assert_eq!(
            txt2::ensure_value(&out, &path, value).unwrap(),
            out,
            "key {key}"
        );
    }
}

#[test]
fn txt2_ensure_value_refuses_missing_or_non_dictionary_parents() {
    let block = b"/0 << /1 7 /2 [ ] >>";
    let paths: &[&[Step<'_>]] = &[
        &[],
        &[Step::Key("missing"), Step::Key("1")],
        &[Step::Key("0"), Step::Key("1"), Step::Key("3")],
        &[Step::Key("0"), Step::Key("2"), Step::Key("3")],
        &[Step::Key("0"), Step::Key("2"), Step::Index(0)],
        &[Step::Key("0"), Step::Key("2")],
    ];
    for path in paths {
        assert!(
            txt2::ensure_value(block, path, &Value::Int(3)).is_err(),
            "{path:?}"
        );
    }
}

#[test]
fn txt2_same_utf16_length_splices_only_text_even_when_escaped_bytes_grow() {
    let block = editable_block("AB");
    let (start, end) = txt2::text_span(&block).unwrap();
    for text in ["\u{1d11e}", "\u{2800}\\"] {
        let out = txt2::replace_text(&block, text).unwrap();
        assert_splice(&block, &out, start, end, &txt2::utf16(text));
        assert_eq!(txt2::read_text(&out).as_deref(), Some(text));
        assert_eq!(txt2::run_lengths(&out).unwrap(), vec![3]);
        assert_eq!(txt2::paragraph_run_lengths(&out).unwrap(), vec![3]);
        assert_eq!(
            txt2::read_path(&out, CACHE_PATH).unwrap(),
            txt2::read_path(&block, CACHE_PATH).unwrap()
        );
        txt2::tokens(&out).unwrap();
    }
}

#[test]
fn txt2_rebuilds_empty_and_non_bmp_paragraphs_and_keeps_first_sheets() {
    let original = editable_block("ABCDE");
    let split = txt2::set_run_layout(&original, &[2, 4]).unwrap();
    let split = txt2::set_run_font_size(&split, 1, 96.0).unwrap();
    let first_sheet = txt2::read_path(&split, &run_sheet_path(0))
        .unwrap()
        .unwrap();
    let paragraph_sheet = txt2::read_path(&split, &txt2::paragraph_props_path(0))
        .unwrap()
        .unwrap();
    let text = "\r\u{1d11e}\r\rA\u{2800}\\";
    let out = txt2::replace_text(&split, &format!("{text}\r\n\r\n")).unwrap();
    assert_eq!(txt2::read_text(&out).as_deref(), Some(text));
    assert_eq!(txt2::run_count(&out).unwrap(), 1);
    assert_eq!(txt2::run_lengths(&out).unwrap(), vec![9]);
    assert_eq!(txt2::paragraph_run_lengths(&out).unwrap(), vec![1, 3, 1, 4]);
    assert_eq!(txt2::paragraph_count(&out).unwrap(), 4);
    assert_eq!(
        txt2::read_path(&out, &run_sheet_path(0)).unwrap().unwrap(),
        first_sheet
    );
    for paragraph in 0..4 {
        assert_eq!(
            txt2::read_path(&out, &txt2::paragraph_props_path(paragraph))
                .unwrap()
                .unwrap(),
            paragraph_sheet
        );
    }
    assert_eq!(
        txt2::read_path(&out, CACHE_PATH).unwrap().as_deref(),
        Some(b"[ ]".as_slice())
    );
    for key in ["0", "98"] {
        assert_eq!(
            txt2::read_path(&out, &[Step::Key(key)]).unwrap(),
            txt2::read_path(&split, &[Step::Key(key)]).unwrap()
        );
    }
    txt2::tokens(&out).unwrap();
    let crlf = text.replace('\r', "\r\n") + "\r\n";
    assert_eq!(
        txt2::replace_text(&out, &crlf).unwrap(),
        out,
        "normalization-only edit changed bytes"
    );
}

#[test]
fn txt2_run_layout_counts_unescaped_utf16_and_gives_the_last_run_the_terminator() {
    let block = txt2::replace_text(&editable_block("seed"), "\u{1d11e}(A)\\\rB").unwrap();
    let split = txt2::set_run_layout(&block, &[2, 4, 2]).unwrap();
    assert_eq!(txt2::run_lengths(&split).unwrap(), vec![2, 4, 3]);
    assert_eq!(txt2::paragraph_run_lengths(&split).unwrap(), vec![7, 2]);
    assert_eq!(
        txt2::read_text(&split).as_deref(),
        Some("\u{1d11e}(A)\\\rB")
    );
    assert_eq!(txt2::set_run_layout(&block, &[2, 4, 3]).unwrap(), split);
    assert_eq!(txt2::set_run_lengths(&split, &[2, 4, 3]).unwrap(), split);
    assert!(
        txt2::set_run_lengths(&split, &[1, 4, 3]).is_err(),
        "Unicode scalar count was accepted as UTF-16 length"
    );
    assert!(
        txt2::set_run_lengths(&split, &[2, 4, 2]).is_err(),
        "missing terminator was accepted"
    );
    assert!(
        txt2::set_run_layout(&split, &[2, 4, 3]).is_err(),
        "existing run splits were silently discarded"
    );
    let sheet = txt2::read_path(&block, &run_sheet_path(0))
        .unwrap()
        .unwrap();
    for run in 0..3 {
        assert_eq!(
            txt2::read_path(&split, &run_sheet_path(run))
                .unwrap()
                .unwrap(),
            sheet
        );
    }
}

#[test]
fn txt2_empty_text_retains_one_terminator_and_cannot_donate_a_run_unit() {
    let block = txt2::replace_text(&editable_block("seed"), "\r\n").unwrap();
    assert_eq!(txt2::read_text(&block).as_deref(), Some(""));
    assert_eq!(txt2::run_lengths(&block).unwrap(), vec![1]);
    assert_eq!(txt2::paragraph_run_lengths(&block).unwrap(), vec![1]);
    assert_eq!(txt2::set_run_layout(&block, &[1]).unwrap(), block);
    assert!(txt2::duplicate_run(&block, 0).is_err());
    assert!(txt2::duplicate_run(&block, 1).is_err());
    assert!(txt2::set_run_layout(&block, &[1, 1]).is_err());
}

#[test]
fn txt2_empty_text_rejects_an_empty_run_layout_without_panicking() {
    let block = editable_block("");
    assert_eq!(txt2::read_text(&block).as_deref(), Some(""));
    assert_eq!(txt2::run_lengths(&block).unwrap(), vec![1]);
    assert!(matches!(
        txt2::set_run_layout(&block, &[]),
        Err(Error::Txt2(_))
    ));
}

#[test]
fn txt2_run_readers_reject_missing_non_array_and_non_integer_tables() {
    assert!(txt2::run_count(b"/1 << >>").is_err());
    let block = editable_block("AB");
    let scalar = txt2::replace_container(&block, STYLE_TABLE_PATH, b"3").unwrap();
    assert!(txt2::run_count(&scalar).is_err());
    for table in [
        b"[ << /0 << >> >> ]".as_slice(),
        b"[ << /1 3.0 >> ]",
        b"[ << /1 /Wrong >> ]",
    ] {
        let malformed = txt2::replace_container(&block, STYLE_TABLE_PATH, table).unwrap();
        assert_eq!(txt2::run_count(&malformed).unwrap(), 1);
        assert!(txt2::run_lengths(&malformed).is_err(), "{table:?}");
    }
}

#[test]
fn txt2_requires_a_bom_terminator_and_valid_surrogate_pairs_to_edit_text() {
    let block = editable_block("AB");
    let (start, end) = txt2::text_span(&block).unwrap();
    let malformed: &[&[u8]] = &[
        b"\xff\xfe\x00A\x00\x0d",
        b"\xfe\xff\x00A",
        b"\xfe\xff\xd8\x00\x00\x0d",
        b"\xfe\xff\xdc\x00\x00\x0d",
    ];
    for bytes in malformed {
        let mut invalid = block[..start].to_vec();
        invalid.extend_from_slice(bytes);
        invalid.extend_from_slice(&block[end..]);
        assert_eq!(txt2::read_text(&invalid), None, "{bytes:?}");
        assert!(txt2::replace_text(&invalid, "fixed").is_err(), "{bytes:?}");
    }
}

#[test]
fn txt2_font_names_use_escaped_utf16_without_a_text_terminator() {
    assert_eq!(txt2::utf16(ESCAPED_TEXT), ESCAPED_UTF16);
    let block = editable_block("AB");
    let (out, index) = txt2::ensure_font(&block, ESCAPED_TEXT).unwrap();
    assert_eq!(index, 0);
    assert_eq!(txt2::font_count(&out).unwrap(), 1);
    let name_path = [
        Step::Key("0"),
        Step::Key("1"),
        Step::Key("0"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("0"),
    ];
    let mut expected_name = b"(".to_vec();
    expected_name.extend_from_slice(&ESCAPED_UTF16[..ESCAPED_UTF16.len() - 2]);
    expected_name.push(b')');
    assert_eq!(
        txt2::read_path(&out, &name_path).unwrap().unwrap(),
        expected_name
    );
    assert_eq!(
        txt2::ensure_font(&out, ESCAPED_TEXT).unwrap(),
        (out.clone(), index)
    );
    assert_eq!(txt2::read_text(&out), txt2::read_text(&block));
    assert_eq!(
        txt2::read_path(&out, STYLE_TABLE_PATH).unwrap(),
        txt2::read_path(&block, STYLE_TABLE_PATH).unwrap()
    );
    assert_eq!(
        txt2::read_path(&out, CACHE_PATH).unwrap(),
        txt2::read_path(&block, CACHE_PATH).unwrap()
    );
}
