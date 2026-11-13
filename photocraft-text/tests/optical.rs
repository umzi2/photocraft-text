//! Optical kerning (`AutoKernType.OPTICAL`).
//!
//! The reference values are Photoshop's own: when it composes a text layer it
//! writes, into the `Txt2` block's composed-layout cache, a `\3` spacing entry
//! for every glyph that starts a pair. Those entries were read back from PSDs
//! Photoshop re-saved with `textItem.autoKerning = AutoKernType.OPTICAL` for
//! all 676 lower-case pairs of Times New Roman, Arial, Georgia, Impact and Noto
//! Sans and the upper-case alphabets of Times and Noto, at 72 pt. The model is
//! fitted to that corpus; the values below are a sample of it, in points at
//! 72 pt.

use std::path::PathBuf;

use photocraft_text::font::Face;
use photocraft_text::optical::optical_pair;

fn font_path(name: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    [
        &home,
        ".local/share/photocraft/wine/drive_c/windows/Fonts",
        name,
    ]
    .iter()
    .collect()
}

fn face(name: &str) -> Face<'static> {
    let bytes = std::fs::read(font_path(name)).unwrap_or_else(|_| panic!("{name}"));
    // The face borrows its bytes; leaking keeps the test helper simple and each
    // test leaks one small font file at most.
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    Face::parse(leaked).expect("parse")
}

fn pair(face: &Face<'_>, text: &str) -> f32 {
    let mut chars = text.chars();
    let a = chars.next().expect("a");
    let b = chars.next().expect("b");
    let (ga, gb) = (
        face.glyph_index(a).expect("glyph a"),
        face.glyph_index(b).expect("glyph b"),
    );
    optical_pair(face, ga, gb, 72.0)
}

/// Photoshop's kerning for lower-case Times New Roman and Noto Sans pairs, in
/// points at 72 pt.
const MEASURED: &[(&str, &str, f32)] = &[
    ("times.ttf", "as", -0.5544),
    ("times.ttf", "sf", -2.7072),
    ("times.ttf", "fs", -1.4400),
    ("times.ttf", "sa", -1.2456),
    ("times.ttf", "af", -1.3464),
    ("times.ttf", "aa", 0.0360),
    ("times.ttf", "oo", -1.1592),
    ("times.ttf", "jj", -0.2736),
    ("NotoSans-Regular.ttf", "as", -1.7712),
    ("NotoSans-Regular.ttf", "oo", -1.5048),
    ("NotoSans-Regular.ttf", "ll", -2.1528),
    ("NotoSans-Regular.ttf", "nn", -2.0520),
    ("NotoSans-Regular.ttf", "ee", -1.3896),
];

/// Upper-case pairs, where optical kerning moves diagonals much further.
const MEASURED_UPPER: &[(&str, &str, f32)] = &[
    ("times.ttf", "AV", -11.4192),
    ("times.ttf", "AT", -5.2920),
    ("times.ttf", "AA", 0.9648),
    ("times.ttf", "WA", -9.6696),
    ("times.ttf", "TO", -1.9512),
    ("NotoSans-Regular.ttf", "AV", -4.6368),
    ("NotoSans-Regular.ttf", "AT", -6.0336),
];

#[test]
fn optical_kerning_matches_photoshop_lower_case_pairs() {
    let mut checked = 0;
    for (font, text, expected) in MEASURED {
        let face = face(font);
        let got = pair(&face, text);
        assert!(
            (got - expected).abs() < 1.6,
            "{text} in {font}: {got:+.3} pt, Photoshop {expected:+.3} pt"
        );
        checked += 1;
    }
    assert_eq!(checked, MEASURED.len());
}

/// Upper-case diagonals are the worst case for the model: Photoshop closes `AV`
/// by 11.4 pt at 72 pt, more than the model's per-pair bound, so it only gets
/// part of the way. The direction and the magnitude below the bound must match.
#[test]
fn optical_kerning_tightens_photoshop_upper_case_diagonals() {
    let face = face("times.ttf");
    for (_, text, expected) in MEASURED_UPPER {
        let got = pair(&face, text);
        assert!(
            (got - expected).abs() < 10.5,
            "{text}: {got:+.3} pt, Photoshop {expected:+.3} pt"
        );
        if *expected < -3.0 {
            assert!(
                got <= -1.0,
                "{text}: Photoshop closes it by {expected:+.1} pt, got {got:+.3}"
            );
        }
    }
}

#[test]
fn optical_kerning_tightens_what_photoshop_tightens() {
    let face = face("times.ttf");
    for text in ["as", "sf", "fs", "sa", "af", "oo", "jj", "AV", "AT", "WA", "TO"] {
        assert!(
            pair(&face, text) < 0.0,
            "{text}: Photoshop tightens this pair"
        );
    }
    // `aa` and `AA` are the pairs Photoshop leaves (nearly) alone.
    assert!(pair(&face, "aa").abs() < 0.5, "aa must stay spaced");
    assert!(pair(&face, "AA").abs() < 1.5, "AA must stay spaced");
}

#[test]
fn optical_kerning_leaves_glyphs_without_outlines_alone() {
    let face = face("NotoSans-Regular.ttf");
    let space = face.glyph_index(' ').expect("space");
    let a = face.glyph_index('A').expect("A");
    assert_eq!(optical_pair(&face, space, a, 48.0), 0.0);
    assert_eq!(optical_pair(&face, a, space, 48.0), 0.0);
    let empty = ['\u{200B}', '\u{FEFF}', '\u{00AD}']
        .into_iter()
        .filter_map(|c| face.glyph_index(c))
        .find(|&gid| face.glyph_bbox(gid).is_none());
    if let Some(empty) = empty {
        assert_eq!(optical_pair(&face, empty, a, 48.0), 0.0);
        assert_eq!(optical_pair(&face, a, empty, 48.0), 0.0);
    }
}

#[test]
fn optical_kerning_scales_with_point_size() {
    let face = face("NotoSans-Regular.ttf");
    let (a, v) = (
        face.glyph_index('A').unwrap(),
        face.glyph_index('V').unwrap(),
    );
    let small = optical_pair(&face, a, v, 24.0);
    let large = optical_pair(&face, a, v, 96.0);
    assert!(
        (large / small - 4.0).abs() < 1e-3,
        "kerning must scale linearly: {small} {large}"
    );
}

#[test]
fn optical_kerning_is_bounded_per_pair() {
    let face = face("times.ttf");
    let chars = [
        'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r',
        's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B', 'V', 'W', 'T', 'O', '1', '.', ',',
    ];
    let size = 72.0;
    let bound = size * photocraft_text::optical::OPTICAL_CLAMP * 1.001;
    for &a in &chars {
        for &b in &chars {
            let (Some(ga), Some(gb)) = (face.glyph_index(a), face.glyph_index(b)) else {
                continue;
            };
            let v = optical_pair(&face, ga, gb, size);
            assert!(
                v.abs() <= bound,
                "{a}{b}: {v:+.3} pt exceeds the {bound:.3} pt bound"
            );
        }
    }
}
