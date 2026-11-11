//! EngineData round-trip against blobs written by Photoshop itself.

use photocraft_text::engine_data::{parse, write, Value};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/data/{name}_engine_data.txt")).unwrap()
}

#[test]
fn parses_photoshop_engine_data() {
    let data = fixture("plain_latin");
    let value = parse(&data).unwrap();
    let engine = value.get("EngineDict").expect("EngineDict");
    let text = engine
        .get("Editor")
        .and_then(|e| e.get("Text"))
        .and_then(Value::as_str)
        .expect("editor text");
    assert_eq!(text, "Hamburgefonstiv AVATAR To Ty.\r");

    let fonts = value
        .get("ResourceDict")
        .and_then(|r| r.get("FontSet"))
        .and_then(Value::as_array)
        .expect("FontSet");
    let names: Vec<&str> = fonts.iter().filter_map(|f| f.get("Name")).filter_map(Value::as_str).collect();
    assert_eq!(names, ["NotoSans-Regular", "MyriadPro-Regular", "AdobeInvisFont"]);

    let style = engine
        .get("StyleRun")
        .and_then(|s| s.get("RunArray"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|r| r.get("StyleSheet"))
        .and_then(|s| s.get("StyleSheetData"))
        .expect("style sheet data");
    assert_eq!(style.get("FontSize").and_then(Value::as_f32), Some(48.0));
    assert_eq!(style.get("Font").and_then(Value::as_i64), Some(0));
    assert_eq!(style.get("AutoKerning").and_then(Value::as_bool), Some(true));

    let para = engine
        .get("ParagraphRun")
        .and_then(|s| s.get("RunArray"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|r| r.get("ParagraphSheet"))
        .and_then(|s| s.get("Properties"))
        .expect("paragraph properties");
    let just = para.get("Justification").and_then(Value::as_i64);
    assert_eq!(just, Some(0));
}

#[test]
fn round_trips_through_write_and_parse() {
    for name in ["plain_latin", "box_left"] {
        let data = fixture(name);
        let first = parse(&data).unwrap();
        let written = write(&first);
        let second = parse(&written).unwrap();
        assert_eq!(first, second, "{name} did not survive the round trip");

        // Writing twice must be stable.
        let again = write(&second);
        assert_eq!(written, again, "{name} writer is not deterministic");
    }
}

#[test]
fn writes_the_same_shape_photoshop_writes() {
    let data = fixture("plain_latin");
    let value = parse(&data).unwrap();
    let written = write(&value);
    let has = |needle: &[u8]| written.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"\t/EngineDict\n\t<<"));
    assert!(has(b"/FontSize 48.0"));
    assert!(has(b"/Text (\xfe\xff\x00H\x00a\x00m"));
    assert!(has(b"/RunLengthArray [ 30 ]"));
}

#[test]
fn rejects_garbage() {
    assert!(parse(b"not engine data").is_err());
    assert!(parse(b"<< /Key ").is_err());
}
