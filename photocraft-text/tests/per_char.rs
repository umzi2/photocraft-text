//! Per-character parameter changes.
//!
//! Every other suite lays out a layer whose whole text shares one style. These
//! tests pin down what happens when the parameters change *between glyphs*:
//! size, colour, baseline shift, vertical and horizontal scale, tracking, font
//! and the paragraph-level knobs that interact with them.
//!
//! Layout assertions read `Line::glyphs` / `Line::attrs`; raster assertions
//! read the pixels produced by `render_from_engine_data`, so a per-run style
//! that is parsed but never painted fails here.

use std::path::PathBuf;

use photocraft_text::area::{layout_area_runs, ParagraphStyle};
use photocraft_text::engine_data::Value;
use photocraft_text::font::Face;
use photocraft_text::layer_render::{render_from_engine_data, FaceStore, LayerPixels};
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::{self, FontSet, RunStyle};

fn font_path(name: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    [&home, ".local/share/photocraft/wine/drive_c/windows/Fonts", name].iter().collect()
}

fn font_bytes(name: &str) -> Vec<u8> {
    let path = font_path(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn sans() -> Vec<u8> {
    font_bytes("NotoSans-Regular.ttf")
}

fn base() -> TextStyle {
    TextStyle { size: 24.0, ..TextStyle::default() }
}

fn run(len: usize) -> RunStyle {
    RunStyle { len, ..RunStyle::default() }
}

/// Lays out `text` with one run per character, from `styles`.
fn layout_per_char(text: &str, styles: Vec<RunStyle>) -> (Vec<photocraft_text::layout::GlyphPos>, Vec<runs::GlyphAttrs>) {
    let bytes = sans();
    let fonts = FontSet::new("NotoSans", Face::parse(&bytes).expect("parse"));
    let runs: Vec<RunStyle> = styles
        .into_iter()
        .map(|mut r| {
            r.len = 1;
            r
        })
        .collect();
    let shaped = runs::shape_runs_attrs(&fonts, text, &runs, &base());
    let (pos, attrs): (Vec<_>, Vec<_>) = shaped.into_iter().unzip();
    (pos, attrs)
}

fn sized(size: f32) -> RunStyle {
    RunStyle { size: Some(size), ..RunStyle::default() }
}

#[test]
fn per_character_size_scales_advance_and_height() {
    let (pos, attrs) = layout_per_char("AV", vec![sized(24.0), sized(48.0)]);
    assert_eq!(pos.len(), 2);
    let (a, v) = (pos[0], pos[1]);
    let sans_bytes = sans();
    let plain = Face::parse(&sans_bytes).expect("parse");
    let expect_a = plain.advance(a.gid) as f32 / 1000.0 * 24.0;
    let expect_v = plain.advance(v.gid) as f32 / 1000.0 * 48.0;
    assert!((a.advance - expect_a).abs() < 0.05, "A advance {} != {expect_a}", a.advance);
    assert!((v.advance - expect_v).abs() < 0.05, "V advance {} != {expect_v}", v.advance);
    assert_eq!(attrs[0].run, 0);
    assert_eq!(attrs[1].run, 1, "second run must own the second glyph");
}

#[test]
fn per_character_size_mid_word_keeps_pen_continuous() {
    // "AB": the B is twice the size of the A, so its origin is exactly the end
    // of A's advance and the line width is A + B, with nothing shared.
    let (pos, _) = layout_per_char("AB", vec![sized(20.0), sized(40.0)]);
    assert!((pos[1].x - pos[0].advance).abs() < 1e-3, "pen broke at the run boundary: {} vs {}", pos[1].x, pos[0].advance);
}

#[test]
fn per_character_baseline_shift_lifts_only_that_glyph() {
    let shifted = RunStyle { baseline_shift: Some(10.0), ..RunStyle::default() };
    let (pos, _) = layout_per_char("HH", vec![run(1), shifted]);
    // Baseline shift is applied by the renderer, not the pen; x stays put and
    // the glyph keeps its own advance.
    assert!((pos[1].x - pos[0].advance).abs() < 1e-3, "baseline shift moved the pen");
}

#[test]
fn per_character_horizontal_scale_stretches_advance_only() {
    let wide = RunStyle { horizontal_scale: Some(1.5), ..RunStyle::default() };
    let (plain, _) = layout_per_char("AA", vec![run(1), run(1)]);
    let (scaled, _) = layout_per_char("AA", vec![run(1), wide]);
    assert!((scaled[1].x - plain[1].x).abs() < 1e-3, "horizontal scale moved the second origin");
    let ratio = scaled[1].advance / plain[1].advance;
    assert!((ratio - 1.5).abs() < 1e-3, "advance ratio {ratio} != 1.5");
}

#[test]
fn per_character_vertical_scale_does_not_touch_advance() {
    let tall = RunStyle { vertical_scale: Some(2.0), ..RunStyle::default() };
    let (plain, _) = layout_per_char("AA", vec![run(1), run(1)]);
    let (scaled, attrs) = layout_per_char("AA", vec![run(1), tall]);
    assert!((scaled[1].advance - plain[1].advance).abs() < 1e-3);
    assert!((attrs[1].vertical_scale - 2.0).abs() < 1e-6, "vertical scale {} != 2", attrs[1].vertical_scale);
    assert!((attrs[0].vertical_scale - 1.0).abs() < 1e-6);
}

#[test]
fn per_character_tracking_widens_only_the_tracked_run() {
    let loose = RunStyle { tracking: Some(200), ..RunStyle::default() };
    let (plain, _) = layout_per_char("AAA", vec![run(1), run(1), run(1)]);
    let (tracked, _) = layout_per_char("AAA", vec![run(1), loose, run(1)]);
    let extra = 200.0 / 1000.0 * 24.0;
    assert!((tracked[1].x - plain[1].x).abs() < 1e-3, "tracking moved the origin of its own glyph");
    assert!((tracked[1].advance - plain[1].advance - extra).abs() < 1e-3, "tracked advance {} != {}", tracked[1].advance, plain[1].advance + extra);
    assert!((tracked[2].x - plain[2].x - extra).abs() < 1e-3, "gap after the tracked run missing");
}

#[test]
fn per_character_colour_is_kept_per_run() {
    let red = RunStyle { color: Some([255, 0, 0]), ..RunStyle::default() };
    let blue = RunStyle { color: Some([0, 0, 255]), ..RunStyle::default() };
    let (_, attrs) = layout_per_char("AB", vec![red, blue]);
    assert_eq!(attrs[0].color, [255, 0, 0]);
    assert_eq!(attrs[1].color, [0, 0, 255]);
}

// ---------------------------------------------------------------------------
// Rasterised output
// ---------------------------------------------------------------------------

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    let home = std::env::var("HOME").expect("HOME");
    let dir: PathBuf = [&home, ".local/share/photocraft/wine/drive_c/windows/Fonts"].iter().collect();
    let loaded = store.load_dir(&dir);
    assert!(loaded > 0, "no fonts loaded from {}", dir.display());
    store
}

fn engine_data(fonts: &[&str], runs: Vec<RunStyle>, text: &str) -> Value {
    let font_set = fonts
        .iter()
        .map(|name| Value::dict([("Name", Value::Str(name.to_string())), ("Script", Value::Int(0))]))
        .collect();
    let mut runs = runs;
    let mut covered = 0;
    for r in runs.iter_mut() {
        covered += r.len;
    }
    assert_eq!(covered, text.encode_utf16().count(), "runs must cover the text");
    Value::dict([
        (
            "EngineDict",
            Value::dict([("StyleRun", runs::to_engine_data(&runs))]),
        ),
        (
            "ResourceDict",
            Value::dict([("FontSet", Value::Array(font_set))]),
        ),
    ])
}

fn render(text: &str, runs: Vec<RunStyle>, box_size: (f32, f32)) -> LayerPixels {
    let ed = engine_data(&["NotoSans-Regular"], runs, text);
    render_from_engine_data(&store(), &ed, text, box_size, (0.0, 0.0)).expect("render")
}

/// Widest run of fully transparent columns: the gap between the two glyphs.
fn glyph_gap(p: &LayerPixels) -> (u32, u32) {
    glyph_gap_above(p, p.height)
}

/// Same, but only rows above `y_limit` (decorations below the baseline would
/// otherwise close the gap between two glyphs).
fn glyph_gap_above(p: &LayerPixels, y_limit: u32) -> (u32, u32) {
    let empty = |x: u32| (0..y_limit.min(p.height)).all(|y| p.rgba[((y * p.width + x) * 4 + 3) as usize] <= 8);
    let (mut best, mut best_len) = ((0, 0), 0);
    let mut run_start: Option<u32> = None;
    for x in 0..=p.width {
        if x < p.width && empty(x) {
            run_start.get_or_insert(x);
        } else if let Some(start) = run_start.take() {
            let len = x - start;
            if len > best_len && start > 0 && x < p.width {
                best = (start, x);
                best_len = len;
            }
        }
    }
    assert!(best_len >= 2, "no gap between the glyphs (widest empty run {best_len})");
    best
}

fn ink_column_range(p: &LayerPixels, x0: u32, x1: u32) -> Option<(u32, u32)> {
    let mut lo = u32::MAX;
    let mut hi = 0;
    for y in 0..p.height {
        for x in x0..x1.min(p.width) {
            if p.rgba[((y * p.width + x) * 4 + 3) as usize] > 8 {
                lo = lo.min(y);
                hi = hi.max(y);
            }
        }
    }
    (hi > lo).then_some((lo, hi))
}

#[test]
fn per_character_colour_reaches_the_pixels() {
    let red = RunStyle { color: Some([255, 0, 0]), len: 1, ..RunStyle::default() };
    let blue = RunStyle { color: Some([0, 0, 255]), len: 1, ..RunStyle::default() };
    let p = render("HH", vec![red, blue], (200.0, 80.0));
    let (g0, g1) = glyph_gap(&p);
    let sample = |x0: u32, x1: u32| {
        let mut best = [0u8; 4];
        for y in 0..p.height {
            for x in x0..x1 {
                let i = ((y * p.width + x) * 4) as usize;
                if p.rgba[i + 3] > 200 && p.rgba[i + 3] > best[3] {
                    best = [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2], p.rgba[i + 3]];
                }
            }
        }
        best
    };
    let left = sample(0, g0);
    let right = sample(g1, p.width);
    assert!(left[0] > 200 && left[2] < 40, "left glyph is not red: {left:?}");
    assert!(right[2] > 200 && right[0] < 40, "right glyph is not blue: {right:?}");
}

#[test]
fn per_character_size_changes_rendered_cap_height() {
    let small = RunStyle { size: Some(24.0), len: 1, ..RunStyle::default() };
    let big = RunStyle { size: Some(48.0), len: 1, ..RunStyle::default() };
    let p = render("HH", vec![small, big], (240.0, 120.0));
    let (g0, g1) = glyph_gap(&p);
    let lo = ink_column_range(&p, 0, g0).expect("left ink");
    let ro = ink_column_range(&p, g1, p.width).expect("right ink");
    let (lh, rh) = (lo.1 - lo.0, ro.1 - ro.0);
    let ratio = rh as f32 / lh as f32;
    assert!((ratio - 2.0).abs() < 0.12, "cap height ratio {ratio} != 2 ({lh} vs {rh})");
    assert!(lo.0 - ro.0 > (lh / 2) as u32, "the larger glyph must rise above the small one (tops {} {})", lo.0, ro.0);
    assert!(ro.1.abs_diff(lo.1) <= 2, "both glyphs share the baseline (bottoms {} {})", lo.1, ro.1);
}

#[test]
fn per_character_baseline_shift_moves_rendered_ink() {
    let up = RunStyle { baseline_shift: Some(12.0), len: 1, ..RunStyle::default() };
    let plain = RunStyle { len: 1, ..RunStyle::default() };
    let p = render("HH", vec![plain, up], (240.0, 120.0));
    let (g0, g1) = glyph_gap(&p);
    let lo = ink_column_range(&p, 0, g0).expect("left ink");
    let ro = ink_column_range(&p, g1, p.width).expect("right ink");
    assert!(ro.1 < lo.1 - 8, "shifted glyph did not rise: left bottom {} right bottom {}", lo.1, ro.1);
    assert!((ro.0 as i32 - lo.0 as i32 + 12).abs() <= 2, "shift {} != 12", lo.0 as i32 - ro.0 as i32);
}

#[test]
fn per_character_vertical_scale_stretches_rendered_ink() {
    let tall = RunStyle { vertical_scale: Some(2.0), len: 1, ..RunStyle::default() };
    let plain = RunStyle { len: 1, ..RunStyle::default() };
    let p = render("HH", vec![plain, tall], (240.0, 160.0));
    let (g0, g1) = glyph_gap(&p);
    let lo = ink_column_range(&p, 0, g0).expect("left ink");
    let ro = ink_column_range(&p, g1, p.width).expect("right ink");
    let ratio = (ro.1 - ro.0) as f32 / (lo.1 - lo.0) as f32;
    assert!((ratio - 2.0).abs() < 0.15, "rendered vertical scale {ratio} != 2");
}

#[test]
fn per_character_font_switch_uses_the_second_face() {
    // Noto Sans and Noto Serif do not share advances, so a run that names the
    // serif face must move the pen by the serif metrics.
    let serif_bytes = font_bytes("NotoSerif-Regular.ttf");
    let sans_bytes = sans();
    let sans_face = Face::parse(&sans_bytes).expect("parse sans");
    let serif_face = Face::parse(&serif_bytes).expect("parse serif");
    let fonts = FontSet::new("NotoSans-Regular", sans_face);
    let mut fonts = fonts;
    fonts.insert("NotoSerif-Regular", serif_face);
    let text = "HH";
    let runs = [
        RunStyle { len: 1, ..RunStyle::default() },
        RunStyle { len: 1, font: Some("NotoSerif-Regular".into()), ..RunStyle::default() },
    ];
    let shaped = runs::shape_runs_attrs(&fonts, text, &runs, &base());
    assert_eq!(shaped.len(), 2);
    let serif = fonts.get("NotoSerif-Regular").expect("serif face");
    let expect = serif.advance(serif.glyph_index('H').unwrap()) as f32 / 1000.0 * 24.0;
    assert!((shaped[1].0.advance - expect).abs() < 0.05, "serif advance {} != {expect}", shaped[1].0.advance);
    let sans_adv = fonts.default_face().advance(fonts.default_face().glyph_index('H').unwrap()) as f32
        / 1000.0
        * 24.0;
    assert!(
        (shaped[1].0.advance - sans_adv).abs() > 0.01,
        "the serif glyph must advance differently from the sans one ({sans_adv})"
    );
}

#[test]
fn per_character_paragraph_style_still_justifies() {
    // A mix of sizes inside a justified paragraph: the extra space has to land
    // between the glyphs, not inside one of them.
    let bytes = sans();
    let fonts = FontSet::new("NotoSans-Regular", Face::parse(&bytes).expect("parse"));
    let runs = [
        RunStyle { len: 1, size: Some(20.0), ..RunStyle::default() },
        RunStyle { len: 1, size: Some(40.0), ..RunStyle::default() },
    ];
    let para = ParagraphStyle { justification: photocraft_text::area::Justification::JustifyLastLeft, ..ParagraphStyle::default() };
    let layout = layout_area_runs(
        &fonts,
        "HH",
        &runs,
        &base(),
        &para,
        240.0,
        120.0,
        (0.0, 0.0),
        0.76,
    );
    assert_eq!(layout.lines.len(), 1);
    let line = &layout.lines[0];
    assert_eq!(line.glyphs.len(), 2);
    let scaled = line.glyphs[1].advance / 2.0;
    let plain_line = layout_area_runs(
        &fonts,
        "HH",
        &[
            RunStyle { len: 1, size: Some(20.0), ..RunStyle::default() },
            RunStyle { len: 1, size: Some(20.0), ..RunStyle::default() },
        ],
        &base(),
        &ParagraphStyle { justification: photocraft_text::area::Justification::JustifyLastLeft, ..ParagraphStyle::default() },
        240.0,
        120.0,
        (0.0, 0.0),
        0.76,
    );
    let plain_adv = plain_line.lines[0].glyphs[1].advance;
    assert!(scaled > plain_adv * 0.95, "second glyph lost its own advance");
}

#[test]
fn per_character_underline_paints_only_its_run() {
    let plain = RunStyle { len: 1, ..RunStyle::default() };
    let under = RunStyle { len: 1, underline: true, ..RunStyle::default() };
    let p = render("HH", vec![plain, under], (240.0, 120.0));
    // The underline spans its run, so the split has to be found above it.
    let (g0, g1) = glyph_gap_above(&p, p.height.saturating_sub(6));
    let ink = |x0: u32, x1: u32| -> (u32, u32) {
        let mut count = 0;
        let mut bottom = 0;
        for y in 0..p.height {
            for x in x0..x1 {
                // A 1.2 px bar at 24 pt lands across two rows at ~50% coverage,
                // so decorations show up well below the "solid ink" threshold.
                if p.rgba[((y * p.width + x) * 4 + 3) as usize] > 32 {
                    count += 1;
                    bottom = bottom.max(y);
                }
            }
        }
        (count, bottom)
    };
    let left = ink(0, g0);
    let right = ink(g1, p.width);
    assert!(right.0 > left.0, "the underlined glyph has no extra ink: {} vs {}", right.0, left.0);
    assert!(
        right.1 >= left.1 + 2,
        "underline did not paint below the baseline: left bottom {} right bottom {}",
        left.1,
        right.1
    );
}

#[test]
fn per_character_strikethrough_sits_above_the_underline() {
    let strike = RunStyle { len: 1, strikethrough: true, ..RunStyle::default() };
    let under = RunStyle { len: 1, underline: true, ..RunStyle::default() };
    let strike_px = render("H", vec![strike], (160.0, 100.0));
    let under_px = render("H", vec![under], (160.0, 100.0));
    let bottom = |p: &LayerPixels| -> u32 {
        let mut b = 0;
        for y in 0..p.height {
            for x in 0..p.width {
                if p.rgba[((y * p.width + x) * 4 + 3) as usize] > 32 {
                    b = b.max(y);
                }
            }
        }
        b
    };
    assert!(
        bottom(&strike_px) < bottom(&under_px),
        "strikethrough must stay above the underline ({} vs {})",
        bottom(&strike_px),
        bottom(&under_px)
    );
}

/// Per-run fonts must reach the rasteriser, not just the layout: a second run
/// set in the bold face has to come out heavier than the same glyphs in the
/// regular face. This is the check that catches "advance from that face, but
/// outlines from the default face" — the mirror image of the horizontal-scale
/// bug the check pack found.
#[test]
fn per_run_font_paints_its_own_outlines() {
    let bold = RunStyle {
        len: 1,
        font: Some("NotoSans-Bold".into()),
        font_index: Some(1),
        size: Some(48.0),
        ..RunStyle::default()
    };
    let regular = RunStyle { len: 1, font_index: Some(0), size: Some(48.0), ..RunStyle::default() };
    let mixed_ed = engine_data(&["NotoSans-Regular", "NotoSans-Bold"], vec![regular.clone(), bold], "aA");
    let plain_ed =
        engine_data(&["NotoSans-Regular"], vec![regular, RunStyle { len: 1, size: Some(48.0), ..RunStyle::default() }], "aA");
    let store = store();
    let mixed = render_from_engine_data(&store, &mixed_ed, "aA", (0.0, 0.0), (0.0, 0.0)).expect("mixed");
    let plain = render_from_engine_data(&store, &plain_ed, "aA", (0.0, 0.0), (0.0, 0.0)).expect("plain");
    let ink = |p: &LayerPixels| -> u64 { p.rgba.chunks_exact(4).map(|px| u64::from(px[3])).sum() };
    let (mixed_ink, plain_ink) = (ink(&mixed), ink(&plain));
    assert!(
        mixed_ink > plain_ink + plain_ink / 100,
        "bold run did not paint heavier ({mixed_ink} vs {plain_ink})"
    );
}

/// Decorations take the run's fill colour. Photoshop paints the rule with the
/// text colour; painting black there is the same class of bug as a per-run
/// property that only reaches the layout.
#[test]
fn underline_takes_the_run_colour() {
    let red = [255u8, 0, 0];
    let under = RunStyle {
        len: 1,
        underline: true,
        color: Some(red),
        ..RunStyle::default()
    };
    let p = render("H", vec![under], (160.0, 100.0));
    let bottom_row = |p: &LayerPixels| -> (u32, [u8; 4]) {
        let mut best = (0u32, [0u8; 4]);
        for y in 0..p.height {
            for x in 0..p.width {
                let i = ((y * p.width + x) * 4) as usize;
                if p.rgba[i + 3] > 128 {
                    best = (y, [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2], p.rgba[i + 3]]);
                }
            }
        }
        best
    };
    let (_, px) = bottom_row(&p);
    assert!(px[0] > 200 && px[1] < 60 && px[2] < 60, "underline pixel is {px:?}, not red");
}

/// The live editor renders through the single-run path; a property that only
/// arrives in `Line::attrs` (vertical scale, faux italic, caps) used to be
/// dropped there, so its control did nothing. All of them must paint.
#[test]
fn single_run_render_spec_honours_attributes() {
    use photocraft_text::layer_render::{LayerTextStyle, RenderOptions};
    let store = store();
    let spec = LayerTextStyle {
        style: TextStyle { size: 48.0, ..TextStyle::default() },
        paragraph: ParagraphStyle::default(),
        box_size: (0.0, 0.0),
        origin: (0.0, 0.0),
        color: [0, 0, 0],
    };
    let render = |text: &str, patch: &dyn Fn(&mut RunStyle)| {
        let mut run = RunStyle { len: text.encode_utf16().count(), font: Some("NotoSans-Regular".into()), size: Some(48.0), ..RunStyle::default() };
        patch(&mut run);
        store
            .render_spec_with(text, &[run], &spec, RenderOptions { aa: None, use_atlas: false })
            .expect("render")
    };
    let base = render("H", &|_| {});
    let vscaled = render("H", &|r| r.vertical_scale = Some(2.0));
    assert!(
        vscaled.height > base.height + base.height / 2,
        "vertical scale did not stretch ({} -> {})",
        base.height,
        vscaled.height
    );
    let italic = render("H", &|r| r.faux_italic = true);
    assert!(italic.width > base.width + 4, "faux italic did not skew ({} -> {})", base.width, italic.width);
    let bold = render("H", &|r| r.faux_bold = true);
    let ink = |p: &LayerPixels| p.rgba.chunks_exact(4).map(|px| u64::from(px[3])).sum::<u64>();
    assert!(ink(&bold) > ink(&base), "faux bold did not thicken");
    let caps_plain = render("abc", &|_| {});
    let caps_on = render("abc", &|r| r.all_caps = true);
    assert!(
        caps_on.width > caps_plain.width + 10,
        "all caps did not change the glyphs ({} -> {})",
        caps_plain.width,
        caps_on.width
    );
    assert!(
        caps_on.height < caps_plain.height * 2,
        "caps must not wrap: {} rows vs {}",
        caps_on.height,
        caps_plain.height
    );
}

/// Multi-paragraph point text with per-run sizes, the shape the live editor
/// sends: two lines, the first run bigger, both baselines inside one canvas.
#[test]
fn multi_paragraph_point_text_with_per_run_sizes() {
    use photocraft_text::layer_render::{LayerTextStyle, RenderOptions};
    let store = store();
    let spec = LayerTextStyle {
        style: TextStyle { size: 48.0, ..TextStyle::default() },
        paragraph: ParagraphStyle::default(),
        box_size: (0.0, 0.0),
        origin: (0.0, 0.0),
        color: [0, 0, 0],
    };
    let text = "Hi\rall";
    let runs = [
        RunStyle { len: 2, font: Some("NotoSans-Regular".into()), size: Some(96.0), ..RunStyle::default() },
        RunStyle { len: 4, font: Some("NotoSans-Regular".into()), size: Some(24.0), ..RunStyle::default() },
    ];
    let pixels = store
        .render_spec_with(text, &runs, &spec, RenderOptions { aa: None, use_atlas: false })
        .expect("render");
    // Two lines of ink: there is a fully transparent band between them.
    let empty_row = |y: u32| (0..pixels.width).all(|x| pixels.rgba[((y * pixels.width + x) * 4 + 3) as usize] == 0);
    let gaps = (1..pixels.height - 1).filter(|&y| empty_row(y)).count();
    assert!(gaps > 2, "expected a blank band between the paragraphs, found {gaps} rows");
    // The 96 pt line is visibly taller than the 24 pt one.
    assert!(pixels.height > 100, "two paragraphs should exceed one line: {}", pixels.height);
}

#[test]
fn manual_kerning_moves_the_pair_by_the_stored_value() {
    // `Kerning` is 1/1000 em and only lives in `EngineData` (Photoshop has no
    // measured `Txt2` slot for it), so the rendered pixels are the oracle.
    let text = "AV";
    let plain = RunStyle { len: 2, auto_kerning: Some(false), kerning: Some(0), ..RunStyle::default() };
    let tight = RunStyle { len: 2, auto_kerning: Some(false), kerning: Some(-100), ..RunStyle::default() };
    let a = render(text, vec![plain], (300.0, 120.0));
    let b = render(text, vec![tight.clone()], (300.0, 120.0));
    let shift = a.width as i32 - b.width as i32;
    // -100/1000 em at the 12 pt default: 1.2 pt, i.e. one pixel at this scale.
    assert!((shift - 1).abs() <= 1, "ink width {0} vs {1}", a.width, b.width);
    // The writer must put the pair into the sheet the renderer reads back.
    let value = runs::to_engine_data(&[tight]);
    let run = value.get("RunArray").and_then(Value::as_array).and_then(|r| r.first()).unwrap();
    let sheet = run.get("StyleSheet").and_then(|s| s.get("StyleSheetData")).unwrap();
    assert_eq!(sheet.get("AutoKerning").and_then(Value::as_bool), Some(false));
    assert_eq!(sheet.get("Kerning").and_then(Value::as_i64), Some(-100));
}
