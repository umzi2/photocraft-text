//! Subpixel x phase: Photoshop keeps the fractional pen position and quantizes its
//! glyph bitmaps to a quarter-pixel grid (measured: a `H` layer at 48 pt renders
//! identically at pen fractions 0.125/0.25 and at 0.875/0.0, i.e. `round(f*4) mod 4`,
//! see out/verify/sync/parity-log.md). These tests pin that the *renderer* carries the
//! fraction through to the raster instead of rounding the glyph origin.

use std::path::{Path, PathBuf};

use photocraft_text::engine_data;
use photocraft_text::layer_render::{FaceStore, LayerPixels, render_from_engine_data};
use photocraft_text::psd::LayerSpec;

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    store
        .load(&assets().join("fonts"), "NotoSans-Regular")
        .expect("checked-in font");
    store
}

fn spec() -> LayerSpec {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("assets/cases.json")).expect("cases");
    let case = cases
        .as_array()
        .and_then(|a| a.iter().find(|c| c["name"] == "point_plain"))
        .expect("point_plain case");
    serde_json::from_value(case["spec"].clone()).expect("spec")
}

fn render(origin_x: f32) -> LayerPixels {
    let store = store();
    let spec = spec();
    let ed = photocraft_text::psd::build_engine_data(&spec);
    let text = spec.text.replace("\r\n", "\r").replace('\n', "\r");
    let text = text.trim_end_matches(['\r', '\n']);
    render_from_engine_data(&store, &ed, text, (600.0, 200.0), (origin_x, 60.0)).expect("render")
}

fn ink(p: &LayerPixels) -> (i32, i32, i32, i32, u64) {
    let (x0, y0) = (p.offset.0 as i32, p.offset.1 as i32);
    let mut min = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut sum = 0u64;
    for y in 0..p.height {
        for x in 0..p.width {
            let a = p.rgba[((y * p.width + x) * 4 + 3) as usize];
            if a > 8 {
                sum += u64::from(a);
                let (px, py) = (x0 + x as i32, y0 + y as i32);
                min.0 = min.0.min(px);
                min.1 = min.1.min(py);
                min.2 = min.2.max(px);
                min.3 = min.3.max(py);
            }
        }
    }
    (min.0, min.1, min.2, min.3, sum)
}

#[test]
fn report_offsets() {
    for ox in [100.0f32, 100.25, 100.5] {
        let p = render(ox);
        println!("origin {ox} offset {:?} ink {:?}", p.offset, ink(&p));
    }
}

#[test]
fn fractional_origin_moves_the_ink_and_keeps_subpixel_coverage() {
    let base = render(100.0);
    let quarter = render(100.25);
    let half = render(100.5);
    let (b, q, h) = (ink(&base), ink(&quarter), ink(&half));
    // Coverage sum is an antialiasing fingerprint: with an integer pen the four
    // renders are bit-identical, which is what the old path produced.
    assert_ne!(
        base.rgba, quarter.rgba,
        "quarter-pixel origin was rounded away"
    );
    assert_ne!(base.rgba, half.rgba, "half-pixel origin was rounded away");
    // The ink box moves by at most the integer step the pen actually crossed.
    assert!(q.0 - b.0 <= 1 && h.0 - b.0 <= 1);
    assert_eq!(b.1, q.1);
    assert_eq!(b.1, h.1);
}
