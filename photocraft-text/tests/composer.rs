//! Every-line composer and line breaking against Photoshop.
//!
//! The fixtures come from `out/composer_{single,every}.pdf`: Photoshop set the
//! same paragraph twice, once with the single-line composer and once with the
//! every-line composer, and the PDFs record where every line ends.

use std::path::PathBuf;

use photocraft_text::area::{layout_area, Justification, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;

#[derive(serde::Deserialize)]
struct Fixture {
    text: String,
    composer: String,
    font: String,
    size: f32,
    leading: f32,
    box_width: f32,
    box_height: f32,
    box_left: f32,
    line_ends: Vec<usize>,
    lines: Vec<String>,
    #[serde(default)]
    advances: Vec<f32>,
}

fn font_path(name: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    PathBuf::from(home)
        .join(".local/share/photocraft/wine/drive_c/windows/Fonts")
        .join(format!("{name}.ttf"))
}

fn fixture(id: &str) -> Fixture {
    let path = PathBuf::from("tests/data").join(format!("{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("parse")
}

fn layout(fx: &Fixture) -> Vec<String> {
    let bytes = std::fs::read(font_path(&fx.font)).expect("font");
    let face = Face::parse(&bytes).expect("parse");
    let style = TextStyle { size: fx.size, leading: Some(fx.leading), ..Default::default() };
    let para = ParagraphStyle {
        justification: Justification::Left,
        every_line_composer: fx.composer == "every",
        ..Default::default()
    };
    let out = layout_area(
        &face,
        &fx.text,
        &style,
        &para,
        fx.box_width,
        fx.box_height,
        (fx.box_left, 0.0),
        0.76,
    );
    out.lines
        .iter()
        .map(|l| l.glyphs.iter().map(|g| g.ch).collect::<String>().trim_end().to_string())
        .collect()
}

#[test]
fn single_line_composer_matches_photoshop_breaks() {
    let fx = fixture("composer_single");
    assert_eq!(fx.composer, "single");
    let ours = layout(&fx);
    assert_eq!(ours, fx.lines, "single-line composer broke the paragraph differently");
    let mut at = 0;
    let ends: Vec<usize> = ours
        .iter()
        .map(|l| {
            at += l.chars().count() + 1;
            at.min(fx.text.chars().count())
        })
        .collect();
    assert_eq!(ends, fx.line_ends, "line-end indices drifted from Photoshop's");
}

#[test]
fn every_line_composer_matches_photoshop_breaks() {
    let fx = fixture("composer_every");
    assert_eq!(fx.composer, "every");
    let ours = layout(&fx);
    assert_eq!(ours, fx.lines, "every-line composer broke the paragraph differently");
    // The composers must actually disagree here, otherwise the fixture proves
    // nothing about the every-line search.
    assert_ne!(layout(&fixture("composer_single")), ours);
}

#[test]
fn single_line_composer_reproduces_photoshop_advances() {
    // Photoshop's single-line composer leaves glyph advances untouched, so the
    // pen deltas in its PDF are the metrics the engine has to emit. (The
    // every-line composer nudges advances to balance the paragraph; that is
    // not reproduced.)
    let fx = fixture("composer_single");
    let bytes = std::fs::read(font_path(&fx.font)).expect("font");
    let face = Face::parse(&bytes).expect("parse");
    let style = TextStyle { size: fx.size, leading: Some(fx.leading), ..Default::default() };
    let para = ParagraphStyle { justification: Justification::Left, ..Default::default() };
    let out = layout_area(
        &face,
        &fx.text,
        &style,
        &para,
        fx.box_width,
        fx.box_height,
        (fx.box_left, 0.0),
        0.76,
    );
    // The fixture's advance list drops each line's last glyph (no `Td` before
    // the next line), so compare in the same order.
    let mut ours = Vec::new();
    for line in &out.lines {
        let n = line.glyphs.len();
        for g in line.glyphs.iter().take(n.saturating_sub(1)) {
            ours.push(g.advance);
        }
    }
    let count = ours.len().min(fx.advances.len());
    assert!(count > 50, "fixture has too few advances: {count}");
    let mut worst = 0.0f32;
    for i in 0..count {
        // Line-final glyphs differ (the fixture keeps the trailing space's
        // advance), so allow a space's worth of drift there.
        let d = (ours[i] - fx.advances[i]).abs();
        worst = worst.max(d);
        assert!(d < 0.02, "glyph {i}: ours {:.4} vs Photoshop {:.4}", ours[i], fx.advances[i]);
    }
    assert!(worst < 0.02);
}
