//! EngineData parity pins measured against Photoshop 20.0.2's own saves.
//!
//! The differential harness is `examples/ed_parity.rs`: it dumps PS's stored
//! `TySh` `EngineData` and this crate's writer output for the same read spec.
//! These tests pin the four rules that harness derived (source file:
//! `out/psd_showcase/10_all_features.psd`, PS's own save; evidence table in
//! `out/verify/sync/enginedata-parity.md`).

use photocraft_text::engine_data::Value as E;
use photocraft_text::psd::{LayerSpec, ParagraphSpec, RunSpec};

fn spec(size: f32) -> LayerSpec {
    LayerSpec {
        text: "Auto".into(),
        runs: vec![RunSpec {
            len: 4,
            size: Some(size),
            auto_leading: Some(true),
            ..Default::default()
        }],
        paragraph: ParagraphSpec {
            auto_leading: 1.2,
            ..Default::default()
        },
        ..Default::default()
    }
}

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

/// PS writes the computed line height in an auto-leading run sheet
/// (30 -> 36.0, 26 -> 31.2, 22 -> 26.4) and keeps the resource default sheet
/// at 0; the read-back model stays ratio-based (`leading: None`).
#[test]
fn auto_leading_matches_photoshop_run_sheets() {
    for (size, expected) in [(30.0, 36.0), (26.0, 31.2), (22.0, 26.4)] {
        let root = photocraft_text::psd::build_engine_data(&spec(size));
        let got = style_data(&root)
            .get("Leading")
            .and_then(E::as_f32)
            .expect("leading");
        assert!(
            (got - expected).abs() < 1e-2,
            "{size} pt: {got} != {expected}"
        );
        let runs = photocraft_text::runs::from_engine_data(&root);
        assert_eq!(runs[0].auto_leading, Some(true));
        assert_eq!(runs[0].leading, None, "the model must keep the ratio form");
    }
    let root = photocraft_text::psd::build_engine_data(&spec(30.0));
    let resource = root
        .get("ResourceDict")
        .and_then(|d| d.get("StyleSheetSet"))
        .and_then(E::as_array)
        .and_then(|a| a.first())
        .and_then(|s| s.get("StyleSheetData"))
        .expect("resource sheet");
    assert_eq!(resource.get("Leading").and_then(E::as_f32), Some(0.0));
}

/// PS ends every font table with the two fallback faces and points the
/// document default style sheet at `MyriadPro-Regular`; real faces are
/// `FontType 1`, fallbacks `FontType 0`.
#[test]
fn font_table_carries_photoshop_defaults() {
    let mut with_font = spec(24.0);
    with_font.runs[0].font = Some("NotoSans-Regular".into());
    let root = photocraft_text::psd::build_engine_data(&with_font);
    for table in ["ResourceDict", "DocumentResources"] {
        let fonts = root
            .get(table)
            .and_then(|d| d.get("FontSet"))
            .and_then(E::as_array)
            .unwrap_or_else(|| panic!("{table}/FontSet"));
        let names: Vec<_> = fonts
            .iter()
            .filter_map(|f| f.get("Name").and_then(E::as_str))
            .collect();
        assert!(
            names.ends_with(&["MyriadPro-Regular", "AdobeInvisFont"]),
            "{table}: {names:?}"
        );
        assert_eq!(
            fonts[0].get("FontType").and_then(E::as_i64),
            Some(1),
            "{table} first face"
        );
        assert_eq!(
            fonts[names.len() - 1].get("FontType").and_then(E::as_i64),
            Some(0),
            "{table} fallback"
        );
        let myriad = names
            .iter()
            .position(|n| *n == "MyriadPro-Regular")
            .unwrap() as i64;
        let font = root
            .get(table)
            .and_then(|d| d.get("StyleSheetSet"))
            .and_then(E::as_array)
            .and_then(|a| a.first())
            .and_then(|s| s.get("StyleSheetData"))
            .and_then(|d| d.get("Font"))
            .and_then(E::as_i64);
        assert_eq!(font, Some(myriad), "{table} default sheet font index");
    }
}

/// PS omits OpenType feature keys that are off and writes `YUnderline` as 0
/// when underline is on (1 when off).
#[test]
fn default_valued_keys_are_omitted() {
    let mut plain = spec(20.0);
    plain.runs[0].features = vec![photocraft_text::runs::RunFeature {
        tag: "calt".into(),
        value: 0,
    }];
    let root = photocraft_text::psd::build_engine_data(&plain);
    assert!(style_data(&root).get("ContextualLigatures").is_none());
    let mut underlined = spec(20.0);
    underlined.runs[0].underline = true;
    let root = photocraft_text::psd::build_engine_data(&underlined);
    assert_eq!(
        style_data(&root).get("YUnderline").and_then(E::as_i64),
        Some(0)
    );
    assert_eq!(
        style_data(&root).get("Underline").and_then(E::as_bool),
        Some(true)
    );
}

/// Each paragraph run carries PS's default `Adjustments` block, and
/// justify-all serialises as 6.
#[test]
fn paragraph_runs_carry_photoshop_adjustments() {
    let mut spec = spec(18.0);
    spec.paragraphs = vec![ParagraphSpec {
        justification: 6,
        ..Default::default()
    }];
    let root = photocraft_text::psd::build_engine_data(&spec);
    let run = root
        .get("EngineDict")
        .and_then(|d| d.get("ParagraphRun"))
        .and_then(|d| d.get("RunArray"))
        .and_then(E::as_array)
        .and_then(|a| a.first())
        .expect("paragraph run");
    let axis = run
        .get("Adjustments")
        .and_then(|a| a.get("Axis"))
        .and_then(E::as_array)
        .expect("Axis");
    assert_eq!(axis.len(), 3);
    let xy = run
        .get("Adjustments")
        .and_then(|a| a.get("XY"))
        .and_then(E::as_array)
        .expect("XY");
    assert_eq!(xy.len(), 2);
    let justification = run
        .get("ParagraphSheet")
        .and_then(|s| s.get("Properties"))
        .and_then(|p| p.get("Justification"))
        .and_then(E::as_i64);
    assert_eq!(justification, Some(6), "justify-all is 6 in EngineData");
}
