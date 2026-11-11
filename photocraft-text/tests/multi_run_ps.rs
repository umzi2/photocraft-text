//! Photoshop's layout of authored multi-run text, checked glyph by glyph.
//!
//! Each fixture in `tests/data/multi_run/` holds the run spec that was written
//! into a `Txt2` block and the glyph positions Photoshop produced for it. The
//! engine must reproduce the pen positions, advances and baseline offsets.

use std::path::PathBuf;

use photocraft_text::area::{layout_area_runs, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::{FontSet, RunStyle};

#[derive(serde::Deserialize)]
struct Fixture {
    text: String,
    runs: Vec<Run>,
    glyphs: Vec<Glyph>,
}

#[derive(serde::Deserialize)]
struct Run {
    len: usize,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    tracking: Option<i16>,
    #[serde(default)]
    baseline_shift: Option<f32>,
    #[serde(default)]
    horizontal_scale: Option<f32>,
    #[serde(default)]
    vertical_scale: Option<f32>,
}

#[derive(serde::Deserialize)]
struct Glyph {
    ch: String,
    x: f32,
    dy: f32,
    advance: Option<f32>,
}

fn font_bytes() -> Vec<u8> {
    let home = std::env::var("HOME").expect("HOME");
    let path: PathBuf = [&home, ".local/share/photocraft/wine/drive_c/windows/Fonts", "NotoSans-Regular.ttf"]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn multi_run_layout_matches_photoshop() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/multi_run");
    let data = font_bytes();
    let mut checked = 0;

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();

    for path in entries {
        let fx: Fixture = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let faces = FontSet::new("NotoSans-Regular", Face::parse(&data).unwrap());
        let runs: Vec<RunStyle> = fx
            .runs
            .iter()
            .map(|r| RunStyle {
                len: r.len,
                size: r.size,
                tracking: r.tracking,
                baseline_shift: r.baseline_shift,
                horizontal_scale: r.horizontal_scale,
                vertical_scale: r.vertical_scale,
                ..RunStyle::default()
            })
            .collect();
        // One line: the width is far past the text, and PS wraps nothing here.
        let out = layout_area_runs(
            &faces,
            &fx.text,
            &runs,
            &TextStyle { size: 48.0, ..Default::default() },
            &ParagraphStyle::default(),
            10_000.0,
            1000.0,
            (0.0, 0.0),
            0.8,
        );
        let line = &out.lines[0];
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(line.glyphs.len(), fx.glyphs.len(), "{name}: glyph count");
        let y0 = line.glyphs[0].y;
        for (i, (g, want)) in line.glyphs.iter().zip(&fx.glyphs).enumerate() {
            assert_eq!(g.ch.to_string(), want.ch, "{name}[{i}]: char");
            assert!((g.x - want.x).abs() < 0.01, "{name}[{i}] x {} != {}", g.x, want.x);
            if let Some(want_adv) = want.advance {
                assert!((g.advance - want_adv).abs() < 0.01, "{name}[{i}] advance {} != {want_adv}", g.advance);
            }
            let dy = g.y - y0;
            assert!((dy - want.dy).abs() < 0.01, "{name}[{i}] dy {dy} != {}", want.dy);
        }
        checked += 1;
    }
    assert!(checked > 0, "no multi-run fixtures in {}", dir.display());
}
