//! Point text has no frame: Photoshop aligns each line against the type origin
//! (the click), not against the widest line of the block.
//!
//! Measured on CC 2019 with NotoSans-Regular 30 pt, "Hello world" at x=200,
//! 400x160 px document: left bounds 203..359, center 122..278, right 42..198 —
//! every line starts, centres or ends on the origin. A renderer that lays point
//! text out flush left leaves all three at 203..359 (the bug this pins).

use photocraft_text::area::Justification;
use photocraft_text::engine_data::Value;
use photocraft_text::layer_render::{
    FaceStore, LayerPixels, render_from_engine_data_with_paragraphs,
};
use photocraft_text::runs::{ParagraphRun, RunStyle, to_engine_data};

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    let dir = std::env::var_os("HOME").map(|h| {
        std::path::PathBuf::from(h).join(".local/share/photocraft/wine/drive_c/windows/Fonts")
    });
    if let Some(dir) = dir {
        store.load_dir(&dir);
    }
    store
}

fn engine_data(len: usize, text: &str) -> Value {
    assert_eq!(
        text.encode_utf16().count(),
        len,
        "the run has to cover the text"
    );
    let runs = vec![RunStyle {
        len,
        size: Some(30.0),
        font: Some("NotoSans-Regular".into()),
        ..RunStyle::default()
    }];
    Value::dict([
        (
            "EngineDict",
            Value::dict([("StyleRun", to_engine_data(&runs))]),
        ),
        (
            "ResourceDict",
            Value::dict([(
                "FontSet",
                Value::Array(vec![Value::dict([
                    ("Name", Value::Str("NotoSans-Regular".into())),
                    ("Script", Value::Int(0)),
                ])]),
            )]),
        ),
    ])
}

/// Renders `text` as point text (no frame) with one paragraph alignment and
/// returns the ink interval in text space: `[offset.x, offset.x + width]`.
fn ink_interval(justification: Justification) -> (f32, f32) {
    let text = "Hello world";
    let len = text.encode_utf16().count();
    let ed = engine_data(len, text);
    let paragraphs = vec![ParagraphRun {
        len,
        justification,
        ..ParagraphRun::default()
    }];
    let px: LayerPixels = render_from_engine_data_with_paragraphs(
        &store(),
        &ed,
        text,
        &paragraphs,
        (0.0, 0.0),
        (0.0, 0.0),
    )
    .expect("render");
    assert!(px.width > 0, "no ink; the font store is empty");
    (px.offset.0, px.offset.0 + px.width as f32)
}

#[test]
fn point_text_alignment_turns_lines_around_the_origin() {
    let (left_x0, left_x1) = ink_interval(Justification::Left);
    let (center_x0, center_x1) = ink_interval(Justification::Center);
    let (right_x0, right_x1) = ink_interval(Justification::Right);

    // Left: the line starts at the origin (side bearings keep it inside by a
    // pixel or two).
    assert!(
        (0.0..=3.0).contains(&left_x0),
        "left-aligned point text must start at the origin, got {left_x0}"
    );
    let width = left_x1 - left_x0;

    // Center: the ink's midpoint is the origin.
    let center_mid = (center_x0 + center_x1) / 2.0;
    assert!(
        center_mid.abs() <= 2.0,
        "centred point text must centre on the origin, midpoint {center_mid} (interval {center_x0}..{center_x1})"
    );

    // Right: the ink ends at the origin.
    assert!(
        right_x1.abs() <= 2.0,
        "right-aligned point text must end at the origin, got {right_x1} (interval {right_x0}..{right_x1})"
    );

    // All three carry the same glyphs, so they differ only by the alignment
    // shift: half the width for centre, the whole width for right.
    assert!(
        (width - (center_x1 - center_x0)).abs() <= 1.0,
        "centre ink width {width}"
    );
    assert!(
        (width - (right_x1 - right_x0)).abs() <= 1.5,
        "right ink width {width}"
    );
    assert!(
        (left_x0 - center_x0 - width / 2.0).abs() <= 3.0,
        "centre shift {} != width/2 {width}",
        left_x0 - center_x0
    );
    assert!(
        (left_x0 - right_x0 - width).abs() <= 6.0,
        "right shift {} != width {width}",
        left_x0 - right_x0
    );
}
