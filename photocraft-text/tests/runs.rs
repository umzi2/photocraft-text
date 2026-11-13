use std::path::PathBuf;

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};
use photocraft_text::engine_data::{self, Value};
use photocraft_text::font::Face;
use photocraft_text::layout::{Kerning, TextStyle};
use photocraft_text::runs::{self, FontSet, RunStyle};

fn font_bytes(name: &str) -> Vec<u8> {
    let home = std::env::var("HOME").expect("HOME");
    let path: PathBuf = [
        &home,
        ".local/share/photocraft/wine/drive_c/windows/Fonts",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn tysh_engine_data(psd: &str) -> Vec<u8> {
    let bytes = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../corpus")
            .join(psd),
    )
    .unwrap();
    let file = PsdFile::from_bytes(&bytes).unwrap();
    let layer = file
        .layers()
        .iter()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .expect("TySh layer");
    let block = layer.blocks.iter().find(|b| &b.key == b"TySh").unwrap();
    let data = &block.data;
    let (desc, _) = VersionedDescriptor::parse_prefix(&data[52..]).unwrap();
    match desc.descriptor.get("EngineData").cloned() {
        Some(DescValue::RawData(raw)) => raw,
        other => panic!("EngineData missing or not raw: {other:?}"),
    }
}

fn set_style_run(doc: &mut Value, style_run: Value) {
    if let Value::Dict(root) = doc {
        let engine = root
            .iter_mut()
            .find(|(k, _)| k == "EngineDict")
            .expect("EngineDict");
        if let Value::Dict(entries) = &mut engine.1 {
            let slot = entries
                .iter_mut()
                .find(|(k, _)| k == "StyleRun")
                .expect("StyleRun");
            slot.1 = style_run;
        }
    }
}

/// "LAVo" split into runs [L][AV][o]. Expected values from NotoSans-Regular
/// hmtx (upm 1000): L=524, A=639, V=600, o=605. Derivation:
///   L  size 8:  adv 524*0.008 = 4.192                          x=0
///   A  size 12: adv 639*0.012 = 7.668; manual kern -50/1000*12 = -0.6
///              A.adv = 7.668 - 0.6 = 7.068 (pen delta to V)     x=4.192
///   V  size 12: adv 600*0.012 = 7.2                             x=4.192+7.068=11.26
///   o  size 24, hscale 0.5, tracking 100: 605*0.024*0.5 + 100/1000*24*0.5
///              = 7.26 + 1.2 = 8.46                              x=11.26+7.2=18.46
///   AV baseline shift 3.0 -> y=3.0; others y=0.
#[test]
fn three_runs_hand_computed_positions() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let face = Face::parse(&data).unwrap();
    let fonts = FontSet::new("NotoSans-Regular", face);
    let runs = vec![
        RunStyle {
            len: 1,
            size: Some(8.0),
            color: Some([255, 0, 0]),
            ..Default::default()
        },
        RunStyle {
            len: 2,
            size: Some(12.0),
            color: Some([0, 255, 0]),
            auto_kerning: Some(false),
            kerning: Some(-50),
            baseline_shift: Some(3.0),
            ..Default::default()
        },
        RunStyle {
            len: 1,
            size: Some(24.0),
            color: Some([0, 0, 255]),
            tracking: Some(100),
            horizontal_scale: Some(0.5),
            ..Default::default()
        },
    ];
    let base = TextStyle::default();
    let shaped = runs::shape_runs_detailed(&fonts, "LAVo", &runs, &base);
    assert_eq!(shaped.len(), 4);

    let expect = [
        (0.0, 4.192, 0.0, 0),
        (4.192, 7.068, 3.0, 1),
        (11.26, 7.2, 3.0, 1),
        (18.46, 8.46, 0.0, 2),
    ];
    for (g, (x, adv, y, run)) in shaped.iter().zip(expect) {
        assert!(close(g.glyph.x, x), "x {} != {x}", g.glyph.x);
        assert!(
            close(g.glyph.advance, adv),
            "advance {} != {adv}",
            g.glyph.advance
        );
        assert!(close(g.glyph.y, y), "y {} != {y}", g.glyph.y);
        assert_eq!(g.run, run);
    }
    assert_eq!(shaped[0].color, Some([255, 0, 0]));
    assert_eq!(shaped[1].color, Some([0, 255, 0]));
    assert_eq!(shaped[3].color, Some([0, 0, 255]));

    let plain = runs::shape_runs(&fonts, "LAVo", &runs, &base);
    assert_eq!(plain.len(), 4);
    assert!(close(plain[3].x, 18.46));
}

#[test]
fn manual_kerning_moves_second_glyph_by_value_times_size() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans-Regular", Face::parse(&data).unwrap());
    let base = TextStyle {
        kerning: Kerning::Metrics,
        ..Default::default()
    };
    let run = |kern: i16| {
        vec![RunStyle {
            len: 2,
            size: Some(12.0),
            auto_kerning: Some(false),
            kerning: Some(kern),
            ..Default::default()
        }]
    };
    let manual = runs::shape_runs(&fonts, "AV", &run(-50), &base);
    let zero = runs::shape_runs(&fonts, "AV", &run(0), &base);
    // -50/1000 * 12 = -0.6
    assert!(
        close(manual[1].x - zero[1].x, -0.6),
        "manual kern delta {}",
        manual[1].x - zero[1].x
    );
}

#[test]
fn from_engine_data_reads_plain_latin_run() {
    let raw = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/plain_latin_engine_data.txt"),
    )
    .unwrap();
    let root = engine_data::parse(&raw).unwrap();
    let style = runs::from_engine_data(&root);
    assert_eq!(style.len(), 1);
    let run = &style[0];
    assert_eq!(run.len, 30);
    assert_eq!(run.font.as_deref(), Some("NotoSans-Regular"));
    assert_eq!(run.size, Some(48.0));
    assert_eq!(run.color, Some([0, 0, 0]));
    assert_eq!(run.auto_kerning, Some(true));
    let paras = runs::paragraphs_from_engine_data(&root);
    assert_eq!(paras.len(), 1);
    assert_eq!(paras[0].len, 30);
    assert!(close(paras[0].auto_leading, 1.2));
}

/// engine_data::write does not reproduce Photoshop's whitespace/array layout
/// byte-for-byte (raw is 8745 B, write gives 9100 B). The codec lives outside
/// this crate's runs module, so these tests assert equality of parsed trees.
#[test]
fn engine_data_codec_is_semantic_on_corpus() {
    let raw = tysh_engine_data("plain_latin.psd");
    let parsed = engine_data::parse(&raw).unwrap();
    let reparsed = engine_data::parse(&engine_data::write(&parsed)).unwrap();
    assert!(reparsed == parsed, "parse(write(parse(raw))) != parse(raw)");
}

#[test]
fn corpus_style_runs_round_trip_semantic() {
    let raw = tysh_engine_data("plain_latin.psd");
    let original = engine_data::parse(&raw).unwrap();
    let mut doc = original.clone();
    let style = runs::from_engine_data(&doc);
    set_style_run(&mut doc, runs::to_engine_data(&style));
    assert!(
        doc == original,
        "StyleRun rebuilt from runs differs from parsed StyleRun"
    );
    assert!(engine_data::parse(&engine_data::write(&doc)).unwrap() == original);
}
