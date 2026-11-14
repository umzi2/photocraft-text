//! The PSD authoring path the live editor's export uses: text, run split,
//! EngineData mirror, and prerendered pixels all from the same spec.

use std::path::PathBuf;

use photocraft_text::layer_render::FaceStore;
use photocraft_text::psd::{Error, LayerSpec, ParagraphSpec, RunSpec, author, refresh_text_layer};

#[test]
fn flags_reach_engine_data() {
    let mut file =
        photocraft_psd::PsdFile::from_bytes(&std::fs::read(template()).unwrap()).unwrap();
    let spec = LayerSpec {
        text: "Sett".into(),
        runs: vec![RunSpec {
            len: 5,
            bold: true,
            underline: true,
            all_caps: true,
            ..Default::default()
        }],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: None,
        frame: None,
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).unwrap();
    let tysh = file
        .layers()
        .iter()
        .find_map(|l| l.block(b"TySh"))
        .unwrap()
        .data
        .clone();
    let (engine, _) =
        photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&tysh[52..]).unwrap();
    let Some(photocraft_psd::descriptor::Value::RawData(raw)) = engine.descriptor.get("EngineData")
    else {
        panic!()
    };
    let ed = photocraft_text::engine_data::parse(raw).unwrap();
    let styles = photocraft_text::runs::from_engine_data(&ed);
    assert_eq!(styles.len(), 1);
    assert!(
        styles[0].faux_bold && styles[0].underline && styles[0].all_caps,
        "{:?}",
        styles[0]
    );
}

fn template() -> PathBuf {
    assets().join("inputs/plain_latin.bin")
}

/// Area text needs a document whose `Txt2` carries a frame rectangle; a point
/// document has none, so the frame cannot be added to it.
fn area_template() -> PathBuf {
    assets().join("inputs/box_left.bin")
}

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    for name in ["NotoSans-Regular", "NotoSans-Bold"] {
        store
            .load(&assets().join("fonts"), name)
            .expect("fixture font");
    }
    store
}

#[test]
fn export_writes_text_runs_paragraphs_and_pixels() {
    let mut file =
        photocraft_psd::PsdFile::from_bytes(&std::fs::read(template()).expect("corpus psd"))
            .expect("parse");
    let spec = LayerSpec {
        text: "Hi\nall".into(),
        runs: vec![
            RunSpec {
                len: 2,
                font: Some("NotoSans-Bold".into()),
                size: Some(72.0),
                color: Some([255, 0, 0]),
                ..Default::default()
            },
            RunSpec {
                len: 5,
                size: Some(32.0),
                ..Default::default()
            },
        ],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: None,
        frame: None,
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");

    let txt2 = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .expect("Txt2")
        .data
        .clone();
    // LF input is stored as the U+000D paragraph break a PSD uses.
    let text = photocraft_text::txt2::read_text(&txt2).expect("text");
    assert_eq!(text, "Hi\rall");
    assert_eq!(
        photocraft_text::txt2::paragraph_run_lengths(&txt2).expect("paragraph runs"),
        vec![3, 4]
    );

    // TySh EngineData carries the same split for readers that ignore Txt2.
    let tysh = file
        .layers()
        .iter()
        .find_map(|l| l.block(b"TySh"))
        .expect("TySh block")
        .data
        .clone();
    let (engine, _) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&tysh[52..])
        .expect("TySh descriptor");
    let Some(photocraft_psd::descriptor::Value::RawData(raw)) = engine.descriptor.get("EngineData")
    else {
        panic!("no EngineData")
    };
    let ed = photocraft_text::engine_data::parse(raw).expect("parse EngineData");
    let styles = photocraft_text::runs::from_engine_data(&ed);
    assert_eq!(
        styles.iter().map(|s| s.len).collect::<Vec<_>>(),
        vec![2, 5],
        "EngineData run lengths"
    );
    assert_eq!(styles[0].size, Some(72.0));
    assert_eq!(styles[1].size, Some(32.0));
    let editor_text = ed
        .get("EngineDict")
        .and_then(|d| d.get("Editor"))
        .and_then(|d| d.get("Text"));
    assert_eq!(
        editor_text.and_then(|v| match v {
            photocraft_text::engine_data::Value::Str(s) => Some(s.clone()),
            _ => None,
        }),
        Some("Hi\rall\r".to_string())
    );

    // The prerendered layer was re-rasterised: two lines, the first taller.
    let layer = file
        .layers()
        .iter()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .expect("text layer");
    let (width, height) = layer.rect.size().expect("layer rect");
    let version = file.header.version;
    assert!(width > 0 && height > 0, "layer rect {width}x{height}");
    let alpha = layer
        .channel(-1)
        .and_then(|c| c.decode(width, height, file.header.depth, version).ok())
        .expect("transparency channel");
    assert!(
        alpha.iter().filter(|a| **a > 0).count() > 20,
        "layer has ink"
    );
    let rows_with_ink = alpha
        .chunks(width)
        .filter(|r| r.iter().any(|a| *a > 0))
        .count();
    assert!(
        rows_with_ink > 10,
        "two lines of ink, got {rows_with_ink} rows"
    );
}

#[test]
fn area_frame_and_paragraph_leading_reach_txt2() {
    let mut file =
        photocraft_psd::PsdFile::from_bytes(&std::fs::read(area_template()).unwrap()).unwrap();
    let spec = LayerSpec {
        text: "Hamburg\rAVATAR".into(),
        runs: vec![RunSpec {
            len: 15,
            size: Some(48.0),
            leading: Some(40.0),
            ..Default::default()
        }],
        paragraph: ParagraphSpec {
            justification: 2,
            auto_leading: 1.5,
            first_line_indent: 12.0,
            ..Default::default()
        },
        paragraphs: Vec::new(),
        aa: Some(4),
        frame: Some([20.0, 20.0, 420.0, 220.0]),
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author(&mut file, &spec, &store()).expect("author");
    let txt2 = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .expect("Txt2")
        .data
        .clone();
    let frame = photocraft_text::txt2::frame_geometry(&txt2).expect("frame geometry");
    assert_eq!(
        (frame.left, frame.top, frame.right, frame.bottom),
        (20.0, 20.0, 420.0, 220.0)
    );

    let tysh = file
        .layers()
        .iter()
        .find_map(|l| l.block(b"TySh"))
        .unwrap()
        .data
        .clone();
    let (engine, _) =
        photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&tysh[52..]).unwrap();
    let Some(photocraft_psd::descriptor::Value::RawData(raw)) = engine.descriptor.get("EngineData")
    else {
        panic!("no EngineData")
    };
    let ed = photocraft_text::engine_data::parse(raw).unwrap();
    let styles = photocraft_text::runs::from_engine_data(&ed);
    assert_eq!(styles[0].leading, Some(40.0));
    assert_eq!(styles[0].auto_leading_flag(), Some(false));
    let runs = photocraft_text::runs::paragraphs_from_engine_data(&ed);
    assert_eq!(
        runs[0].auto_leading, 1.5,
        "paragraph AutoLeading reaches EngineData"
    );
    assert_eq!(runs[0].first_line_indent, 12.0);

    // The prerendered pixels come from that same frame: a centred line cannot
    // start at the frame's left edge.
    let layer = file
        .layers()
        .iter()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .expect("text layer");
    assert!(
        layer.rect.left > 100,
        "centred ink starts at {}",
        layer.rect.left
    );
}

#[test]
fn invalid_runs_leave_the_psd_unchanged() {
    let input = std::fs::read(template()).expect("fixture PSD");
    for (lengths, expected) in [
        (vec![], "empty"),
        (vec![0, 5], "zero"),
        (vec![usize::MAX, 1], "overflow"),
        (vec![i64::MAX as usize, i64::MAX as usize, 2], "overflow"),
        (vec![2, 3], "surrogate"),
        (vec![4], "coverage"),
    ] {
        let mut file = photocraft_psd::PsdFile::from_bytes(&input).unwrap();
        let before = file.to_bytes().unwrap();
        let spec = LayerSpec {
            text: "A😀B".into(),
            runs: lengths
                .into_iter()
                .map(|len| RunSpec {
                    len,
                    ..Default::default()
                })
                .collect(),
            paragraph: Default::default(),
            paragraphs: Vec::new(),
            aa: None,
            frame: None,
            shape: None,
            orientation: None,
            transform: None,
            atlas: None,
            fractional: None,
            preserve_trailing_break: false,
        };
        let error = author(&mut file, &spec, &FaceStore::new()).unwrap_err();
        match expected {
            "empty" => assert!(matches!(error, Error::NoRuns)),
            "zero" => assert!(matches!(error, Error::ZeroLengthRun { index: 0 })),
            "overflow" => assert!(matches!(error, Error::RunLengthOverflow)),
            "surrogate" => assert!(matches!(
                error,
                Error::SurrogateSplit {
                    index: 0,
                    offset: 2
                }
            )),
            "coverage" => assert!(matches!(
                error,
                Error::RunCoverage {
                    covered: 4,
                    expected: 5
                }
            )),
            _ => unreachable!(),
        }
        assert_eq!(file.to_bytes().unwrap(), before);
    }
}

#[test]
fn malformed_tysh_does_not_commit_txt2_edits() {
    let input = std::fs::read(template()).expect("fixture PSD");
    let mut file = photocraft_psd::PsdFile::from_bytes(&input).unwrap();
    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.block(b"TySh").is_some())
        .unwrap();
    layer
        .blocks
        .iter_mut()
        .find(|b| &b.key == b"TySh")
        .unwrap()
        .data
        .clear();
    let before = file.to_bytes().unwrap();
    let spec = LayerSpec {
        text: "Hi".into(),
        runs: vec![RunSpec {
            len: 3,
            ..Default::default()
        }],
        paragraph: Default::default(),
        paragraphs: Vec::new(),
        aa: None,
        frame: None,
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    assert!(matches!(
        author(&mut file, &spec, &FaceStore::new()),
        Err(Error::TruncatedTySh { .. })
    ));
    assert!(matches!(
        refresh_text_layer(&mut file, "Hi", &FaceStore::new()),
        Err(Error::TruncatedTySh { .. })
    ));
    assert_eq!(file.to_bytes().unwrap(), before);
}

#[test]
fn unavailable_named_font_does_not_commit_authoring() {
    let input = std::fs::read(template()).expect("fixture PSD");
    let mut file = photocraft_psd::PsdFile::from_bytes(&input).unwrap();
    let before = file.to_bytes().unwrap();
    let spec: LayerSpec = serde_json::from_str(
        r#"{"text":"Hi","runs":[{"len":3,"font":"UnavailableFont-Regular"}]}"#,
    )
    .unwrap();
    let error = author(&mut file, &spec, &FaceStore::new()).unwrap_err();
    assert!(matches!(
        error,
        Error::Text(photocraft_text::Error::BadFont(_))
    ));
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(file.to_bytes().unwrap(), before);
}

#[test]
fn pixel_encoding_errors_leave_authoring_and_refresh_unchanged() {
    let input = std::fs::read(template()).expect("fixture PSD");
    let mut file = photocraft_psd::PsdFile::from_bytes(&input).unwrap();
    // RGBA is 8-bit; a 16-bit plane requires twice as many bytes.
    file.header.depth = 16;
    let before = file.to_bytes().unwrap();
    let fonts = store();
    assert!(matches!(
        refresh_text_layer(&mut file, "Hi", &fonts),
        Err(Error::Psd(_))
    ));
    assert_eq!(file.to_bytes().unwrap(), before);
    let spec: LayerSpec =
        serde_json::from_str(r#"{"text":"Hi","runs":[{"len":3,"font":"NotoSans-Regular"}]}"#)
            .unwrap();
    assert!(matches!(
        author(&mut file, &spec, &fonts),
        Err(Error::Psd(_))
    ));
    assert_eq!(file.to_bytes().unwrap(), before);
}
