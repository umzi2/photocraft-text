use std::path::PathBuf;

use photocraft_text::area::{layout_area_runs, AreaLayout, Justification, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layout::{GlyphPos, TextStyle};
use photocraft_text::runs::{FontSet, RunStyle};

fn font_bytes(name: &str) -> Vec<u8> {
    let home = std::env::var("HOME").expect("HOME");
    let path: PathBuf = [&home, ".local/share/photocraft/wine/drive_c/windows/Fonts", name].iter().collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn fonts(data: &[u8]) -> FontSet<'_> {
    FontSet::new("NotoSans-Regular", Face::parse(data).unwrap())
}

fn style() -> TextStyle {
    TextStyle { size: 12.0, ..Default::default() }
}

fn run(len: usize, color: [u8; 3]) -> RunStyle {
    RunStyle { len, size: Some(12.0), color: Some(color), ..Default::default() }
}

fn lay(faces: &FontSet<'_>, text: &str, runs: &[RunStyle], para: &ParagraphStyle, width: f32) -> AreaLayout {
    layout_area_runs(faces, text, runs, &style(), para, width, 1000.0, (0.0, 0.0), 0.8)
}

/// Width of the glyphs excluding trailing spaces.
fn content_width(glyphs: &[GlyphPos]) -> f32 {
    let end = glyphs.iter().rposition(|g| g.ch != ' ').map_or(0, |i| i + 1);
    if end == 0 {
        return 0.0;
    }
    glyphs[end - 1].x + glyphs[end - 1].advance - glyphs[0].x
}

#[test]
fn carriage_return_splits_paragraphs_without_leading_space() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let faces = fonts(&data);
    let runs = vec![run(5, [0, 0, 0])];
    let out = lay(&faces, "ab\rcd", &runs, &ParagraphStyle::default(), 1000.0);
    assert_eq!(out.lines.len(), 2);
    assert!(out.lines[1].baseline < out.lines[0].baseline, "second paragraph stacks below");
    let second = &out.lines[1].glyphs;
    assert_eq!(second.iter().map(|g| g.ch).collect::<String>(), "cd");
    assert_eq!(second[0].x, 0.0, "second paragraph starts at origin with no leading space");
}

#[test]
fn crlf_is_one_break() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let faces = fonts(&data);
    let runs = vec![run(6, [0, 0, 0])];
    let out = lay(&faces, "ab\r\ncd", &runs, &ParagraphStyle::default(), 1000.0);
    assert_eq!(out.lines.len(), 2);
    assert_eq!(out.lines[1].glyphs.iter().map(|g| g.ch).collect::<String>(), "cd");
}

#[test]
fn per_glyph_attrs_follow_runs_after_wrap() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let faces = fonts(&data);
    let text = "aaa bbb";
    let runs = vec![run(3, [255, 0, 0]), run(4, [0, 0, 255])];
    let wide = lay(&faces, text, &runs, &ParagraphStyle::default(), 1e6);
    let aaa: f32 = wide.lines[0].glyphs[..3].iter().map(|g| g.advance).sum();
    let bbb: f32 = wide.lines[0].glyphs[4..7].iter().map(|g| g.advance).sum();
    let out = lay(&faces, text, &runs, &ParagraphStyle::default(), aaa.max(bbb) + 0.5);
    assert_eq!(out.lines.len(), 2, "wraps after 'aaa '");
    for (line, expect_run, expect_color) in [(&out.lines[0], 0, [255, 0, 0]), (&out.lines[1], 1, [0, 0, 255])] {
        assert_eq!(line.attrs.len(), line.glyphs.len());
        for (g, a) in line.glyphs.iter().zip(&line.attrs) {
            if g.ch == ' ' {
                continue;
            }
            assert_eq!(a.run, expect_run, "glyph {:?}", g.ch);
            assert_eq!(a.color, expect_color, "glyph {:?}", g.ch);
        }
    }
    assert_eq!(out.lines[1].glyphs.iter().map(|g| g.ch).collect::<String>(), "bbb");
}

#[test]
fn overlong_word_breaks_by_character() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let faces = fonts(&data);
    let text = "WWWWWWWWWWWWWWWWWWWW";
    let runs = vec![run(text.len(), [0, 0, 0])];
    let box_w = 50.0;
    let out = lay(&faces, text, &runs, &ParagraphStyle::default(), box_w);
    assert!(out.lines.len() > 1, "overlong word must wrap");
    let total: usize = out.lines.iter().map(|l| l.glyphs.len()).sum();
    assert_eq!(total, text.len(), "no glyph lost");
    for line in &out.lines {
        assert!(content_width(&line.glyphs) <= box_w + 1e-3);
    }
}

/// Largest slack (box minus content width) over the non-last lines, and the
/// last line's content width.
fn raggedness(out: &AreaLayout, box_w: f32) -> (f32, f32) {
    let n = out.lines.len();
    let max_slack = out.lines[..n - 1]
        .iter()
        .map(|l| box_w - content_width(&l.glyphs))
        .fold(0.0f32, f32::max);
    (max_slack, content_width(&out.lines[n - 1].glyphs))
}

#[test]
fn every_line_composer_is_no_more_ragged_than_greedy() {
    let data = font_bytes("NotoSans-Regular.ttf");
    let faces = fonts(&data);
    let text = "WWWWWW i WWWWWW i WWWW iiii WWWWWW i WWWW ii WWWWWWW i WWWW iiii WWWWWW i WWWW ii WWWWWW";
    let runs = vec![run(text.encode_utf16().count(), [0, 0, 0])];
    let box_w = 160.0;
    let greedy_para = ParagraphStyle { justification: Justification::Left, ..Default::default() };
    let optimal_para = ParagraphStyle { every_line_composer: true, ..greedy_para };
    let greedy = lay(&faces, text, &runs, &greedy_para, box_w);
    let optimal = lay(&faces, text, &runs, &optimal_para, box_w);
    assert_eq!(greedy.lines.len(), optimal.lines.len(), "same line count");
    let (g_slack, g_last) = raggedness(&greedy, box_w);
    let (o_slack, o_last) = raggedness(&optimal, box_w);
    eprintln!("greedy max-slack {g_slack:.2} last {g_last:.2}; optimal max-slack {o_slack:.2} last {o_last:.2}");
    assert!(o_slack <= g_slack + 1e-3, "optimal max slack must not exceed greedy");
    assert!(o_last + 1e-3 >= g_last, "optimal last line not shorter than greedy");
}
