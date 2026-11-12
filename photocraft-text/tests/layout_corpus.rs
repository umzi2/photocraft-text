//! Whole-corpus check: every fixture is a Photoshop PDF export, the engine has
//! to reproduce its glyph pen positions.

use std::path::PathBuf;

use photocraft_text::area::{layout_area, Justification, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layout::{layout_point, Kerning, TextStyle};

#[derive(serde::Deserialize)]
struct Fixture {
    id: String,
    text: String,
    em_px: f32,
    origin: f32,
    font: String,
    size: f32,
    #[serde(default)]
    tracking: i16,
    #[serde(default)]
    kerning: Option<i16>,
    #[serde(default)]
    leading: Option<f32>,
    /// Horizontal scale in percent, as Photoshop's Character panel stores it.
    #[serde(default)]
    hscale: Option<f32>,
    /// Vertical scale in percent; it must not change any advance.
    #[serde(default)]
    vscale: Option<f32>,
    #[serde(default)]
    box_width: Option<f32>,
    #[serde(default)]
    box_height: Option<f32>,
    #[serde(default)]
    box_left: Option<f32>,
    #[serde(default)]
    justification: Option<String>,
    glyphs: Vec<Glyph>,
}

#[derive(serde::Deserialize)]
struct Glyph {
    ch: String,
    x: f32,
    y: f32,
}

fn ascent_ratio(font: &str) -> f32 {
    let data = std::fs::read_to_string("tests/data/font_ascent.json").unwrap();
    let map: std::collections::HashMap<String, f32> = serde_json::from_str(&data).unwrap();
    *map.get(font).unwrap_or(&0.76)
}

fn font_path(name: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    let file = match name {
        "ArialMT" => "arial.ttf".to_string(),
        other => format!("{other}.ttf"),
    };
    PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/windows/Fonts").join(file)
}

fn fixtures() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir("tests/data")
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (p.extension().and_then(|e| e.to_str()) == Some("json")
                && name != "font_ascent.json"
                // CFF outline fixtures are not Photoshop PDF layouts.
                && name != "cff_bbox.json"
                // Composer fixtures carry line breaks instead of glyph positions.
                && !name.starts_with("composer_"))
            .then_some(p)
        })
        .collect();
    v.sort();
    v
}

#[test]
fn layout_matches_photoshop_corpus() {
    let fixtures = fixtures();
    assert!(fixtures.len() >= 20, "fixtures missing: {}", fixtures.len());
    let mut failures = Vec::new();

    for path in fixtures {
        let fx: Fixture = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let bytes = std::fs::read(font_path(&fx.font)).expect("font");
        let face = Face::parse(&bytes).unwrap();

        let style = TextStyle {
            size: fx.em_px, // points per em as Photoshop wrote them into the PDF
            tracking: fx.tracking,
            kerning: match fx.kerning {
                Some(v) => Kerning::Manual(v),
                None => Kerning::Metrics,
            },
            leading: match fx.leading {
                Some(0.0) | None => None,
                Some(v) => Some(v),
            },
            horizontal_scale: fx.hscale.map(|p| p / 100.0).unwrap_or(1.0),
            ..Default::default()
        };

        let mut ours: Vec<(char, f32, f32)> = Vec::new();
        match fx.box_width {
            None => {
                for line in layout_point(&face, &fx.text, &style, (fx.origin, 0.0)) {
                    for g in &line.glyphs {
                        ours.push((g.ch, g.x, g.y));
                    }
                }
            }
            Some(width) => {
                let para = ParagraphStyle {
                    justification: match fx.justification.as_deref() {
                        Some("right") => Justification::Right,
                        Some("center") => Justification::Center,
                        Some("justify") => Justification::JustifyLastLeft,
                        _ => Justification::Left,
                    },
                    ..Default::default()
                };
                // Ascender Photoshop uses for the first line of an area box.
                // The value is per font; `font_ascent.json` holds the measured
                // ratios (exact derivation is still open, see docs/STATUS.md).
                let ratio = ascent_ratio(&fx.font);
                let scaled = width * fx.em_px / fx.size;
                let out = layout_area(
                    &face,
                    &fx.text,
                    &style,
                    &para,
                    scaled,
                    fx.box_height.unwrap_or(200.0),
                    (fx.box_left.unwrap_or(fx.origin), 0.0),
                    ratio,
                );
                for line in &out.lines {
                    for g in &line.glyphs {
                        ours.push((g.ch, g.x, g.y));
                    }
                }
            }
        }

        if ours.len() != fx.glyphs.len() {
            failures.push(format!("{}: {} glyphs vs {}", fx.id, ours.len(), fx.glyphs.len()));
            continue;
        }
        for (i, (expected, (ch, x, y))) in fx.glyphs.iter().zip(&ours).enumerate() {
            let ch_expected: char = expected.ch.chars().next().unwrap();
            if *ch != ch_expected {
                failures.push(format!("{}[{i}]: char {ch:?} vs {ch_expected:?}", fx.id));
                break;
            }
            let tol = 0.01 + i as f32 * 0.0006;
            if (x - expected.x).abs() > tol {
                failures.push(format!(
                    "{}[{i}] {:?}: x {x:.5} vs {:.5} (d {:.5})",
                    fx.id,
                    ch,
                    expected.x,
                    x - expected.x
                ));
                break;
            }
            let dy = (y - ours[0].2) - (expected.y - fx.glyphs[0].y);
            if dy.abs() > 0.01 {
                failures.push(format!("{}[{i}] {:?}: y offset {dy:.5}", fx.id, ch));
                break;
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn vertical_scale_leaves_advances_alone() {
    // Same text set twice: one layer at 150% horizontal scale, one at 150%
    // vertical scale. Only the horizontal one may move the pen.
    let v: Fixture = serde_json::from_str(&std::fs::read_to_string("tests/data/vscale_150.json").unwrap()).unwrap();
    let h: Fixture = serde_json::from_str(&std::fs::read_to_string("tests/data/hscale_150.json").unwrap()).unwrap();
    assert_eq!(v.vscale, Some(150.0));
    assert_eq!(h.hscale, Some(150.0));
    assert_eq!(v.glyphs.len(), h.glyphs.len());
    for (i, (a, b)) in v.glyphs.iter().zip(h.glyphs.iter()).enumerate() {
        let plain = a.x - v.origin;
        let wide = b.x - h.origin;
        assert!(
            // 1e-4 of the pen position: the fixtures come from PDF text
            // matrices with limited precision, so errors accumulate.
            (wide - plain * 1.5).abs() < 0.02 + wide * 1e-4,
            "glyph {i}: vertical scale moved the pen ({plain}) or horizontal scale did not ({wide})"
        );
    }
}
