//! Recorded from the pre-refactor implementation; these are compatibility,
//! not Photoshop-correctness, expectations. All fixture files are mandatory.
//! Source baseline: repository commit c900bdd, captured before source changes.

use std::path::{Path, PathBuf};
use std::time::Instant;

use photocraft_psd::PsdFile;
use photocraft_text::area::{self, AreaLayout};
use photocraft_text::atlas::Atlas;
use photocraft_text::engine_data::{self, Value};
use photocraft_text::layer_render::{
    self, FaceStore, LayerPixels, LayerTextStyle, RectTarget, RenderOptions, RunFeatures,
};
use photocraft_text::layout::{Kerning, TextStyle};
use photocraft_text::psd::{LayerSpec, author};
use photocraft_text::runs::{self, FontSet, RunStyle};
use photocraft_text::warp::{FlatGeometry, Style, Warp};

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    template: String,
    spec: LayerSpec,
}

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn check_environment() {
    for (key, _) in std::env::vars_os() {
        assert!(
            !key.to_string_lossy().starts_with("PHOTOCRAFT_"),
            "baseline requires no PHOTOCRAFT_* overrides: {key:?}"
        );
    }
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("assets/cases.json")).expect("compatibility input cases")
}

fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|c| c.name == name)
        .expect("named case")
}

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    for name in ["NotoSans-Regular", "NotoSans-Bold"] {
        store
            .load(&assets().join("fonts"), name)
            .expect("checked-in font");
    }
    store
}

fn text(case: &Case) -> String {
    case.spec
        .text
        .replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn box_size(case: &Case) -> (f32, f32) {
    case.spec
        .frame
        .map(|[l, t, r, b]| (r - l, b - t))
        .unwrap_or((0.0, 0.0))
}

fn model(case: &Case) -> Value {
    let raw = std::fs::read(
        assets()
            .join("models")
            .join(format!("{}.engine.bin", case.name)),
    )
    .expect("checked-in EngineData input");
    engine_data::parse(&raw).expect("EngineData input")
}

fn golden(path: PathBuf, actual: &[u8]) {
    let expected = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("required baseline {}: {e}", path.display()));
    let difference = expected.iter().zip(actual).position(|(a, b)| a != b);
    assert!(
        difference.is_none() && expected.len() == actual.len(),
        "{}: first differing byte {:?}, expected {} bytes, got {}",
        path.display(),
        difference.or_else(
            || (expected.len() != actual.len()).then_some(expected.len().min(actual.len()))
        ),
        expected.len(),
        actual.len()
    );
}

fn check_bytes(name: &str, actual: &[u8]) {
    golden(assets().join("goldens").join(name), actual);
}

fn check_pixels(name: &str, pixels: &LayerPixels) {
    assert_eq!(
        pixels.rgba.len(),
        pixels.width as usize * pixels.height as usize * 4
    );
    assert!(
        pixels.rgba.chunks_exact(4).any(|p| p[3] != 0),
        "{name}: no ink"
    );
    let mut geometry = Vec::new();
    // Width, height, offset.x bits, offset.y bits: little-endian u32 words.
    for word in [
        pixels.width,
        pixels.height,
        pixels.offset.0.to_bits(),
        pixels.offset.1.to_bits(),
    ] {
        geometry.extend_from_slice(&word.to_le_bytes());
    }
    check_bytes(&format!("{name}.pixels.bin"), &geometry);
    check_bytes(&format!("{name}.rgba.bin"), &pixels.rgba);
}

fn engine_data_of(file: &PsdFile) -> Value {
    let tysh = file
        .layers()
        .iter()
        .find_map(|l| l.block(b"TySh"))
        .expect("TySh");
    let (desc, _) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&tysh.data[52..])
        .expect("TySh descriptor");
    let Some(photocraft_psd::descriptor::Value::RawData(raw)) = desc.descriptor.get("EngineData")
    else {
        panic!("TySh EngineData missing")
    };
    engine_data::parse(raw).expect("authored EngineData")
}

fn check_author(case: &Case) {
    let store = store();
    let bytes = std::fs::read(assets().join("inputs").join(&case.template)).expect("PSD input");
    let mut file = PsdFile::from_bytes(&bytes).expect("PSD input parse");
    author(&mut file, &case.spec, &store).expect("author original PSD");
    check_bytes(
        &format!("{}.psd.bin", case.name),
        &file.to_bytes().expect("serialize entire PSD"),
    );
    golden(
        assets()
            .join("models")
            .join(format!("{}.engine.bin", case.name)),
        &engine_data::write(&engine_data_of(&file)),
    );
}

fn style_of(runs: &[RunStyle]) -> TextStyle {
    let run = &runs[0];
    TextStyle {
        size: run.size.unwrap_or(12.0),
        tracking: run.tracking.unwrap_or(0),
        horizontal_scale: run.horizontal_scale.unwrap_or(1.0),
        baseline_shift: run.baseline_shift.unwrap_or(0.0),
        leading: if run.auto_leading == Some(true) {
            None
        } else {
            run.leading
        },
        ..Default::default()
    }
}

fn layout(case: &Case, features: &[RunFeatures]) -> AreaLayout {
    let store = store();
    let mut faces = FontSet::new(
        "NotoSans-Regular",
        store.face("NotoSans-Regular").expect("regular face"),
    );
    faces.insert(
        "NotoSans-Bold",
        store.face("NotoSans-Bold").expect("bold face"),
    );
    let ed = model(case);
    let mut runs = runs::from_engine_data(&ed);
    for (run, feature) in runs.iter_mut().zip(features) {
        run.old_style |= feature.old_style;
    }
    let mut style = style_of(&runs);
    if features.iter().any(|f| f.optical_kerning) {
        style.kerning = Kerning::Optical;
        for run in &mut runs {
            run.auto_kerning = None;
            run.kerning = None;
        }
    }
    let paras = layer_render::paragraph_styles_of(&runs::paragraphs_from_engine_data(&ed));
    let ascender = area::first_baseline_ratio(
        faces
            .get(runs[0].font.as_deref().expect("explicit run font"))
            .expect("primary face"),
    );
    let advances: Vec<f32> = features
        .iter()
        .flat_map(|f| f.advance_deltas.iter().copied())
        .collect();
    let size = box_size(case);
    if size.0 > 0.0 && size.1 > 0.0 {
        area::layout_area_runs_paragraphs_with_advances(
            &faces,
            &text(case),
            &runs,
            &style,
            &paras,
            size.0,
            size.1,
            (0.25, 1.5),
            ascender,
            &advances,
        )
    } else {
        area::layout_area_runs_paragraphs_point(
            &faces,
            &text(case),
            &runs,
            &style,
            &paras,
            1.0e7,
            1.0e7,
            (0.25, 1.5),
            ascender,
            &advances,
        )
    }
}

fn layout_bytes(layout: &AreaLayout) -> Vec<u8> {
    let mut bytes = Vec::new();
    let word = |bytes: &mut Vec<u8>, value: u32| bytes.extend_from_slice(&value.to_le_bytes());
    word(
        &mut bytes,
        layout.lines.len().try_into().expect("line count"),
    );
    for line in &layout.lines {
        word(&mut bytes, line.baseline.to_bits());
        word(&mut bytes, line.width.to_bits());
        word(
            &mut bytes,
            line.glyphs.len().try_into().expect("glyph count"),
        );
        assert_eq!(
            line.glyphs.len(),
            line.attrs.len(),
            "parallel glyph attributes"
        );
        for (glyph, attrs) in line.glyphs.iter().zip(&line.attrs) {
            for value in [
                u32::from(glyph.gid),
                glyph.ch as u32,
                glyph.x.to_bits(),
                glyph.y.to_bits(),
                glyph.advance.to_bits(),
                glyph.nominal_advance.to_bits(),
                attrs.run.try_into().expect("run index"),
            ] {
                word(&mut bytes, value);
            }
            bytes.extend_from_slice(&attrs.color);
            for value in [
                attrs.vertical_scale.to_bits(),
                attrs.x_scale.to_bits(),
                u32::from(attrs.faux_italic),
                attrs.leading.to_bits(),
            ] {
                word(&mut bytes, value);
            }
        }
    }
    bytes
}

fn check_case_render(case: &Case) {
    let store = store();
    let ed = model(case);
    let pixels = layer_render::render_from_engine_data(
        &store,
        &ed,
        &text(case),
        box_size(case),
        (0.25, 1.5),
    )
    .expect("EngineData render");
    check_pixels(&case.name, &pixels);
    check_bytes(
        &format!("{}.layout.bin", case.name),
        &layout_bytes(&layout(case, &[])),
    );
    let runs = runs::from_engine_data(&ed);
    let paragraphs = layer_render::paragraph_styles_of(&runs::paragraphs_from_engine_data(&ed));
    let spec = LayerTextStyle {
        style: style_of(&runs),
        paragraph: paragraphs[0],
        box_size: box_size(case),
        origin: (0.25, 1.5),
        color: [17, 33, 65],
    };
    let pixels = store
        .render_spec_with(&text(case), &runs, &spec, RenderOptions::default())
        .expect("direct specification render");
    check_pixels(&format!("{}_spec", case.name), &pixels);
}

fn target() -> RectTarget {
    RectTarget {
        origin: (-3, -5),
        size: (160, 100),
        anchor: (10.25, 40.5),
    }
}

fn check_fixed_and_vertical() {
    let case = case("point_styles");
    let ed = model(&case);
    let store = store();
    let pixels = layer_render::render_from_engine_data_in_rect(
        &store,
        &ed,
        &text(&case),
        &[],
        (0.0, 0.0),
        (0.25, 1.5),
        target(),
    )
    .expect("fixed rectangle render");
    check_pixels("fixed_rect", &pixels);
    let pixels =
        layer_render::render_vertical_from_engine_data(&store, &ed, &text(&case), (0.25, 1.5))
            .expect("vertical render");
    check_pixels("vertical", &pixels);
    let pixels = layer_render::render_vertical_from_engine_data_in_rect(
        &store,
        &ed,
        &text(&case),
        (0.25, 1.5),
        target(),
    )
    .expect("vertical fixed rectangle render");
    check_pixels("vertical_fixed_rect", &pixels);
}

fn check_features() {
    let case = case("point_plain");
    let ed = model(&case);
    let store = store();
    for (name, features) in [
        (
            "features_optical",
            vec![RunFeatures {
                optical_kerning: true,
                old_style: true,
                ..Default::default()
            }],
        ),
        (
            "features_advances",
            vec![RunFeatures {
                advance_deltas: vec![0.25, -0.125, 0.5, -0.75, 0.0625, 0.0, 0.375],
                ..Default::default()
            }],
        ),
    ] {
        let pixels = layer_render::render_from_engine_data_with_paragraphs_and_features(
            &store,
            &ed,
            &text(&case),
            &[],
            &features,
            (0.0, 0.0),
            (0.25, 1.5),
        )
        .expect("character features render");
        check_pixels(name, &pixels);
        check_bytes(
            &format!("{name}.layout.bin"),
            &layout_bytes(&layout(&case, &features)),
        );
        let pixels = layer_render::render_from_engine_data_in_rect_with_features(
            &store,
            &ed,
            &text(&case),
            &[],
            &features,
            (0.0, 0.0),
            (0.25, 1.5),
            target(),
        )
        .expect("fixed feature render");
        check_pixels(&format!("{name}_fixed_rect"), &pixels);
    }
}

fn check_warps() {
    let case = case("point_plain");
    let mut ed = model(&case);
    let mut run = runs::from_engine_data(&ed).remove(0);
    run.len = 4;
    run.size = Some(24.0);
    assert!(runs::apply_style_runs(&mut ed, &[run]));
    let store = store();
    for (name, style) in [("warp_arc", Style::Arc), ("warp_arch", Style::Arch)] {
        let out = layer_render::render_from_engine_data_warped(
            &store,
            &ed,
            "Warp",
            &[],
            &[],
            (0.0, 0.0),
            &Warp {
                style,
                value: 50.0,
                ..Default::default()
            },
            FlatGeometry {
                bounds: (10.0, 20.0, 110.0, 44.0),
                baseline: 40.0,
            },
            RectTarget {
                origin: (0, 0),
                size: (120, 60),
                anchor: (10.0, 40.0),
            },
        )
        .expect("warped render");
        check_pixels(name, &out.pixels);
        let mut rect = Vec::new();
        for value in [out.rect.0 as u32, out.rect.1 as u32, out.rect.2, out.rect.3] {
            rect.extend_from_slice(&value.to_le_bytes());
        }
        check_bytes(&format!("{name}.rect.bin"), &rect);
    }
}

fn atlas(phase: f32, varying: bool) -> Atlas {
    let mut atlas = Atlas::load(&assets().join(if varying { "atlas/vary" } else { "atlas" }))
        .expect("checked-in atlas");
    atlas.phase = phase;
    atlas
}

fn atlas_spec() -> (Vec<RunStyle>, LayerTextStyle) {
    let runs = vec![RunStyle {
        len: 22,
        font: Some("NotoSans-Regular".into()),
        size: Some(48.0),
        tracking: Some(23),
        color: Some([35, 90, 180]),
        ..Default::default()
    }];
    let spec = LayerTextStyle {
        style: TextStyle {
            size: 48.0,
            ..Default::default()
        },
        origin: (0.25, 0.5),
        ..Default::default()
    };
    (runs, spec)
}

fn check_atlases() {
    let (runs, spec) = atlas_spec();
    for (name, atlases) in [
        ("atlas_single", vec![atlas(0.0, false)]),
        (
            "atlas_invariant",
            vec![
                atlas(0.5, false),
                atlas(0.0, false),
                atlas(0.25, false),
                atlas(0.75, false),
            ],
        ),
        (
            "atlas_invariant_no_zero",
            vec![atlas(0.25, false), atlas(0.75, false)],
        ),
        (
            "atlas_phases",
            vec![
                atlas(0.5, false),
                atlas(0.25, true),
                atlas(0.0, false),
                atlas(0.75, true),
            ],
        ),
        (
            "atlas_missing_phase",
            vec![atlas(0.0, false), atlas(0.5, true)],
        ),
        (
            "atlas_duplicate_phase",
            vec![atlas(0.25, true), atlas(0.25, false), atlas(0.0, false)],
        ),
    ] {
        let mut store = store();
        store.set_atlases(atlases);
        let pixels = store
            .render_spec_with(
                "Hamburgefonstiv AVTRy.",
                &runs,
                &spec,
                RenderOptions {
                    use_atlas: true,
                    ..Default::default()
                },
            )
            .expect("atlas selection render");
        check_pixels(name, &pixels);
    }
}

#[test]
fn authored_psd_bytes() {
    check_environment();
    for case in &cases() {
        check_author(case);
    }
}

#[test]
fn point_render_and_geometry() {
    check_environment();
    for name in ["point_plain", "point_styles", "point_center"] {
        check_case_render(&case(name));
    }
}

#[test]
fn area_render_and_geometry() {
    check_environment();
    for name in ["area_paragraphs", "area_composer"] {
        check_case_render(&case(name));
    }
}

#[test]
fn fixed_and_vertical_bytes() {
    check_environment();
    check_fixed_and_vertical();
}

#[test]
fn character_features_bytes() {
    check_environment();
    check_features();
}

#[test]
fn warped_pixels_bytes() {
    check_environment();
    check_warps();
}

#[test]
fn atlas_selection_bytes() {
    check_environment();
    check_atlases();
}

#[test]
#[ignore = "manual hot-path timing; excludes fixture loading and compilation"]
fn render_hot_path_timing() {
    check_environment();
    let mut store = store();
    let point = case("point_plain");
    let area = case("area_paragraphs");
    for case in [&point, &area] {
        let ed = model(case);
        let text = text(case);
        let size = box_size(case);
        let start = Instant::now();
        let mut last = None;
        for _ in 0..100 {
            last = Some(std::hint::black_box(
                layer_render::render_from_engine_data(&store, &ed, &text, size, (0.25, 1.5))
                    .expect("timed render"),
            ));
        }
        eprintln!("{}: 100 renders {:?}", case.name, start.elapsed());
        check_pixels(&case.name, &last.expect("100 renders"));
    }
    store.set_atlases(vec![
        atlas(0.5, false),
        atlas(0.25, true),
        atlas(0.0, false),
        atlas(0.75, true),
    ]);
    let (runs, spec) = atlas_spec();
    let start = Instant::now();
    let mut last = None;
    for _ in 0..100 {
        last = Some(std::hint::black_box(
            store
                .render_spec_with(
                    "Hamburgefonstiv AVTRy.",
                    &runs,
                    &spec,
                    RenderOptions {
                        use_atlas: true,
                        ..Default::default()
                    },
                )
                .expect("timed atlas render"),
        ));
    }
    eprintln!("atlas_phases: 100 renders {:?}", start.elapsed());
    check_pixels("atlas_phases", &last.expect("100 renders"));
}
