use std::path::PathBuf;

use photocraft_text::Error;
use photocraft_text::engine_data::Value;
use photocraft_text::layer_render::{
    FaceStore, LayerPixels, LayerTextStyle, RectTarget, RenderOptions, render_from_engine_data,
    render_from_engine_data_in_rect, render_from_engine_data_with_paragraphs,
    render_vertical_from_engine_data,
};
use photocraft_text::runs::{self, RunStyle};

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/fonts");
    store.load(&dir, "NotoSans-Regular").expect("bundled font");
    store
}

fn run(len: usize) -> RunStyle {
    RunStyle {
        len,
        font: Some("NotoSans-Regular".into()),
        font_index: Some(0),
        size: Some(24.0),
        ..Default::default()
    }
}

fn engine_data(styles: &[RunStyle]) -> Value {
    Value::dict([
        (
            "EngineDict",
            Value::dict([("StyleRun", runs::to_engine_data(styles))]),
        ),
        (
            "ResourceDict",
            Value::dict([(
                "FontSet",
                Value::Array(vec![Value::dict([(
                    "Name",
                    Value::Str("NotoSans-Regular".into()),
                )])]),
            )]),
        ),
    ])
}

fn assert_same_pixels(a: &LayerPixels, b: &LayerPixels) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert_eq!(
        (a.offset.0.to_bits(), a.offset.1.to_bits()),
        (b.offset.0.to_bits(), b.offset.1.to_bits())
    );
    assert_eq!(a.rgba, b.rgba);
}

#[test]
fn empty_font_store_is_an_error_not_a_panic() {
    let result = FaceStore::new().render_spec("A", &[run(1)], &LayerTextStyle::default());
    assert!(matches!(result, Err(Error::BadFont(_))));
}

#[test]
fn missing_named_run_font_is_not_silently_substituted() {
    let styles = [
        run(1),
        RunStyle {
            font: Some("Unavailable-Font".into()),
            ..run(1)
        },
    ];
    let result = store().render_spec("AB", &styles, &LayerTextStyle::default());
    assert!(matches!(result, Err(Error::BadFont(_))));
}

#[test]
fn absent_style_table_is_an_error_not_a_panic() {
    let result =
        render_from_engine_data(&store(), &Value::Dict(vec![]), "A", (0.0, 0.0), (0.0, 0.0));
    assert!(matches!(result, Err(Error::EngineData("no style runs"))));
}

#[test]
fn empty_paragraph_override_matches_the_stored_paragraphs() {
    let store = store();
    let ed = engine_data(&[run(5)]);
    let a = render_from_engine_data(&store, &ed, "Hi\rA", (100.0, 100.0), (0.0, 0.0)).unwrap();
    let b = render_from_engine_data_with_paragraphs(
        &store,
        &ed,
        "Hi\rA",
        &[],
        (100.0, 100.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert_same_pixels(&a, &b);
}

#[test]
fn unavailable_atlas_does_not_change_outline_pixels() {
    let store = store();
    let style = LayerTextStyle::default();
    let a = store.render_spec("AV", &[run(2)], &style).unwrap();
    let b = store
        .render_spec_with(
            "AV",
            &[run(2)],
            &style,
            RenderOptions {
                use_atlas: true,
                aa: None,
            },
        )
        .unwrap();
    assert_same_pixels(&a, &b);
}

#[test]
fn fixed_rectangle_has_exact_buffer_size_and_zero_offset() {
    let store = store();
    let ed = engine_data(&[run(3)]);
    let target = RectTarget {
        origin: (-10, -20),
        size: (80, 64),
        anchor: (1.25, 2.5),
    };
    let pixels =
        render_from_engine_data_in_rect(&store, &ed, "AV", &[], (0.0, 0.0), (0.0, 0.0), target)
            .unwrap();
    assert_eq!((pixels.width, pixels.height), target.size);
    assert_eq!(pixels.rgba.len(), 80 * 64 * 4);
    assert_eq!(pixels.offset, (0.0, 0.0));
    assert!(pixels.rgba.chunks_exact(4).any(|p| p[3] > 0));
}

#[test]
fn vertical_terminator_only_run_is_rejected_without_indexing_an_empty_table() {
    let ed = engine_data(&[run(1)]);
    let result = render_vertical_from_engine_data(&store(), &ed, "", (0.0, 0.0));
    assert!(matches!(result, Err(Error::EngineData("no style runs"))));
}

#[test]
fn explicit_paragraphs_match_stored_sheets_and_override_alignment() {
    let store = store();
    let ed = photocraft_text::engine_data::parse(include_bytes!(
        "assets/models/area_paragraphs.engine.bin"
    ))
    .expect("bundled model");
    let text = "Hamburg AVATAR wraps over several words.\rSecond paragraph.";
    let mut paragraphs = runs::paragraphs_from_engine_data(&ed);
    assert_eq!(paragraphs.len(), 2);
    let original = render_from_engine_data(&store, &ed, text, (250.0, 200.0), (0.0, 0.0)).unwrap();
    let explicit = render_from_engine_data_with_paragraphs(
        &store,
        &ed,
        text,
        &paragraphs,
        (250.0, 200.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert_same_pixels(&original, &explicit);
    paragraphs[0].justification = photocraft_text::area::Justification::Left;
    let changed = render_from_engine_data_with_paragraphs(
        &store,
        &ed,
        text,
        &paragraphs,
        (250.0, 200.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert_ne!(
        (original.offset.0.to_bits(), &original.rgba),
        (changed.offset.0.to_bits(), &changed.rgba),
        "nonempty paragraph overrides must not be ignored",
    );
}
