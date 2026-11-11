use std::path::PathBuf;

use photocraft_text::engine_data::Value;
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::{self, FontSet, RunStyle};
use photocraft_text::typography::faux_bold_extra_advance;

const BASE_SIZE: f32 = 24.0;

fn font_bytes(name: &str) -> Vec<u8> {
    let home = std::env::var("HOME").expect("HOME");
    let path: PathBuf = [&home, ".local/share/photocraft/wine/drive_c/windows/Fonts", name].iter().collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn base() -> TextStyle {
    TextStyle { size: BASE_SIZE, ..TextStyle::default() }
}

fn run(len: usize) -> RunStyle {
    RunStyle { len, ..RunStyle::default() }
}

#[test]
fn fi_split_across_two_runs_ligates_to_one_glyph() {
    let bytes = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let shaped = runs::shape_runs_attrs(&fonts, "fi", &[run(1), run(1)], &base());
    assert_eq!(shaped.len(), 1, "fi with identical runs must ligate");
    let (glyph, attrs) = shaped[0];
    let face = fonts.default_face();
    assert_ne!(glyph.gid, face.glyph_index('f').unwrap());
    assert_eq!(attrs.run, 0, "ligature takes the first component's run");
}

#[test]
fn ligatures_disabled_keeps_every_glyph() {
    let bytes = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let off = RunStyle {
        len: 6,
        sheet: Some(Value::dict([("Ligatures", Value::Bool(false))])),
        ..RunStyle::default()
    };
    let shaped = runs::shape_runs_attrs(&fonts, "office", &[off], &base());
    assert_eq!(shaped.len(), 6);
    let face = fonts.default_face();
    for ((glyph, _), ch) in shaped.iter().zip("office".chars()) {
        assert_eq!(glyph.gid, face.glyph_index(ch).unwrap());
    }
    let on = runs::shape_runs_attrs(&fonts, "office", &[run(6)], &base());
    assert!(on.len() < 6, "default Ligatures=true must substitute");
}

#[test]
fn all_caps_maps_to_uppercase_glyphs() {
    let bytes = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let caps = RunStyle { len: 2, all_caps: true, ..RunStyle::default() };
    let shaped = runs::shape_runs_attrs(&fonts, "ab", &[caps], &base());
    let face = fonts.default_face();
    assert_eq!(shaped.len(), 2);
    assert_eq!(shaped[0].0.gid, face.glyph_index('A').unwrap());
    assert_eq!(shaped[1].0.gid, face.glyph_index('B').unwrap());
    assert!(close(shaped[0].1.vertical_scale, 1.0));
}

#[test]
fn small_caps_scales_lowercase_only() {
    let bytes = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let small = RunStyle { len: 2, small_caps: true, ..RunStyle::default() };
    let shaped = runs::shape_runs_attrs(&fonts, "aB", &[small], &base());
    let face = fonts.default_face();
    assert_eq!(shaped.len(), 2);
    assert_eq!(shaped[0].0.gid, face.glyph_index('A').unwrap());
    assert_eq!(shaped[1].0.gid, face.glyph_index('B').unwrap());
    assert!(close(shaped[0].1.vertical_scale, runs::SMALL_CAPS_SCALE));
    assert!(close(shaped[1].1.vertical_scale, 1.0));

    // Last glyph's advance is its own advance: scaled by SMALL_CAPS_SCALE.
    let single = RunStyle { len: 1, small_caps: true, ..RunStyle::default() };
    let one = runs::shape_runs_attrs(&fonts, "a", &[single], &base());
    let expected = face.advance(face.glyph_index('A').unwrap()) as f32 * BASE_SIZE / face.units_per_em() as f32
        * runs::SMALL_CAPS_SCALE;
    assert!(close(one[0].0.advance, expected), "{} vs {expected}", one[0].0.advance);
}

#[test]
fn vertical_scale_does_not_move_pen() {
    let bytes = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let plain = runs::shape_runs_attrs(&fonts, "Hello", &[run(5)], &base());
    let scaled = RunStyle { len: 5, vertical_scale: Some(0.5), ..RunStyle::default() };
    let tall = runs::shape_runs_attrs(&fonts, "Hello", &[scaled], &base());
    assert_eq!(plain.len(), tall.len());
    for ((p, _), (t, a)) in plain.iter().zip(tall.iter()) {
        assert!(close(p.x, t.x));
        assert!(close(a.vertical_scale, 0.5));
    }
}

#[test]
fn faux_bold_adds_extra_advance_per_glyph() {
    let bytes = font_bytes("NotoSans-Regular.ttf");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let bold = RunStyle { len: 2, faux_bold: true, faux_italic: true, ..RunStyle::default() };
    let plain = runs::shape_runs_attrs(&fonts, "ab", &[run(2)], &base());
    let fat = runs::shape_runs_attrs(&fonts, "ab", &[bold], &base());
    let extra = faux_bold_extra_advance(BASE_SIZE);
    assert!(close(fat[1].0.x - fat[0].0.x, plain[1].0.x - plain[0].0.x + extra));
    assert!(close(fat[0].0.advance, plain[0].0.advance + extra));
    assert!(fat[0].1.faux_italic);
    assert!(!plain[0].1.faux_italic);
}

/// Small caps shrink the glyph in both axes. The layout suite only sees
/// advances; painting that scale in y alone gives tall, narrow letters.
#[test]
fn small_caps_scale_reaches_both_axes() {
    let bytes = std::fs::read(
        std::env::var("HOME")
            .map(|h| format!("{h}/.local/share/photocraft/wine/drive_c/windows/Fonts/NotoSans-Regular.ttf"))
            .expect("HOME"),
    )
    .expect("font");
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let small = RunStyle { len: 1, small_caps: true, size: Some(48.0), ..RunStyle::default() };
    let plain = RunStyle { len: 1, size: Some(48.0), ..RunStyle::default() };
    let shaped = runs::shape_runs_attrs(&fonts, "a", &[small.clone()], &base());
    let attrs = shaped[0].1;
    assert!(close(attrs.x_scale, runs::SMALL_CAPS_SCALE), "x scale {}", attrs.x_scale);
    assert!(close(attrs.vertical_scale, runs::SMALL_CAPS_SCALE), "y scale {}", attrs.vertical_scale);
    let shaped_plain = runs::shape_runs_attrs(&fonts, "a", &[plain], &base());
    assert!(close(shaped_plain[0].1.x_scale, 1.0), "regular glyphs must not scale");
}
