use photocraft_text::font::Face;
use photocraft_text::layout::GlyphPos;
use photocraft_text::typography::*;

fn font_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join(".local/share/photocraft/wine/drive_c/windows/Fonts")
        .join(name)
}

fn load(name: &str) -> Vec<u8> {
    std::fs::read(font_path(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn glyph(ch: char, advance: f32) -> GlyphPos {
    GlyphPos {
        gid: 0,
        ch,
        x: 0.0,
        y: 0.0,
        advance,
        nominal_advance: advance,
    }
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn auto_leading_table_matches_ratio() {
    for row in AUTO_LEADING_TABLE {
        if let Some(l) = row.leading {
            assert!(
                approx(l, row.size * AUTO_LEADING),
                "{} @ {}: {} != {}",
                row.font,
                row.size,
                l,
                row.size * AUTO_LEADING
            );
        }
    }
    let measured = AUTO_LEADING_TABLE
        .iter()
        .filter(|r| r.leading.is_some())
        .count();
    assert!(measured >= 4, "need several measured fonts, got {measured}");
}

#[test]
fn auto_leading_on_real_face() {
    let bytes = load("NotoSans-Regular.ttf");
    let face = Face::parse(&bytes).unwrap();
    assert!(approx(auto_leading(&face, 48.0), 57.6));
    assert!(approx(auto_leading(&face, 96.0), 115.2));
}

#[test]
fn hanging_off_by_default_and_no_shift() {
    let rules = Kinsoku::default();
    assert!(!rules.enabled);
    let line = [glyph('(', 4.0), glyph('a', 5.0)];
    assert_eq!(hanging_adjust(&line, 40.0, &rules), 0.0);
}

#[test]
fn hanging_opening_shifts_by_advance() {
    let rules = Kinsoku {
        enabled: true,
        ..Kinsoku::default()
    };
    let line = [glyph('(', 4.0), glyph('a', 5.0)];
    assert!(approx(hanging_adjust(&line, 40.0, &rules), -4.0));
    // Non-opening first glyph: no hang.
    let plain = [glyph('a', 5.0), glyph('(', 4.0)];
    assert_eq!(hanging_adjust(&plain, 40.0, &rules), 0.0);
    // Line wider than the box: no hang.
    assert_eq!(hanging_adjust(&line, 5.0, &rules), 0.0);
    assert_eq!(hanging_adjust(&[], 40.0, &rules), 0.0);
}

#[test]
fn trailing_punctuation_hangs_up_to_advance() {
    let rules = Kinsoku {
        enabled: true,
        ..Kinsoku::default()
    };
    // Content fits, overhang 3 <= advance 3.
    assert!(can_hang_trailing(40.0, '.', 3.0, 40.0, &rules));
    // Overhang 1 is within the advance.
    assert!(can_hang_trailing(38.0, ',', 3.0, 40.0, &rules));
    // Content already past the box: must wrap.
    assert!(!can_hang_trailing(41.0, '.', 3.0, 40.0, &rules));
    // Non-punctuation never hangs.
    assert!(!can_hang_trailing(40.0, 'a', 3.0, 40.0, &rules));
    // Disabled rules never hang.
    assert!(!can_hang_trailing(
        40.0,
        '.',
        3.0,
        40.0,
        &Kinsoku::default()
    ));
}

#[test]
fn paragraph_spacing_before_after_and_indents() {
    let p1 = ParagraphBlock {
        space_before: 0.0,
        space_after: 5.0,
        first_line_indent: 3.0,
        start_indent: 2.0,
        line_count: 2,
        leading: 10.0,
    };
    let p2 = ParagraphBlock {
        space_before: 4.0,
        space_after: 0.0,
        first_line_indent: 0.0,
        start_indent: 2.0,
        line_count: 1,
        leading: 10.0,
    };
    let empty = ParagraphBlock {
        line_count: 0,
        ..p2
    };
    let out = apply_paragraph_spacing(100.0, &[p1, empty, p2]);
    assert_eq!(out.len(), 3);
    assert!(approx(out[0].baseline, 100.0) && approx(out[0].x_offset, 5.0));
    assert!(approx(out[1].baseline, 110.0) && approx(out[1].x_offset, 2.0));
    // 110 + leading 10 + space_after 5 + space_before 4 = 129
    assert!(approx(out[2].baseline, 129.0) && approx(out[2].x_offset, 2.0));
}

#[test]
fn gsub_feature_parsing_synthetic() {
    // Header: version, scriptList=0, featureList=10, lookupList=0.
    // FeatureList at 10: count=1, record {liga, offset 0}.
    let mut gsub = vec![0, 1, 0, 0, 0, 0, 0, 10, 0, 0];
    gsub.extend_from_slice(&[0, 1]);
    gsub.extend_from_slice(b"liga");
    gsub.extend_from_slice(&[0, 0]);
    assert!(gsub_has_feature(&gsub, *b"liga"));
    assert!(!gsub_has_feature(&gsub, *b"kern"));
    assert!(!gsub_has_feature(&gsub[..8], *b"liga"));
}

#[test]
fn ligature_spans_policy() {
    assert_eq!(ligature_spans("ffi", true), vec![(0, 3)]);
    assert_eq!(ligature_spans("a fi fl", true), vec![(2, 4), (5, 7)]);
    // Inactive policy: no substitution at all.
    assert!(ligature_spans("ffi fi", false).is_empty());
}

#[test]
fn ligatures_real_fonts() {
    // NotoSans has GSUB liga: active when the layer flag is on.
    let noto = load("NotoSans-Regular.ttf");
    let face = Face::parse(&noto).unwrap();
    assert!(standard_ligatures_active(&face, true));
    assert!(!standard_ligatures_active(&face, false));
    // CCMeanwhile has no GSUB: must be a no-op even with the flag on.
    let cc = load("CCMeanwhile.otf");
    let cc_face = Face::parse(&cc).unwrap();
    assert!(!standard_ligatures_active(&cc_face, true));
    assert!(ligature_spans("fi", standard_ligatures_active(&cc_face, true)).is_empty());
}

#[test]
fn faux_italic_skew_geometry() {
    assert!((FAUX_ITALIC_SKEW - 0.2126).abs() < 1e-3);
    let (x, y) = faux_italic_point(0.0, 100.0);
    assert!(approx(y, 100.0));
    assert!((x - 21.26).abs() < 0.05);
    // Baseline points do not move.
    assert_eq!(faux_italic_point(7.0, 0.0), (7.0, 0.0));
}

#[test]
fn faux_bold_strength() {
    // Measured on `10_all_features.psd` layer `01 capitalisation underline
    // faux bold`: 32 glyphs at 30 pt widen the run by 25.9 pt, i.e. size/37 per
    // glyph (Photoshop's extra is smaller than the old size/24 guess).
    assert!(approx(
        faux_bold_extra_advance(24.0),
        24.0 / FAUX_BOLD_DIVISOR
    ));
    assert!(approx(
        faux_bold_stroke_offset(24.0),
        12.0 / FAUX_BOLD_DIVISOR
    ));
}

#[test]
fn underline_and_strikeout_geometry() {
    // post: version(4), italicAngle(4), underlinePosition=-100, underlineThickness=50.
    let mut post = vec![0u8; 12];
    post[8..10].copy_from_slice(&(-100i16).to_be_bytes());
    post[10..12].copy_from_slice(&50i16.to_be_bytes());
    let u = underline_geometry(&post, 10.0, 1000).unwrap();
    assert!(approx(u.offset, -1.0));
    assert!(approx(u.thickness, 0.5));
    assert!(underline_geometry(&post[..10], 10.0, 1000).is_none());

    // OS/2: yStrikeoutSize at 26 = 50, yStrikeoutPosition at 28 = 300.
    let mut os2 = vec![0u8; 30];
    os2[26..28].copy_from_slice(&50i16.to_be_bytes());
    os2[28..30].copy_from_slice(&300i16.to_be_bytes());
    let s = strikeout_geometry(&os2, 10.0, 1000).unwrap();
    assert!(approx(s.offset, 3.0));
    assert!(approx(s.thickness, 0.5));
    assert!(strikeout_geometry(&os2[..28], 10.0, 1000).is_none());
}
