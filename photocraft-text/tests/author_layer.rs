//! Multi-layer PSD authoring through `photocraft_text::psd::author_layer`: the
//! `Txt2` object each text layer owns, the `TySh` `EngineData` mirror, the stored
//! pixels, and the byte-preservation of objects the spec leaves unchanged.

use std::path::PathBuf;

use photocraft_psd::TySh;
use photocraft_text::layer_render::FaceStore;
use photocraft_text::psd::{FrameSpec, LayerSpec, ParagraphSpec, RunSpec, ShapeSpec, author_layer};
use photocraft_text::txt2;

#[test]
fn a_two_layer_point_document_writes_both_txt2_objects() {
    let mut file = fixture("plain_latin.bin");
    let first = text_layer(&file);
    let second = duplicate_text_layer(&mut file, first, 1);
    let store = store();

    let alpha = LayerSpec {
        text: "Alpha".into(),
        runs: vec![RunSpec {
            len: 6,
            font: Some("NotoSans-Bold".into()),
            size: Some(64.0),
            tracking: Some(120),
            color: Some([200, 10, 20]),
            stroke: Some([0, 100, 200]),
            bold: true,
            underline: true,
            all_caps: true,
            old_style: true,
            ligatures: Some(false),
            alternate_ligatures: Some(true),
            no_break: Some(true),
            language: Some(14),
            ..Default::default()
        }],
        paragraph: ParagraphSpec {
            justification: 2,
            first_line_indent: 10.0,
            hanging_punctuation: true,
            ..Default::default()
        },
        paragraphs: Vec::new(),
        aa: Some(1),
        frame: None,
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    let beta = LayerSpec {
        text: "Beta\rTwo".into(),
        runs: vec![
            RunSpec {
                len: 5,
                size: Some(40.0),
                ..Default::default()
            },
            RunSpec {
                len: 4,
                size: Some(24.0),
                italic: true,
                ..Default::default()
            },
        ],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: Some(3),
        frame: None,
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author_layer(&mut file, first, &alpha, &store).expect("first layer");
    author_layer(&mut file, second, &beta, &store).expect("second layer");

    let block = file.txt2().expect("Txt2").to_vec();
    // Both objects exist at their `TextIndex` slots, with the terminal CR.
    assert_eq!(txt2::object_text(&block, 0).as_deref(), Some("Alpha"));
    assert_eq!(txt2::object_text(&block, 1).as_deref(), Some("Beta\rTwo"));
    // One style run per layer, the length covering the text + terminator: the
    // `/5 1` character-run marker is what makes Photoshop read the run.
    assert_eq!(txt2::run_lengths_at(&block, 0).unwrap(), vec![6]);
    assert_eq!(txt2::run_lengths_at(&block, 1).unwrap(), vec![5, 4]);
    assert_eq!(txt2::paragraph_run_lengths_at(&block, 0).unwrap(), vec![6]);
    assert_eq!(
        txt2::paragraph_run_lengths_at(&block, 1).unwrap(),
        vec![5, 4]
    );
    for object in 0..2 {
        for run in 0..txt2::run_count_at(&block, object).unwrap() {
            let mut path = txt2::run_path_for_at(object, run);
            for key in ["0", "0", "5"] {
                path.push(txt2::Step::Key(key));
            }
            assert_eq!(
                txt2::read_path(&block, &path).unwrap().as_deref(),
                Some(b"1".as_slice()),
                "object {object} run {run}: /5 1 marker"
            );
        }
    }
    // Character properties land in the `Txt2` sheet Photoshop reads.
    let sheet = |key: &str| {
        let mut path = txt2::run_path_for_at(0, 0);
        for step in ["0", "0", "6"] {
            path.push(txt2::Step::Key(step));
        }
        path.push(txt2::Step::Key(key));
        txt2::read_path(&block, &path)
            .unwrap()
            .map(|raw| raw.to_vec())
    };
    assert_eq!(sheet("1").as_deref(), Some(b"64.0".as_slice()), "size");
    assert_eq!(sheet("8").as_deref(), Some(b"120".as_slice()), "tracking");
    assert_eq!(sheet("2").as_deref(), Some(b"true".as_slice()), "faux bold");
    assert_eq!(sheet("12").as_deref(), Some(b"2".as_slice()), "all caps");
    assert_eq!(sheet("16").as_deref(), Some(b"2".as_slice()), "underline");
    assert_eq!(sheet("30").as_deref(), Some(b"2".as_slice()), "old style");
    assert_eq!(
        sheet("18").as_deref(),
        Some(b"false".as_slice()),
        "ligatures"
    );
    assert_eq!(
        sheet("19").as_deref(),
        Some(b"true".as_slice()),
        "alternate ligatures"
    );
    assert_eq!(sheet("52").as_deref(), Some(b"true".as_slice()), "no break");
    assert_eq!(sheet("38").as_deref(), Some(b"14".as_slice()), "language");
    assert_eq!(
        sheet("11").as_deref(),
        Some(b"1".as_slice()),
        "metrics kerning"
    );
    // Both paints are `SimplePaint` dictionaries, written through the shared
    // object-aware setters (alpha first, straight RGB in 0…1).
    let fill = txt2::run_fill_color_at(&block, 0, 0).expect("fill paint");
    assert!((fill[0] - 1.0).abs() < 1e-4);
    assert!((fill[1] - 200.0 / 255.0).abs() < 1e-4);
    assert!((fill[2] - 10.0 / 255.0).abs() < 1e-4);
    assert!((fill[3] - 20.0 / 255.0).abs() < 1e-4);
    let stroke = txt2::run_stroke_color_at(&block, 0, 0).expect("stroke paint");
    assert!((stroke[1] - 0.0).abs() < 1e-4);
    assert!((stroke[2] - 100.0 / 255.0).abs() < 1e-4);
    assert!((stroke[3] - 200.0 / 255.0).abs() < 1e-4);
    assert_eq!(txt2::object_antialias(&block, 0), Some(1));
    assert_eq!(txt2::object_antialias(&block, 1), Some(3));
    // Paragraph sheets: justification and the first-line indent reach `Txt2`.
    assert_eq!(
        txt2::paragraph_justification_at(&block, 0, 0).map(|j| j.code()),
        Some(2)
    );
    assert_eq!(
        txt2::paragraph_first_line_indent_at(&block, 0, 0),
        Some(10.0)
    );
    assert_eq!(
        txt2::paragraph_hanging_punctuation_at(&block, 0, 0),
        Some(true)
    );
    // One paragraph run per paragraph on the second layer.
    assert_eq!(
        txt2::paragraph_justification_at(&block, 1, 1).map(|j| j.code()),
        Some(0)
    );

    // The `TySh` mirror says the same thing: the descriptor text and the
    // EngineData `Editor/Text` and run split.
    assert_eq!(
        file.text_layer_text(first).unwrap().unwrap(),
        "Alpha".to_string()
    );
    for (index, expected) in [(first, "Alpha\r"), (second, "Beta\rTwo\r")] {
        let engine = file.text_layer_engine_data(index).unwrap().unwrap();
        let ed = photocraft_text::engine_data::parse(&engine).unwrap();
        let editor = ed
            .get("EngineDict")
            .and_then(|d| d.get("Editor"))
            .and_then(|d| d.get("Text"))
            .and_then(|v| v.as_str());
        assert_eq!(editor, Some(expected));
        let styles = photocraft_text::runs::from_engine_data(&ed);
        assert_eq!(
            styles.iter().map(|s| s.len).collect::<Vec<_>>(),
            if index == first { vec![6] } else { vec![5, 4] }
        );
        if index == first {
            assert_eq!(styles[0].size, Some(64.0));
            assert!(styles[0].faux_bold && styles[0].underline && styles[0].all_caps);
            assert_eq!(styles[0].ligatures, Some(false));
            assert_eq!(styles[0].language, Some(14));
        } else {
            assert_eq!(styles[1].size, Some(24.0));
            assert!(styles[1].faux_italic);
        }
    }

    let read = photocraft_text::psd::read_layer(&file, first).expect("read");
    assert_eq!(read.runs[0].color, Some([200, 10, 20]));
    assert_eq!(read.runs[0].stroke, Some([0, 100, 200]));

    // Both layers were re-rasterised from that EngineData.
    for index in [first, second] {
        let layer = &file.layers()[index];
        let (width, height) = layer.rect.size().unwrap();
        assert!(width > 0 && height > 0, "layer {index} rect");
        let alpha = layer
            .channel(-1)
            .and_then(|c| {
                c.decode(width, height, file.header.depth, file.header.version)
                    .ok()
            })
            .expect("transparency channel");
        assert!(
            alpha.iter().filter(|a| **a > 0).count() > 10,
            "layer {index} has ink"
        );
    }
}

#[test]
fn a_box_layer_gets_its_frame_paragraph_sheets_and_hyphenation() {
    let mut file = fixture("box_left.bin");
    let index = text_layer(&file);
    let spec = LayerSpec {
        text: "Hamburg AVATAR wraps over several words.\rSecond paragraph.".into(),
        runs: vec![RunSpec {
            len: "Hamburg AVATAR wraps over several words.\rSecond paragraph."
                .encode_utf16()
                .count() as usize
                + 1,
            font: Some("NotoSans-Regular".into()),
            size: Some(30.0),
            auto_leading: Some(false),
            leading: Some(36.0),
            ..Default::default()
        }],
        paragraph: ParagraphSpec {
            justification: 5,
            every_line_composer: true,
            first_line_indent: 12.0,
            start_indent: 18.0,
            end_indent: 6.0,
            space_before: 4.0,
            space_after: 8.0,
            auto_leading: 1.5,
            word_spacing: [0.9, 1.0, 1.1],
            letter_spacing: [0.01, 0.02, 0.03],
            glyph_scaling: [0.95, 1.0, 1.05],
            hanging_punctuation: true,
            hyphenate: true,
            hyphenate_words_longer_than: 8,
            hyphenate_after_first: 3,
            hyphenate_before_last: 4,
            hyphenate_limit: 2,
            hyphenate_zone: 18.0,
            hyphenate_capital_words: false,
            burasagari: false,
            kinsoku_order: 0,
        },
        paragraphs: Vec::new(),
        aa: Some(0),
        frame: Some([20.0, 30.0, 420.0, 260.0]),
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let block = file.txt2().expect("Txt2").to_vec();
    let frame = txt2::object_frame_geometry(&block, 0).expect("frame");
    assert_eq!(
        (frame.left, frame.top, frame.right, frame.bottom),
        (20.0, 30.0, 420.0, 260.0)
    );
    for i in 0..2 {
        assert_eq!(
            txt2::paragraph_justification_at(&block, 0, i).map(|j| j.code()),
            Some(5)
        );
        assert_eq!(
            txt2::paragraph_every_line_composer_at(&block, 0, i),
            Some(true)
        );
        assert_eq!(txt2::paragraph_start_indent_at(&block, 0, i), Some(18.0));
        assert_eq!(txt2::paragraph_space_after_at(&block, 0, i), Some(8.0));
        assert_eq!(txt2::paragraph_auto_leading_at(&block, 0, i), Some(1.5));
        let hyphenation = txt2::paragraph_hyphenation_at(&block, 0, i).expect("hyphenation");
        assert!(hyphenation.hyphenate);
        assert_eq!(hyphenation.words_longer_than, 8);
        assert_eq!(hyphenation.after_first, 3);
        assert_eq!(hyphenation.before_last, 4);
        assert_eq!(hyphenation.zone_pt, 18.0);
        assert!(!hyphenation.capital_words);
    }
    // The descriptor's layout rectangle is the composed text extent Photoshop
    // keeps there, not the authored frame (the frame lives in `Txt2`), so it is
    // left as the template wrote it.
    let before = fixture("box_left.bin");
    let tysh = file.text_layer_tysh(index).unwrap().unwrap();
    let template = before.text_layer_tysh(index).unwrap().unwrap();
    assert_eq!(tysh.layout_bounds(), template.layout_bounds());
}

#[test]
fn an_unchanged_photoshop_object_is_preserved_byte_for_byte() {
    // The fixture's object is Photoshop-authored: text 29 units + CR, size 48,
    // one run, font NotoSans-Regular, metrics kerning. A spec that says exactly
    // that must leave the block untouched, so re-saving does not drop the keys
    // this writer does not model (glyph pen positions, cookies, …).
    let mut file = fixture("plain_latin.bin");
    let index = text_layer(&file);
    let before = file.txt2().expect("Txt2").to_vec();
    let text = txt2::object_text(&before, 0).expect("object text");
    let text = text.trim_end_matches('\r').to_string();
    let len = text.encode_utf16().count() + 1;
    let spec = LayerSpec {
        text,
        runs: vec![RunSpec {
            len,
            font: Some("NotoSans-Regular".into()),
            size: Some(48.0),
            auto_kerning: Some(true),
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
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let after = file.txt2().expect("Txt2");
    let first_diff = before.iter().zip(after).position(|(a, b)| a != b);
    assert!(
        first_diff.is_none() && before.len() == after.len(),
        "an object the spec agrees with must not be rewritten: \
         first differing byte {first_diff:?}, before {} bytes, after {} bytes",
        before.len(),
        after.len()
    );
}

#[test]
fn a_new_layer_without_a_txt2_block_gets_the_photoshop_scaffolding() {
    // A document that never had a `Txt2` block (PhotoCraft-native) gets
    // Photoshop's scaffolding, so the block it saves is one Photoshop opens.
    let mut file = fixture("plain_latin.bin");
    let index = text_layer(&file);
    file.set_txt2(None);
    let spec = LayerSpec {
        text: "Native".into(),
        runs: vec![RunSpec {
            len: 7,
            font: Some("NotoSans-Regular".into()),
            size: Some(36.0),
            ..Default::default()
        }],
        paragraph: ParagraphSpec::default(),
        paragraphs: Vec::new(),
        aa: Some(3),
        frame: None,
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let block = file.txt2().expect("Txt2");
    assert_eq!(txt2::object_text(block, 0).as_deref(), Some("Native"));
    // The scaffolding keys Photoshop needs beside the objects.
    for path in [
        vec![txt2::Step::Key("98"), txt2::Step::Key("0")],
        vec![
            txt2::Step::Key("0"),
            txt2::Step::Key("1"),
            txt2::Step::Key("0"),
        ],
        vec![txt2::Step::Key("1"), txt2::Step::Key("0")],
    ] {
        assert!(
            txt2::read_path(block, &path).unwrap().is_some(),
            "missing {path:?}"
        );
    }
}

#[test]
fn a_negative_text_index_falls_back_to_the_first_object() {
    // A hostile `TextIndex` must not panic or address a negative slot.
    let mut file = fixture("plain_latin.bin");
    let index = text_layer(&file);
    let mut tysh = TySh::from_bytes(&file.layers()[index].block(b"TySh").unwrap().data).unwrap();
    tysh.set_text_index(-3);
    file.set_text_layer_tysh(index, &tysh).unwrap();
    let spec = LayerSpec {
        text: "Safe".into(),
        runs: vec![RunSpec {
            len: 5,
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
    author_layer(&mut file, index, &spec, &store()).expect("author");
    assert_eq!(
        txt2::object_text(file.txt2().unwrap(), 0).as_deref(),
        Some("Safe")
    );
}

fn text_layer(file: &photocraft_psd::PsdFile) -> usize {
    file.layers()
        .iter()
        .position(|l| l.block(b"TySh").is_some())
        .expect("text layer")
}

fn duplicate_text_layer(file: &mut photocraft_psd::PsdFile, from: usize, text_index: i32) -> usize {
    let mut layer = file.layers()[from].clone();
    let block = layer.block_mut(b"TySh").expect("TySh");
    let mut tysh = TySh::from_bytes(&block.data).expect("parse TySh");
    tysh.set_text_index(text_index);
    block.data = tysh.to_bytes();
    block.padding = None;
    let index = file.layers().len();
    file.layers_mut().push(layer);
    index
}

fn fixture(name: &str) -> photocraft_psd::PsdFile {
    let path = assets().join("inputs").join(name);
    photocraft_psd::PsdFile::from_bytes(&std::fs::read(path).expect("fixture")).expect("parse")
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

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

#[test]
fn author_updates_the_tysh_descriptor_text() {
    // Every `TySh`-only reader (Photoshop's Text field, the fork importer) reads
    // the descriptor's `Txt `, so an edit that only rewrote `Txt2`/`EngineData`
    // left them showing the template's old string.
    let mut file = fixture("plain_latin.bin");
    let index = text_layer(&file);
    let before = file.layers()[index]
        .block(b"TySh")
        .and_then(|b| TySh::from_bytes(&b.data).ok())
        .and_then(|t| t.text())
        .expect("template text");
    let spec = LayerSpec {
        text: "Hi\rall".into(),
        runs: vec![
            RunSpec {
                len: 2,
                font: Some("NotoSans-Bold".into()),
                size: Some(48.0),
                ..Default::default()
            },
            RunSpec {
                len: 5,
                size: Some(24.0),
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
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let tysh = file.text_layer_tysh(index).unwrap().unwrap();
    assert_eq!(tysh.text().as_deref(), Some("Hi\rall"));
    assert_ne!(tysh.text().as_deref(), Some(before.as_str()));
    // The descriptor text, the `Txt2` text and `EngineData` agree.
    let block = file.txt2().expect("Txt2");
    assert_eq!(txt2::object_text(block, 0).as_deref(), Some("Hi\rall"));
    let ed = photocraft_text::engine_data::parse(&tysh.engine_data().unwrap()).unwrap();
    assert_eq!(
        ed.get("EngineDict")
            .and_then(|d| d.get("Editor"))
            .and_then(|d| d.get("Text"))
            .and_then(|v| v.as_str()),
        Some("Hi\rall\r")
    );
    // A re-authoring of the same spec keeps the descriptor text (idempotent).
    author_layer(&mut file, index, &spec, &store()).expect("re-author");
    assert_eq!(
        file.text_layer_tysh(index)
            .unwrap()
            .unwrap()
            .text()
            .as_deref(),
        Some("Hi\rall")
    );
}

#[test]
fn per_paragraph_sheets_and_cjk_options_reach_both_copies() {
    // `LayerSpec.paragraphs` gives every paragraph its own sheet; the two
    // `EngineData`-only CJK options have no `Txt2` slot, so they must appear in
    // the paragraph run sheets Photoshop reads.
    let mut file = fixture("box_left.bin");
    let index = text_layer(&file);
    let text = "First line.\rSecond line.";
    let spec = LayerSpec {
        text: text.into(),
        runs: vec![RunSpec {
            len: text.encode_utf16().count() + 1,
            size: Some(24.0),
            ..Default::default()
        }],
        paragraph: ParagraphSpec::default(),
        paragraphs: vec![
            ParagraphSpec {
                justification: 0,
                first_line_indent: 10.0,
                burasagari: true,
                kinsoku_order: 1,
                ..Default::default()
            },
            ParagraphSpec {
                justification: 5,
                first_line_indent: 20.0,
                burasagari: false,
                kinsoku_order: 2,
                ..Default::default()
            },
        ],
        aa: None,
        frame: Some([10.0, 10.0, 300.0, 220.0]),
        shape: None,
        orientation: None,
        transform: None,
        atlas: None,
        fractional: None,
        preserve_trailing_break: false,
    };
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let block = file.txt2().expect("Txt2").to_vec();
    assert_eq!(
        txt2::paragraph_justification_at(&block, 0, 0).map(|j| j.code()),
        Some(0)
    );
    assert_eq!(
        txt2::paragraph_justification_at(&block, 0, 1).map(|j| j.code()),
        Some(5)
    );
    assert_eq!(
        txt2::paragraph_first_line_indent_at(&block, 0, 0),
        Some(10.0)
    );
    assert_eq!(
        txt2::paragraph_first_line_indent_at(&block, 0, 1),
        Some(20.0)
    );

    let ed =
        photocraft_text::engine_data::parse(&file.text_layer_engine_data(index).unwrap().unwrap())
            .expect("EngineData");
    let runs = ed
        .get("EngineDict")
        .and_then(|d| d.get("ParagraphRun"))
        .and_then(|p| p.get("RunArray"))
        .and_then(|a| a.as_array())
        .expect("paragraph runs");
    assert_eq!(runs.len(), 2);
    let sheet = |i: usize| {
        runs[i]
            .get("ParagraphSheet")
            .and_then(|s| s.get("Properties"))
            .expect("paragraph properties")
    };
    assert_eq!(
        sheet(0).get("Burasagari").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        sheet(0).get("KinsokuOrder").and_then(|v| v.as_i64()),
        Some(1)
    );
    assert_eq!(
        sheet(1).get("Burasagari").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        sheet(1).get("KinsokuOrder").and_then(|v| v.as_i64()),
        Some(2)
    );
}

#[test]
fn manual_kerning_uses_photoshop_fixed_point_encoding() {
    // `Txt2` `/6/10` is 16.16 fixed point (kern / 65536), not the 1/1000 em the
    // model and `EngineData` use: Photoshop stores manual kern 100 as `0.00153`
    // (`out/verify/mk100_res.psd`, read back as 100). Writing an integer there
    // means millions of milliem, so the round trip is asserted both ways.
    let mut file = fixture("plain_latin.bin");
    let index = text_layer(&file);
    let spec = LayerSpec {
        text: "Kern".into(),
        runs: vec![RunSpec {
            len: 5,
            size: Some(48.0),
            kerning: Some(80),
            kerning_mode: Some(0),
            // Fixed so the `Txt2` sheet already carries the slot: the point of
            // the test is the encoding, not the insertion.
            font: Some("NotoSans-Regular".into()),
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
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let block = file.txt2().expect("Txt2").to_vec();
    assert_eq!(txt2::run_manual_kerning_at(&block, 0, 0), Some(80));
    let mut path = txt2::run_path_for_at(0, 0);
    for step in ["0", "0", "6", "10"] {
        path.push(txt2::Step::Key(step));
    }
    assert_eq!(
        txt2::read_path(&block, &path).unwrap().as_deref(),
        Some(b"0.00122".as_slice()),
        "Photoshop's 16.16 fixed point, not 80"
    );
    // The reader (which the editor uses) sees the model value again.
    let read = photocraft_text::psd::read_layer(&file, index).expect("read");
    assert_eq!(read.runs[0].kerning, Some(80));
    assert_eq!(read.runs[0].kerning_mode, Some(0));
    // `EngineData` keeps the 1/1000 em integer.
    let ed =
        photocraft_text::engine_data::parse(&file.text_layer_engine_data(index).unwrap().unwrap())
            .expect("EngineData");
    let styles = photocraft_text::runs::from_engine_data(&ed);
    assert_eq!(styles[0].kerning, Some(80));
    // Re-authoring the same spec leaves the block byte-identical.
    let before = block.clone();
    author_layer(&mut file, index, &spec, &store()).expect("re-author");
    assert_eq!(file.txt2().expect("Txt2"), before.as_slice());
}

/// The four straight cubic segments Photoshop writes for a text box, eight
/// numbers per segment (`anchor, out, next-in, next-anchor`, shared anchors
/// repeated).
fn box_points([left, top, right, bottom]: [f64; 4]) -> Vec<f64> {
    let corners = [
        [left, top],
        [right, top],
        [right, bottom],
        [left, bottom],
        [left, top],
    ];
    let mut points = Vec::new();
    for pair in corners.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        for point in [a, a, b, b] {
            points.extend_from_slice(&point);
        }
    }
    points
}

#[test]
fn a_shape_frame_is_written_and_preserved() {
    // A non-rectangular frame lives in the document frame list the object's view
    // names (`/0 /8 /0 [frame] /0`); the spec's `shape.frame` carries the stored
    // points (text space) and is written through the path setter.
    let mut file = fixture("box_left.bin");
    let index = text_layer(&file);
    let points = box_points([10.0, 20.0, 410.0, 220.0]);
    let spec = LayerSpec {
        text: "Boxed".into(),
        runs: vec![RunSpec {
            len: 6,
            font: Some("NotoSans-Regular".into()),
            size: Some(30.0),
            ..Default::default()
        }],
        frame: Some([10.0, 20.0, 410.0, 220.0]),
        shape: Some(ShapeSpec {
            frame: Some(FrameSpec {
                points: points.clone(),
                kind: 1,
                range: [-2.0, -2.0],
                ..Default::default()
            }),
            path: None,
        }),
        ..Default::default()
    };
    author_layer(&mut file, index, &spec, &store()).expect("author");
    let block = file.txt2().expect("Txt2").to_vec();
    let path = txt2::object_frame_path(&block, 0).expect("frame path");
    assert_eq!(path.points, points);
    assert_eq!(path.kind, 1, "box frame kind");
    assert_eq!(path.range, [-2.0, -2.0]);
    assert_eq!(path.matrix, None, "new paths are written in text space");
    // The object's own composed frame rectangle follows the spec's box.
    let geometry = txt2::object_frame_geometry(&block, 0).expect("frame rect");
    assert_eq!(
        (geometry.left, geometry.top, geometry.right, geometry.bottom),
        (10.0, 20.0, 410.0, 220.0)
    );
    // Re-authoring the same shape writes nothing (the path setter and the object
    // agreement check both compare the stored frame).
    author_layer(&mut file, index, &spec, &store()).expect("re-author");
    assert_eq!(file.txt2().expect("Txt2"), block.as_slice());
}

#[test]
fn a_frame_change_patches_the_engine_box_bounds() {
    // Photoshop composes a text box from the TySh EngineData cookie's
    // `BoxBounds`, not from the `Txt2` frame: authoring three widths must give
    // three different cookie boxes. This is the fit-rule experiment's measured
    // failure (frames 479/480/481 re-composed identically while the cookie kept
    // 480), so the frame has to land in both copies.
    use photocraft_psd::descriptor::Value as DescriptorValue;
    for width in [479.0f32, 480.0, 481.0] {
        let mut file = fixture("box_left.bin");
        let index = text_layer(&file);
        let frame = [0.0, 0.0, width, 160.0];
        let spec = LayerSpec {
            text: "Fit".into(),
            runs: vec![RunSpec {
                len: 4,
                size: Some(16.0),
                ..Default::default()
            }],
            frame: Some(frame),
            ..Default::default()
        };
        author_layer(&mut file, index, &spec, &store()).expect("author");
        let tysh = photocraft_psd::TySh::from_bytes(
            &file.layers()[index]
                .block(b"TySh")
                .expect("TySh")
                .data,
        )
        .expect("parse TySh");
        let raw = match tysh.descriptor.descriptor.get("EngineData") {
            Some(DescriptorValue::RawData(bytes)) => bytes.clone(),
            other => panic!("EngineData: {other:?}"),
        };
        let ed = photocraft_text::engine_data::parse(&raw).expect("EngineData");
        let cookie = ed
            .get("EngineDict")
            .and_then(|e| e.get("Rendered"))
            .and_then(|e| e.get("Shapes"))
            .and_then(|e| e.get("Children"))
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.get("Cookie"))
            .and_then(|c| c.get("Photoshop"))
            .expect("Photoshop cookie");
        let bounds: Vec<f64> = cookie
            .get("BoxBounds")
            .and_then(|b| b.as_array())
            .expect("BoxBounds")
            .iter()
            .map(|v| match v {
                photocraft_text::engine_data::Value::Float(f) => *f,
                photocraft_text::engine_data::Value::Int(i) => *i as f64,
                other => panic!("BoxBounds value: {other:?}"),
            })
            .collect();
        assert_eq!(bounds, vec![0.0, 0.0, f64::from(width), 160.0]);
        // The `Txt2` frame says the same box, so both copies agree.
        let block = file.txt2().expect("Txt2").to_vec();
        let geometry = txt2::object_frame_geometry(&block, 0).expect("frame");
        assert_eq!(
            (geometry.left, geometry.top, geometry.right, geometry.bottom),
            (0.0, 0.0, width, 160.0)
        );
    }
}

#[test]
fn auto_leading_round_trips_as_a_ratio() {
    use photocraft_text::engine_data::Value as E;

    fn style_data(root: &E) -> &E {
        root.get("EngineDict")
            .and_then(|d| d.get("StyleRun"))
            .and_then(|d| d.get("RunArray"))
            .and_then(E::as_array)
            .and_then(|a| a.first())
            .and_then(|r| r.get("StyleSheet"))
            .and_then(|d| d.get("StyleSheetData"))
            .expect("style data")
    }

    let spec = |leading: Option<f32>, auto: Option<bool>| LayerSpec {
        text: "Auto".into(),
        runs: vec![RunSpec {
            len: 4,
            size: Some(30.0),
            leading,
            auto_leading: auto,
            ..Default::default()
        }],
        paragraph: ParagraphSpec { auto_leading: 1.2, ..Default::default() },
        ..Default::default()
    };

    // Auto leading on: Photoshop's own form - `AutoLeading` true, the absolute
    // `Leading` neutral 0 - and the ratio lives in the paragraph sheet.
    let root = photocraft_text::psd::build_engine_data(&spec(None, Some(true)));
    let styles = style_data(&root);
    assert_eq!(styles.get("AutoLeading").and_then(E::as_bool), Some(true));
    assert_eq!(styles.get("Leading").and_then(E::as_f32), Some(0.0));
    let para = root
        .get("EngineDict")
        .and_then(|d| d.get("ParagraphRun"))
        .and_then(|d| d.get("RunArray"))
        .and_then(E::as_array)
        .and_then(|a| a.first())
        .and_then(|r| r.get("ParagraphSheet"))
        .and_then(|d| d.get("Properties"))
        .expect("paragraph properties");
    assert_eq!(para.get("AutoLeading").and_then(E::as_f32), Some(1.2));
    // A reader must not take the neutral absolute value as the model's leading.
    let runs = photocraft_text::runs::from_engine_data(&root);
    assert_eq!(runs[0].auto_leading, Some(true));
    assert_eq!(runs[0].leading, None);

    // Auto leading off: the absolute value is written and read back, and a
    // hostile spec that sets both keeps the auto form.
    let root = photocraft_text::psd::build_engine_data(&spec(Some(36.0), Some(false)));
    let runs = photocraft_text::runs::from_engine_data(&root);
    assert_eq!(runs[0].auto_leading, Some(false));
    assert_eq!(runs[0].leading, Some(36.0));
    let root = photocraft_text::psd::build_engine_data(&spec(Some(36.0), Some(true)));
    let runs = photocraft_text::runs::from_engine_data(&root);
    assert_eq!(runs[0].auto_leading, Some(true));
    assert_eq!(runs[0].leading, None, "auto owns the field when both are set");
}
