//! Optical kerning, calibrated against Photoshop's own recomposition.
//!
//! One point-text layer per pair at 48 pt Myriad Pro with `textItem.autoKerning = OPTICAL`, saved
//! through PhotoCraft's UI, opened and force-recomposed by Photoshop (`ti.tracking += 5; -= 5`),
//! then compared with the same text saved with kerning off. The table is Photoshop's kerning for
//! the pair, in px at 48 pt (from `out/verify/txt2work/optical_pairs.json`). The fitted gap model
//! alone reaches only about -1.3 px per pair because its clamp suits lower-case text; the class
//! factors scale the closure for diagonals, `T`/`Y` pairs and punctuation.

use std::path::PathBuf;

use photocraft_text::font::Face;
use photocraft_text::optical::{optical_class_factor, optical_pair_chars};

fn myriad() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(
        ".local/share/photocraft/wine/drive_c/Program Files/Adobe/Adobe Photoshop CC 2019/Required/PDFL/Resource/Fonts/MyriadPro-Regular.otf",
    );
    path.is_file().then_some(path)
}

fn face() -> Option<Face<'static>> {
    let path = myriad()?;
    let bytes = std::fs::read(path).expect("read MyriadPro-Regular.otf");
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    Some(Face::parse(leaked).expect("parse"))
}

/// Photoshop's optical kerning for the pair, px at 48 pt (ink width with optical minus ink width
/// with kerning off, both recomposed by Photoshop).
const MEASURED: &[(&str, f32)] = &[
    ("AV", -3.0),
    ("VA", -3.0),
    ("AT", -5.0),
    ("To", -4.0),
    ("Ty", -4.0),
    ("Wa", -2.0),
    ("Ve", -2.0),
    ("Yo", -4.0),
    ("Te", -4.0),
    ("Fa", -3.0),
    ("LT", -5.0),
    ("P.", -7.0),
];

fn pair_kern(face: &Face<'_>, pair: &str, size: f32) -> f32 {
    let mut chars = pair.chars();
    let a = chars.next().unwrap();
    let b = chars.next().unwrap();
    let (ga, gb) = (face.glyph_index(a).unwrap(), face.glyph_index(b).unwrap());
    optical_pair_chars(face, ga, gb, size, a, b)
}

#[test]
fn optical_pairs_match_photoshop_at_48pt() {
    let Some(face) = face() else {
        eprintln!("skipped: MyriadPro-Regular.otf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    for (pair, want) in MEASURED {
        let got = pair_kern(&face, pair, 48.0);
        assert!(
            (got - want).abs() <= 1.5,
            "{pair}: model {got:.2} px vs Photoshop {want:.1} px at 48 pt"
        );
    }
}

#[test]
fn optical_pairs_are_size_linear() {
    let Some(face) = face() else {
        eprintln!("skipped: MyriadPro-Regular.otf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    for (pair, _) in MEASURED {
        let at48 = pair_kern(&face, pair, 48.0);
        let at96 = pair_kern(&face, pair, 96.0);
        assert!((at96 - 2.0 * at48).abs() <= 0.01, "{pair}: {at48} px at 48 pt vs {at96} px at 96 pt");
    }
}

/// The sums Photoshop's recomposition shows for repeated pairs: `AVAV` -10 px and `AVAVAVAV`
/// -23 px at 48 pt (ink width differences against kerning off). The model's total is compared
/// with the same tolerance as a single pair.
#[test]
fn optical_sums_match_photoshop() {
    let Some(face) = face() else {
        eprintln!("skipped: MyriadPro-Regular.otf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    for (text, want) in [("AVAV", -10.0f32), ("AVAVAVAV", -23.0)] {
        let chars: Vec<char> = text.chars().collect();
        let mut total = 0.0;
        for w in chars.windows(2) {
            total += pair_kern(&face, &format!("{}{}", w[0], w[1]), 48.0);
        }
        assert!((total - want).abs() <= 5.0, "{text}: model {total:.2} px vs Photoshop {want:.1} px");
    }
}

#[test]
fn class_factors_are_the_measured_classes() {
    assert_eq!(optical_class_factor('P', '.'), 0.81);
    assert_eq!(optical_class_factor('A', 'T'), 0.95);
    assert_eq!(optical_class_factor('T', 'o'), 0.95);
    assert_eq!(optical_class_factor('A', 'V'), 0.64);
    assert_eq!(optical_class_factor('F', 'a'), 0.42);
    assert_eq!(optical_class_factor('W', 'a'), 0.78);
    assert_eq!(optical_class_factor('V', 'e'), 0.78);
    assert_eq!(optical_class_factor('a', 'v'), 1.0);
}
