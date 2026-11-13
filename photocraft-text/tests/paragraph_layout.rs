//! Paragraph-level layout against Photoshop 20.0.2 (CC 2019) measurements.
//!
//! Every expected number in this file comes from a Photoshop artefact in the
//! repo; the comment next to it names the file. The engine never invents
//! numbers: unmeasured behaviour is marked `[INFERENCE]` in `src/typography.rs`
//! and `src/area.rs` and exercised only with explicit synthetic inputs here.
//!
//! Covered:
//! - paragraph indents (`out/exp/area_indent.pdf`, `corpus/box_*.pdf`),
//! - justification slack distribution over the paragraph word/letter/glyph
//!   spacing ranges (`corpus/box_justify.pdf` + its layout cache),
//! - the every-line composer's measured 0.0104 em tracking nudge
//!   (`out/composer_every.pdf`),
//! - hanging punctuation at both margins (`out/probe5/chars.pdf`,
//!   `out/probe5/h2.pdf`, `hang4..6.pdf`, `docs/hanging-punctuation.md`),
//! - kinsoku line breaking (synthetic CJK; the rule sets are JIS X 4051 and
//!   marked `[INFERENCE]` in `src/typography.rs`).

use std::path::PathBuf;

use photocraft_text::area::{
    AreaLayout, Justification, ParagraphStyle, layout_area, layout_area_paragraphs_with_advances,
};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::typography;

fn font_path(name: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    PathBuf::from(home)
        .join(".local/share/photocraft/wine/drive_c/windows/Fonts")
        .join(name)
}

fn with_face<R>(name: &str, f: impl FnOnce(&Face<'_>) -> R) -> R {
    let bytes = std::fs::read(font_path(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let face = Face::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    f(&face)
}

fn noto() -> &'static str {
    "NotoSans-Regular.ttf"
}

/// Lays out one paragraph with the Noto Sans shape used by every probe below.
fn lay(
    face: &Face<'_>,
    text: &str,
    para: &ParagraphStyle,
    size: f32,
    leading: Option<f32>,
    width: f32,
    height: f32,
    origin: (f32, f32),
) -> AreaLayout {
    let style = TextStyle {
        size,
        leading,
        ..Default::default()
    };
    layout_area(face, text, &style, para, width, height, origin, 0.76)
}

fn chars(line: &photocraft_text::area::Line) -> String {
    line.glyphs
        .iter()
        .map(|g| g.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// x of the last glyph that is not a break space, relative to the line start.
fn content_end(line: &photocraft_text::area::Line) -> f32 {
    let start = line.glyphs.first().map_or(0.0, |g| g.x);
    let last = line.glyphs.iter().rposition(|g| g.ch != ' ');
    last.map_or(0.0, |i| line.glyphs[i].x + line.glyphs[i].advance - start)
}

#[derive(serde::Deserialize)]
struct MeasuredGlyph {
    ch: String,
    x: f32,
    advance: Option<f32>,
}

#[derive(serde::Deserialize)]
struct GlyphFixture {
    #[allow(dead_code)]
    origin: f32,
    text: String,
    em_px: f32,
    font: String,
    size: f32,
    box_width: f32,
    box_height: f32,
    box_left: f32,
    glyphs: Vec<MeasuredGlyph>,
}

fn load_fixture(id: &str) -> GlyphFixture {
    let path = PathBuf::from("tests/data").join(format!("{id}.json"));
    serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}")),
    )
    .expect("fixture")
}

// ---------------------------------------------------------------------------
// 1. Paragraph geometry: indents, space before/after
// ---------------------------------------------------------------------------

#[test]
fn first_line_and_left_indent_match_area_indent_pdf() {
    // out/exp/area_indent.pdf: two paragraphs, Txt2 paragraph sheet
    // `/5/1 24` (first-line), `/5/2 18` (left), `/5/3 12` (right); the layer is
    // at x = 19.998 and both paragraph first lines start at x = 61.998 =
    // 19.998 + 18 + 24. Photoshop's own recomposition in
    // out/exp/area_indent_resaved.psd stores the same glyph-run origin 42.0 and
    // baselines 27.3598 / 70.5598.
    with_face(noto(), |face| {
        let text = "Hamburg font\rAVATAR text";
        let para = ParagraphStyle {
            first_line_indent: 24.0,
            start_indent: 18.0,
            end_indent: 12.0,
            ..Default::default()
        };
        let out = lay(face, text, &para, 36.0, None, 430.0, 200.0, (19.998, 0.0));
        assert_eq!(out.lines.len(), 2);
        for (i, line) in out.lines.iter().enumerate() {
            let x = line.glyphs[0].x;
            assert!(
                (x - 61.998).abs() < 0.002,
                "paragraph {i} starts at {x}, Photoshop 61.998"
            );
        }
        // Leading is 36 * 1.2 = 43.2; our y axis is PDF-like (decreasing down).
        let step = out.lines[0].baseline - out.lines[1].baseline;
        assert!((step - 43.2).abs() < 1e-3, "baseline step {step} != 43.2");
        assert!(
            (out.lines[0].baseline - 27.3598).abs() < 1e-3,
            "first baseline {}",
            out.lines[0].baseline
        );
    });
}

#[test]
fn right_indent_narrows_the_measure() {
    // [INFERENCE] no corpus layer wraps against a right indent (docs/txt2-map.md
    // marks `/5/3` NEEDS PROBE); the model is the Txt2 sheet's single indentation
    // sum, so a right indent of 60 must remove exactly one 36 pt word.
    with_face(noto(), |face| {
        let text = "aaa bbb ccc";
        let base = ParagraphStyle::default();
        let wide = lay(
            face,
            text,
            &base,
            36.0,
            Some(43.2),
            1000.0,
            300.0,
            (0.0, 0.0),
        );
        assert_eq!(wide.lines.len(), 1);
        let full = content_end(&wide.lines[0]);
        // Shrink the box so the last word no longer fits, but a right indent of
        // the same amount behaves identically.
        let width = full - 5.0;
        let narrow = lay(
            face,
            text,
            &base,
            36.0,
            Some(43.2),
            width,
            300.0,
            (0.0, 0.0),
        );
        let indented = lay(
            face,
            text,
            &ParagraphStyle {
                end_indent: 5.0,
                ..Default::default()
            },
            36.0,
            Some(43.2),
            width + 5.0,
            300.0,
            (0.0, 0.0),
        );
        assert_eq!(narrow.lines.len(), indented.lines.len());
        assert!(narrow.lines.len() > 1, "the test text has to wrap");
        for (a, b) in narrow.lines.iter().zip(&indented.lines) {
            assert_eq!(chars(a), chars(b));
        }
    });
}

#[test]
fn space_before_after_stack_between_paragraphs() {
    // [INFERENCE] measured corpus paragraphs all store SpaceBefore = 0 and
    // SpaceAfter = 0 and the CC 2019 DOM setters are no-ops
    // (out/probe5/mD.json, docs/txt2-map.md), so this asserts the model:
    // space_after(previous) + space_before(next) is added to the one leading
    // that already separates two paragraphs.
    with_face(noto(), |face| {
        let para = ParagraphStyle {
            space_before: 12.0,
            space_after: 18.0,
            ..Default::default()
        };
        let out = lay(
            face,
            "a\rb",
            &para,
            36.0,
            Some(43.2),
            1000.0,
            1000.0,
            (0.0, 0.0),
        );
        assert_eq!(out.lines.len(), 2);
        let first_off = 0.76 * 36.0;
        // Photoshop does not add `SpaceBefore` above the first paragraph of a
        // frame: layer `05 area indents spacing` stores `SpaceBefore 12` and its
        // first ink row still matches the `TySh` baseline (`10_all_features.psd`,
        // render_psd rect-exact, dy 0).
        assert!((out.lines[0].baseline - first_off).abs() < 1e-3);
        // 43.2 leading + 18 space_after + 12 space_before.
        let step = out.lines[0].baseline - out.lines[1].baseline;
        assert!(
            (step - (43.2 + 18.0 + 12.0)).abs() < 1e-3,
            "paragraph step {step}"
        );
    });
    // Zero spacing keeps the plain one-leading step used by every corpus layer.
    with_face(noto(), |face| {
        let out = lay(
            face,
            "a\rb",
            &ParagraphStyle::default(),
            36.0,
            Some(43.2),
            1000.0,
            1000.0,
            (0.0, 0.0),
        );
        let step = out.lines[0].baseline - out.lines[1].baseline;
        assert!((step - 43.2).abs() < 1e-3);
    });
}

// ---------------------------------------------------------------------------
// 2. Justification: Photoshop's measured slack distribution
// ---------------------------------------------------------------------------

#[test]
fn justify_matches_box_justify_glyph_positions_and_line_ends() {
    // corpus/box_justify.psd+pdf: Justification 6 (FULLYJUSTIFIED), 48 pt
    // Noto Sans in a 430 x 200 box, paragraph sheet WordSpacing
    // [0.8 1.0 1.33], LetterSpacing [0 0 0], GlyphSpacing [1 1 1]. Three lines
    // are visible (the fourth baseline falls below the box): every line is
    // stretched to the measure by distributing the slack over the three
    // interior spaces only; the line-final space keeps its natural advance in
    // the cache and is exported with advance 0.
    let fx = load_fixture("box_justify");
    let bytes = std::fs::read(font_path(&format!("{}.ttf", fx.font))).expect("font");
    let face = Face::parse(&bytes).expect("parse");
    let style = TextStyle {
        size: fx.em_px,
        ..Default::default()
    };
    let para = ParagraphStyle {
        justification: Justification::JustifyAll,
        ..Default::default()
    };
    // The PDF's em is 47.997 pt for a 48 pt font; scale the measure like the
    // PDF exporter did (same rule as tests/layout_corpus.rs).
    let width = fx.box_width * fx.em_px / fx.size;
    let out = layout_area(
        &face,
        &fx.text,
        &style,
        &para,
        width,
        fx.box_height,
        (fx.box_left, 0.0),
        0.76,
    );
    let ours: Vec<_> = out.lines.iter().flat_map(|l| l.glyphs.iter()).collect();
    assert_eq!(ours.len(), fx.glyphs.len(), "glyph count");
    let mut max_dx = 0.0f32;
    for (i, (g, m)) in ours.iter().zip(&fx.glyphs).enumerate() {
        assert_eq!(g.ch.to_string(), m.ch, "glyph {i}");
        let dx = (g.x - m.x).abs();
        max_dx = max_dx.max(dx);
        assert!(
            dx < 0.02,
            "glyph {i} {:?}: x {:.5} vs Photoshop {:.5}",
            g.ch,
            g.x,
            m.x
        );
    }
    // The requirement for justified lines: every visible line ends at the box's
    // right edge within 0.01 pt of Photoshop's PDF.
    for (k, line) in out.lines.iter().enumerate() {
        let m_end = fx
            .glyphs
            .iter()
            .filter(|m| (m.x - line.glyphs[0].x).abs() < 1e-3 || m.x >= line.glyphs[0].x)
            .last()
            .map(|m| m.x + m.advance.unwrap_or(0.0));
        let _ = m_end;
        let ours_end = line
            .glyphs
            .iter()
            .rposition(|g| g.ch != ' ')
            .map(|i| line.glyphs[i].x + line.glyphs[i].advance);
        let tgt = fx.box_left + width;
        let d = (ours_end.unwrap() - tgt).abs();
        assert!(d < 0.01, "line {k} ends at {:?}, box right {tgt}", ours_end);
    }
    eprintln!("box_justify: worst glyph x diff {max_dx:.5}");
}

#[test]
fn justify_with_defaults_moves_spaces_only() {
    // The measured box_justify distribution: with the corpus default paragraph
    // sheet the letter and glyph spacing ranges are zero, so the slack can only
    // reach the spaces, and every space's advance grows by the same factor
    // (cache `/1` adjustments 2.30472 / .25955 / .66346 -> PDF space advances
    // 41.2395 / 15.7180 / 20.7582). Letters keep their metrics exactly.
    let fx = load_fixture("box_justify");
    let bytes = std::fs::read(font_path(&format!("{}.ttf", fx.font))).expect("font");
    let face = Face::parse(&bytes).expect("parse");
    let style = TextStyle {
        size: fx.em_px,
        ..Default::default()
    };
    let para = ParagraphStyle {
        justification: Justification::JustifyAll,
        ..Default::default()
    };
    let width = fx.box_width * fx.em_px / fx.size;
    let out = layout_area(
        &face,
        &fx.text,
        &style,
        &para,
        width,
        fx.box_height,
        (fx.box_left, 0.0),
        0.76,
    );
    // Lines 1 and 2 contain interior spaces; their advances must be the
    // Photoshop PDF values (41.2395 and 15.7180 for this text).
    // The PDF prints the two interior spaces of line 1 at 41.2395 and the
    // break space with no advance (it is a no-op at the end of the line).
    let first_spaces: Vec<f32> = out.lines[0]
        .glyphs
        .iter()
        .filter(|g| g.ch == ' ')
        .map(|g| g.advance)
        .collect();
    assert_eq!(
        first_spaces.len(),
        3,
        "line 1: two stretched spaces and the break space"
    );
    for (i, a) in first_spaces[..2].iter().enumerate() {
        assert!(
            (a - 41.2395).abs() < 0.01,
            "line 1 space {i} advance {a} != Photoshop 41.2395"
        );
    }
    assert!(
        (first_spaces[2] - 12.47974).abs() < 0.01,
        "the break space is not stretched"
    );
    // Line 2 prints three spaces at 15.7180.
    let second: Vec<f32> = out.lines[1]
        .glyphs
        .iter()
        .filter(|g| g.ch == ' ')
        .map(|g| g.advance)
        .collect();
    for (i, a) in second.iter().enumerate().take(3) {
        assert!(
            (a - 15.7180).abs() < 0.02,
            "line 2 space {i} advance {a} != Photoshop 15.7180"
        );
    }
    // Letters are untouched: compare against the same text laid out ragged.
    let ragged = layout_area(
        &face,
        &fx.text,
        &style,
        &ParagraphStyle::default(),
        width,
        fx.box_height,
        (fx.box_left, 0.0),
        0.76,
    );
    for (jl, rl) in out.lines.iter().zip(&ragged.lines) {
        for (jg, rg) in jl
            .glyphs
            .iter()
            .zip(&rl.glyphs)
            .filter(|(g, _)| g.ch != ' ')
        {
            assert!(
                (jg.advance - rg.advance).abs() < 1e-4,
                "{:?} letter advance moved by justify",
                jg.ch
            );
        }
    }
}

#[test]
fn justify_uses_letter_spacing_range_when_the_paragraph_defines_one() {
    // Photoshop's paragraph sheet gives word/letter/glyph spacing as
    // [min desired max] (docs/txt2-map.md /5/17, /5/18, /5/19). No Photoshop
    // probe reached the engine with non-default word/letter values (the DOM
    // setters were no-ops, out/probe5/mD.json), so this test pins the
    // *mechanism*: with a non-zero letter-spacing maximum the slack is shared
    // between spaces and letters in proportion to their allowed ranges, and the
    // line still fills the measure.
    with_face(noto(), |face| {
        let text = "ab cd";
        let size = 48.0;
        let width = 300.0;
        let base = ParagraphStyle {
            justification: Justification::JustifyAll,
            ..Default::default()
        };
        let plain = lay(face, text, &base, size, None, width, 100.0, (0.0, 0.0));
        let letters_still: f32 = plain.lines[0].glyphs[0].advance;
        let para = ParagraphStyle {
            letter_spacing: [0.0, 0.0, 0.05],
            ..base
        };
        let spaced = lay(face, text, &para, size, None, width, 100.0, (0.0, 0.0));
        let letters_moved = spaced.lines[0].glyphs[0].advance;
        assert!(
            letters_moved > letters_still + 1.0,
            "letter spacing range did not reach the glyphs"
        );
        assert!(
            (content_end(&spaced.lines[0]) - width).abs() < 0.01,
            "justified line did not fill the box"
        );
        // The desired letter spacing (0.0 here) is the base; only the range is
        // used, so the fill is exact.
        assert!(content_end(&plain.lines[0]) > width - 0.01);
    });
}

#[test]
fn every_line_composer_adds_the_measured_tracking_nudge() {
    // out/composer_every.pdf vs out/composer_single.pdf: the same paragraph set
    // with the single-line and the every-line composer. Every glyph whose next
    // glyph is not a space advances by +0.2496 pt at 24 pt = 0.0104 em, and the
    // layer cache (out/composer_every.psd) stores `/3 .24959` on each line.
    let path = PathBuf::from("tests/data/composer_every.json");
    #[derive(serde::Deserialize)]
    struct Fx {
        text: String,
        font: String,
        size: f32,
        leading: f32,
        box_width: f32,
        box_height: f32,
        box_left: f32,
        lines: Vec<String>,
    }
    let fx: Fx = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let bytes = std::fs::read(font_path(&format!("{}.ttf", fx.font))).unwrap();
    let face = Face::parse(&bytes).unwrap();
    // Photoshop's PDF glyph pens for line 1 of out/composer_every.pdf.
    const PHOTOSHOP_X: [f32; 17] = [
        19.998, 34.76945, 49.85269, 64.62414, 73.53861, 88.30982, 100.05665, 115.14013, 129.91134,
        144.67294, 150.91372, 165.92496, 175.60777, 189.39509, 203.11017, 216.1774, 227.67436,
    ];
    let every = lay(
        &face,
        &fx.text,
        &ParagraphStyle {
            every_line_composer: true,
            ..Default::default()
        },
        fx.size,
        Some(fx.leading),
        fx.box_width,
        fx.box_height,
        (fx.box_left, 0.0),
    );
    let lines: Vec<String> = every.lines.iter().map(chars).collect();
    assert_eq!(lines, fx.lines, "every-line composer breaks");
    for (i, (g, x)) in every.lines[0].glyphs.iter().zip(PHOTOSHOP_X).enumerate() {
        eprintln!(
            "glyph {i} {:?} ours {:.5} photoshop {x:.5} diff {:.5}",
            g.ch,
            g.x,
            g.x - x
        );
        assert!(
            (g.x - x).abs() < 0.04,
            "line 1 glyph {i} {:?}: x {:.5} vs Photoshop {x:.5}",
            g.ch,
            g.x
        );
    }
    // The nudge itself: same layout without the every-line composer keeps the
    // metrics advances, so the difference on the first letter is exactly
    // size * 0.0104.
    let single = lay(
        &face,
        &fx.text,
        &ParagraphStyle {
            justification: Justification::Left,
            ..Default::default()
        },
        fx.size,
        Some(fx.leading),
        fx.box_width,
        fx.box_height,
        (fx.box_left, 0.0),
    );
    let delta = every.lines[0].glyphs[0].advance - single.lines[0].glyphs[0].advance;
    assert!(
        (delta - fx.size * typography::EVERY_LINE_TRACK).abs() < 1e-3,
        "nudge {delta}"
    );
    // The nudge never lands on a space or after one.
    let space = every.lines[0].glyphs.iter().find(|g| g.ch == ' ').unwrap();
    let single_space = single.lines[0].glyphs.iter().find(|g| g.ch == ' ').unwrap();
    assert!(
        (space.advance - single_space.advance).abs() < 1e-3,
        "space advance changed by the nudge"
    );
}

// ---------------------------------------------------------------------------
// 3. Hanging punctuation
// ---------------------------------------------------------------------------

#[test]
fn left_hanging_prefix_matches_chars_pdf() {
    // out/probe5/chars.pdf: Noto Sans 36 pt, box left x = 20, box 400 wide,
    // hangingPunctuation = true, text "<punct>AAAAA". The punctuation's pen sits
    // left of the margin and the next glyph follows in the flow. Measured
    // (pen shift, PDF x0): “/” 10.4043, ‘/’ 3.7801, «/» 18.3241, - 11.592,
    // – 9.0003, — 18.0005, ,/. 9.6479, ;/: 4.8241, * 9.9183, … 14.238; and
    // 『 and （ 17.2805 through Photoshop's fallback glyph (advance 35.281).
    // ( [ { ! ? • do not hang (shift 0).
    let measured: &[(char, f32)] = &[
        ('\u{201C}', 10.4043),
        ('\u{201D}', 10.4043),
        ('\u{2018}', 3.7801),
        ('\u{2019}', 3.7801),
        ('\u{00AB}', 18.3241),
        ('\u{00BB}', 18.3241),
        ('-', 11.5920),
        ('\u{2013}', 9.0003),
        ('\u{2014}', 18.0005),
        (',', 9.6479),
        ('.', 9.6479),
        (';', 4.8241),
        (':', 4.8241),
        ('*', 9.9183),
        ('\u{2026}', 14.2380),
    ];
    let no_hang: &[(char, f32)] = &[
        ('(', 10.7999),
        ('[', 11.8440),
        ('{', 13.6800),
        ('!', 9.6843),
        ('?', 15.6241),
    ];
    with_face(noto(), |face| {
        let para = ParagraphStyle {
            hanging: true,
            ..Default::default()
        };
        for &(ch, shift) in measured {
            let text = format!("{ch}AAAAA");
            let out = lay(
                face,
                &text,
                &para,
                36.0,
                Some(43.2),
                400.0,
                200.0,
                (20.0, 0.0),
            );
            assert_eq!(out.lines.len(), 1, "{ch:?}");
            let line = &out.lines[0];
            let got = 20.0 - line.glyphs[0].x;
            assert!(
                (got - shift).abs() < 0.02,
                "{ch:?} hangs {got:.4} but Photoshop measures {shift:.4} (chars.pdf)"
            );
            // The rest of the line follows in flow: the next glyph starts at
            // margin + (advance - hang).
            let paid = line.glyphs[0].advance - got;
            assert!(
                (line.glyphs[1].x - (20.0 + paid)).abs() < 0.01,
                "{ch:?} next glyph"
            );
        }
        for &(ch, advance) in no_hang {
            let text = format!("{ch}AAAAA");
            let out = lay(
                face,
                &text,
                &para,
                36.0,
                Some(43.2),
                400.0,
                200.0,
                (20.0, 0.0),
            );
            let line = &out.lines[0];
            let got = 20.0 - line.glyphs[0].x;
            assert!(
                got.abs() < 1e-3,
                "{ch:?} must not hang (measured shift 0), got {got}"
            );
            assert!(
                (line.glyphs[1].x - (20.0 + advance)).abs() < 0.01,
                "{ch:?} advances normally"
            );
        }
    });
}

#[test]
fn right_hanging_suffix_does_not_count_in_the_line_fit() {
    // out/probe5/hang5.pdf / hang6.pdf: "AAAAA BBBBB," at Noto Sans 36 pt needs
    // a box of 251 pt without hanging (content 241.0 + comma 9.648) but fits a
    // 242 pt box with hanging, where the comma is drawn at pen 261.02 for a
    // 242 pt box at x = 20 (right margin 262) and its ink ends at 267.7.
    with_face(noto(), |face| {
        let hanging = ParagraphStyle {
            hanging: true,
            ..Default::default()
        };
        let tight = 242.0;
        let out = lay(
            face,
            "AAAAA BBBBB,",
            &hanging,
            36.0,
            Some(43.2),
            tight,
            400.0,
            (20.0, 0.0),
        );
        assert_eq!(
            out.lines.len(),
            1,
            "the comma hangs, so the line fits 242 pt"
        );
        let line = &out.lines[0];
        let comma = line.glyphs.iter().rfind(|g| g.ch == ',').expect("comma");
        // Content excluding the comma is 241.02 pt (measured threshold), so the
        // comma pen is at x = 20 + 241.02.
        assert!(
            (comma.x - 261.02).abs() < 0.05,
            "comma pen {:.3}, Photoshop 261.02",
            comma.x
        );
        assert!(
            comma.x + comma.advance > 20.0 + tight,
            "comma must overflow the right margin"
        );

        // Without hanging the same box cannot hold the whole string.
        let ragged = lay(
            face,
            "AAAAA BBBBB,",
            &ParagraphStyle::default(),
            36.0,
            Some(43.2),
            tight,
            400.0,
            (20.0, 0.0),
        );
        assert!(
            ragged.lines.len() > 1,
            "without hanging the comma has to wrap"
        );

        // ; is not a right-hang character: same break as without hanging.
        let semi_hanging = lay(
            face,
            "AAAAA BBBBB;",
            &hanging,
            36.0,
            Some(43.2),
            tight,
            400.0,
            (20.0, 0.0),
        );
        let semi_ragged = lay(
            face,
            "AAAAA BBBBB;",
            &ParagraphStyle::default(),
            36.0,
            Some(43.2),
            tight,
            400.0,
            (20.0, 0.0),
        );
        assert_eq!(
            semi_hanging.lines.len(),
            semi_ragged.lines.len(),
            "';' must not hang"
        );
    });
}

#[test]
fn justified_line_hangs_the_comma_outside_the_measure() {
    // out/probe5/h2.pdf case 1 (hanging, justification "full" = FULLYJUSTIFIED):
    // Noto Sans 36 pt, 300 pt box at x = 19.999. Line 3 "over the lazy dog, "
    // justifies the content *without* the comma to the right margin (three
    // interior spaces at 13.03812 = natural 9.36026 + 3.67786) and then draws
    // the comma at pen 320.009 (margin + 300.010), overflowing by its advance
    // 9.648. The last line is justified too (period hung at margin + 300.011;
    // its two spaces measure 51.328), proving code 6 is "justify all".
    with_face(noto(), |face| {
        let text = "He said, the quick brown fox jumps over the lazy dog, and then left.";
        let para = ParagraphStyle {
            hanging: true,
            justification: Justification::JustifyAll,
            ..Default::default()
        };
        let out = lay(
            face,
            text,
            &para,
            36.0,
            Some(43.2),
            300.0,
            400.0,
            (19.999, 0.0),
        );
        let texts: Vec<String> = out.lines.iter().map(chars).collect();
        assert_eq!(
            texts,
            [
                "He said, the quick",
                "brown fox jumps",
                "over the lazy dog,",
                "and then left."
            ],
            "hanging changes the break positions (docs/hanging-punctuation.md)"
        );
        let line3 = &out.lines[2];
        let comma = line3.glyphs.iter().rfind(|g| g.ch == ',').unwrap();
        let rel = comma.x - line3.glyphs[0].x;
        assert!(
            (rel - 300.010).abs() < 0.02,
            "comma pen {rel:.3} vs Photoshop 300.010"
        );
        // Line 3's interior spaces carry the whole slack.
        for g in line3.glyphs.iter().filter(|g| g.ch == ' ') {
            if g.x + g.advance < comma.x {
                assert!(
                    (g.advance - 13.03812).abs() < 0.02,
                    "space advance {:.5}",
                    g.advance
                );
            }
        }
        let line4 = &out.lines[3];
        let period = line4.glyphs.iter().rfind(|g| g.ch == '.').unwrap();
        let rel4 = period.x - line4.glyphs[0].x;
        assert!(
            (rel4 - 300.011).abs() < 0.02,
            "period pen {rel4:.3} vs Photoshop 300.011"
        );
        for g in line4.glyphs.iter().filter(|g| g.ch == ' ') {
            assert!(
                (g.advance - 51.328).abs() < 0.03,
                "last line space {:.4} vs Photoshop 51.328",
                g.advance
            );
        }

        // Hanging off: the comma takes part in the justification, so it sits
        // well inside the right margin (no hang).
        let flat = ParagraphStyle {
            hanging: false,
            justification: Justification::JustifyAll,
            ..Default::default()
        };
        let off = lay(
            face,
            text,
            &flat,
            36.0,
            Some(43.2),
            300.0,
            400.0,
            (19.999, 0.0),
        );
        let line3off = &off.lines[2];
        let comma_off = line3off.glyphs.iter().rfind(|g| g.ch == ',').unwrap();
        let rel_off = comma_off.x - line3off.glyphs[0].x;
        assert!(
            rel_off < 295.0,
            "without hanging the comma stays inside, got {rel_off}"
        );
    });
}

// ---------------------------------------------------------------------------
// 4. Kinsoku line breaking
// ---------------------------------------------------------------------------

#[test]
fn kinsoku_keeps_cjk_closing_punctuation_off_the_line_start() {
    // [INFERENCE] rule set: JIS X 4051 (src/typography.rs `kinsoku`). No
    // Photoshop probe measured the CJK break rules; the test asserts the
    // invariant, not Photoshop's exact break points.
    with_face("NotoSansCJK-Regular.ttc", |face| {
        let text = "\u{300C}\u{3053}\u{308C}\u{306F}\u{30C6}\u{30B9}\u{30C8}\u{3067}\u{3059}\u{3002}\u{300D}\u{5F7C}\u{306F}\u{8A00}\u{3063}\u{305F}\u{3002}\u{300C}\u{3082}\u{3046}\u{4E00}\u{5EA6}\u{3001}\u{30C6}\u{30B9}\u{30C8}\u{3057}\u{307E}\u{3059}\u{3002}\u{300D}";
        let para = ParagraphStyle::default();
        let out = lay(face, text, &para, 36.0, None, 260.0, 600.0, (0.0, 0.0));
        assert!(out.lines.len() >= 3, "the CJK text must wrap");
        for (i, line) in out.lines.iter().enumerate() {
            let first = line.glyphs.iter().find(|g| g.ch != ' ').map(|g| g.ch);
            if let Some(ch) = first {
                assert!(
                    typography::kinsoku::may_start_line(ch),
                    "line {i} starts with forbidden closing punctuation {ch:?}"
                );
            }
            let last = line.glyphs.iter().rev().find(|g| g.ch != ' ').map(|g| g.ch);
            if let Some(ch) = last {
                assert!(
                    typography::kinsoku::may_end_line(ch),
                    "line {i} ends with opening punctuation {ch:?}"
                );
            }
        }
    });
}

#[test]
fn kinsoku_pulls_a_latin_closing_mark_onto_the_previous_line() {
    // [INFERENCE] Latin punctuation set: a line may not start with ')' when
    // hanging is off. The breaker pulls it onto the previous line (standard
    // push-in) instead of leaving it stranded at the next line's start.
    with_face(noto(), |face| {
        let text = "aaa ) bbb";
        let style = TextStyle {
            size: 36.0,
            leading: Some(43.2),
            ..Default::default()
        };
        // Just wide enough for "aaa " but not for "aaa ) ".
        let narrow = lay(
            face,
            "aaa",
            &ParagraphStyle::default(),
            36.0,
            Some(43.2),
            1000.0,
            200.0,
            (0.0, 0.0),
        );
        let style_w = content_end(&narrow.lines[0]);
        let para = ParagraphStyle::default();
        let out = layout_area(
            &face,
            text,
            &style,
            &para,
            style_w + 2.0,
            200.0,
            (0.0, 0.0),
            0.76,
        );
        assert!(out.lines.len() > 1, "text must wrap");
        for (i, line) in out.lines.iter().enumerate() {
            if let Some(ch) = line.glyphs.first().map(|g| g.ch) {
                assert!(may_start_glyph(ch), "line {i} starts with {ch:?}");
            }
        }
    });
}

fn may_start_glyph(ch: char) -> bool {
    typography::kinsoku::may_start_line(ch) || ch == ' '
}

#[test]
fn hanging_and_kinsoku_tables_are_measured_data() {
    // The tables are the contract between the layout and the probes: guard the
    // measured members so a refactor cannot silently change Photoshop behaviour.
    for ch in [',', '.', '\u{201D}', '\u{2019}', '\u{00BB}', '"', '\'', '-'] {
        assert_eq!(
            typography::right_hang_fit(ch),
            Some(0.0),
            "{ch:?} right-hang"
        );
        assert!(typography::is_right_hang(ch));
    }
    for ch in [
        ';', ':', '!', '?', ')', ']', '}', '\u{2013}', '\u{2014}', '\u{2026}', '%', '/',
    ] {
        assert_eq!(
            typography::right_hang_fit(ch),
            None,
            "{ch:?} must not right-hang"
        );
    }
    for ch in ['\u{201C}', '\u{2018}', '\u{00AB}', ',', '.', '-'] {
        assert_eq!(
            typography::left_hang_fit(ch),
            Some(0.0),
            "{ch:?} full left-hang"
        );
    }
    for ch in [';', ':', '*', '\u{2026}', '\u{2013}', '\u{2014}'] {
        assert_eq!(
            typography::left_hang_fit(ch),
            Some(0.5),
            "{ch:?} half left-hang"
        );
    }
    for ch in ['(', '[', '{', '!', '?', '\u{2022}'] {
        assert_eq!(
            typography::left_hang_fit(ch),
            None,
            "{ch:?} must not left-hang"
        );
    }
}

/// Txt2's composed cache can hand the layout one advance correction per shaped
/// glyph (the every-line composer's per-run delta or optical kerning). The
/// corrections move the pen, so a later glyph starts `sum(delta)` further along,
/// and a wide enough correction changes line breaks because the fit test sees
/// the corrected widths.
#[test]
fn per_glyph_advances_shift_the_pen_and_the_breaks() {
    let para = ParagraphStyle::default();
    let style = TextStyle {
        size: 10.0,
        ..Default::default()
    };
    with_face(noto(), |face| {
        let plain = layout_area(face, "aa bb", &style, &para, 1.0e7, 1.0e7, (0.0, 0.0), 0.8);
        let glyphs = |l: &AreaLayout| -> Vec<(char, f32)> {
            l.lines[0].glyphs.iter().map(|g| (g.ch, g.x)).collect()
        };
        let base = glyphs(&plain);
        let advanced = layout_area_paragraphs_with_advances(
            face,
            "aa bb",
            &style,
            std::slice::from_ref(&para),
            1.0e7,
            1.0e7,
            (0.0, 0.0),
            0.8,
            &[2.0, 2.0, 0.0, 0.0, 0.0],
        );
        let moved = glyphs(&advanced);
        for (i, ((_, x0), (_, x1))) in base.iter().zip(&moved).enumerate() {
            let shift: f32 = [2.0, 2.0, 0.0, 0.0, 0.0][..i].iter().sum();
            assert!((x1 - x0 - shift).abs() < 1.0e-3, "{i}: {x0} -> {x1}");
        }
        // A measure one point wider than the natural line: widening every
        // advance makes the line overflow and break.
        let width = plain.lines[0]
            .glyphs
            .iter()
            .map(|g| g.x + g.advance)
            .fold(0.0f32, f32::max);
        let narrow = |advances: &[f32]| {
            layout_area_paragraphs_with_advances(
                face,
                "aa bb",
                &style,
                std::slice::from_ref(&para),
                width + 1.0,
                1.0e7,
                (0.0, 0.0),
                0.8,
                advances,
            )
            .lines
            .len()
        };
        assert_eq!(narrow(&[]), 1);
        assert!(narrow(&[10.0; 5]) >= 2);
    });
}

/// Photoshop's composed cache stores, per glyph, the spacing CoolType applied
/// on top of the metrics advance (tracking + optical kerning + composer
/// nudges). The engine subtracts the tracking it already baked into the
/// advance before adding the correction, so feeding the cache's tracking value
/// back must not move the pen, while feeding zero must remove the tracking.
#[test]
fn composed_advances_are_added_on_top_of_untracked_advances() {
    let para = ParagraphStyle::default();
    let style = TextStyle {
        size: 10.0,
        tracking: 100,
        ..Default::default()
    };
    with_face(noto(), |face| {
        let plain = layout_area(face, "aa bb", &style, &para, 1.0e7, 1.0e7, (0.0, 0.0), 0.8);
        let measure = |l: &AreaLayout| {
            l.lines[0]
                .glyphs
                .iter()
                .map(|g| (g.x, g.advance))
                .collect::<Vec<_>>()
        };
        let base = measure(&plain);
        // `tracking / 1000 * size` = 1 pt per glyph: the value CoolType would
        // record for this run.
        let same = layout_area_paragraphs_with_advances(
            face,
            "aa bb",
            &style,
            std::slice::from_ref(&para),
            1.0e7,
            1.0e7,
            (0.0, 0.0),
            0.8,
            &[1.0; 5],
        );
        for (i, (a, b)) in base.iter().zip(measure(&same).iter()).enumerate() {
            assert!((a.0 - b.0).abs() < 1.0e-3, "x {i}: {} -> {}", a.0, b.0);
            assert!(
                (a.1 - b.1).abs() < 1.0e-3,
                "advance {i}: {} -> {}",
                a.1,
                b.1
            );
        }
        // Zero corrections mean "Photoshop added no spacing", so the engine's
        // own tracking is dropped: the line becomes 5 pt narrower.
        let untracked = layout_area_paragraphs_with_advances(
            face,
            "aa bb",
            &style,
            std::slice::from_ref(&para),
            1.0e7,
            1.0e7,
            (0.0, 0.0),
            0.8,
            &[0.0; 5],
        );
        let end = |l: &AreaLayout| {
            l.lines[0]
                .glyphs
                .iter()
                .map(|g| g.x + g.advance)
                .fold(0.0f32, f32::max)
        };
        assert!((end(&plain) - end(&untracked) - 5.0).abs() < 1.0e-2);
    });
}
