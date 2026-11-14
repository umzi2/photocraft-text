//! GPOS pair kerning must be summed over *distinct* lookups.
//!
//! MyriadPro-Regular's GPOS lists nine `kern` feature records (one per script), all pointing at
//! lookup 1. Collecting the lookup once per feature record applied it nine times, so `AV` at
//! 48 pt came out at -477 units instead of -53 and the app rendered metrics-kerned text with the
//! glyphs crushed together. The reference values below were read out of the font's GPOS with
//! fontTools (class-based PairPos format 2, classDefs A->13 V->6, XAdvance -53).

use std::path::PathBuf;

use photocraft_text::Face;

fn myriad() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(
        ".local/share/photocraft/wine/drive_c/Program Files/Adobe/Adobe Photoshop CC 2019/Required/PDFL/Resource/Fonts/MyriadPro-Regular.otf",
    );
    path.is_file().then_some(path)
}

fn kern_of(face: &Face<'_>, pair: &str) -> i32 {
    let mut chars = pair.chars();
    let left = face.glyph_index(chars.next().unwrap()).unwrap();
    let right = face.glyph_index(chars.next().unwrap()).unwrap();
    face.kern_pair(left, right)
}

#[test]
fn a_kern_lookup_is_applied_once_however_many_scripts_name_it() {
    let Some(path) = myriad() else {
        eprintln!("skipped: MyriadPro-Regular.otf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let face = Face::parse(&data).unwrap();
    // The font's GPOS values; before the fix these came back 9x larger (-477, -513, -693, ...).
    for (pair, expected) in [("AV", -53), ("VA", -57), ("AT", -77), ("To", -71), ("Ty", -40)] {
        assert_eq!(kern_of(&face, pair), expected, "{pair} kern in font units");
    }
}

/// `AVAVAVAV` at 48 pt: 224.64 px with kerning off, 206.256 px with the font's metrics kerning.
#[test]
fn metrics_kerning_of_avavavav_at_48pt() {
    let Some(path) = myriad() else {
        eprintln!("skipped: MyriadPro-Regular.otf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let face = Face::parse(&data).unwrap();
    let scale = 48.0 / face.units_per_em() as f32;
    let text = "AVAVAVAV";
    let mut advances = 0f32;
    let mut kerned = 0f32;
    let mut prev: Option<u16> = None;
    for ch in text.chars() {
        let gid = face.glyph_index(ch).unwrap();
        advances += face.advance(gid) as f32 * scale;
        if let Some(p) = prev {
            kerned += face.kern_pair(p, gid) as f32 * scale;
        }
        prev = Some(gid);
    }
    let width = advances + kerned;
    assert!((advances - 224.64).abs() < 0.01, "kerning off: {advances} px");
    assert!((width - 206.256).abs() < 0.01, "metrics kerning: {width} px");
}
