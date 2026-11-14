//! The legacy `kern` table must be read: Photoshop kerns Times New Roman and Arial from it, and
//! those faces carry no GPOS pair kerning at all. The values below were read from the shipped
//! files with fontTools (`kern` format 0, coverage 0x0001) and are what Photoshop applies in the
//! sync matrix (`box_h_times30_lead36`, `point_h_arial22_track-50`).

use std::path::PathBuf;

use photocraft_text::Face;

fn wine_font(dir: &str, file: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".local/share/photocraft/wine/drive_c").join(dir).join(file);
    path.is_file().then_some(path)
}

fn face_and_kern<'a>(path: &PathBuf, data: &'a Vec<u8>, pair: &str) -> i32 {
    let face = Face::parse(data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut chars = pair.chars();
    let left = face.glyph_index(chars.next().unwrap()).unwrap();
    let right = face.glyph_index(chars.next().unwrap()).unwrap();
    face.kern_pair(left, right)
}

#[test]
fn times_new_roman_kerns_from_its_legacy_kern_table() {
    let Some(path) = wine_font("windows/Fonts", "times.ttf") else {
        eprintln!("skipped: times.ttf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    let data = std::fs::read(&path).unwrap();
    let face = Face::parse(&data).unwrap();
    assert_eq!(face.units_per_em(), 2048);
    // fontTools: kern subtable 0 has (36,55) -227 and (36,57) -264.
    assert_eq!(face_and_kern(&path, &data, "AV"), -264, "A/V kern in font units");
    assert_eq!(face_and_kern(&path, &data, "AT"), -227, "A/T kern in font units");
    // The pixels Photoshop's sync case measures: font units scaled to 30 pt.
    let scale = 30.0 / face.units_per_em() as f32;
    assert!((face_and_kern(&path, &data, "AV") as f32 * scale - (-3.867_187_5)).abs() < 1e-4, "A/V at 30 pt");
    assert!((face_and_kern(&path, &data, "AT") as f32 * scale - (-3.325_195_3)).abs() < 1e-4, "A/T at 30 pt");
    // No kern record: no adjustment, not a neighbour's value.
    assert_eq!(face_and_kern(&path, &data, "AA"), 0);
    assert_eq!(face_and_kern(&path, &data, "OO"), 0);
}

#[test]
fn arial_kerns_from_its_legacy_kern_table() {
    let Some(path) = wine_font("windows/Fonts", "arial.ttf") else {
        eprintln!("skipped: arial.ttf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    let data = std::fs::read(&path).unwrap();
    let face = Face::parse(&data).unwrap();
    // fontTools: kern subtable 0 has (36,55) -152 and (36,57) -152.
    assert_eq!(face_and_kern(&path, &data, "AV"), -152);
    assert_eq!(face_and_kern(&path, &data, "AT"), -152);
    let scale = 30.0 / face.units_per_em() as f32;
    assert!((face_and_kern(&path, &data, "AV") as f32 * scale - (-2.226_562_5)).abs() < 1e-4, "A/V at 30 pt");
    assert_eq!(face_and_kern(&path, &data, "AA"), 0);
}

/// A face without a `kern` table (Myriad Pro is CFF+GPOS only) must keep taking its GPOS values:
/// the sync matrix's canonical Myriad rows depend on them.
#[test]
fn a_gpos_only_face_still_takes_the_gpos_value() {
    let Some(path) = wine_font(
        "Program Files/Adobe/Adobe Photoshop CC 2019/Required/PDFL/Resource/Fonts",
        "MyriadPro-Regular.otf",
    ) else {
        eprintln!("skipped: MyriadPro-Regular.otf is not installed in PhotoCraft's Wine prefix");
        return;
    };
    let data = std::fs::read(&path).unwrap();
    let face = Face::parse(&data).unwrap();
    assert!(!face.kern.has_subtables(), "Myriad Pro must not have a usable legacy kern subtable");
    for (pair, expected) in [("AV", -53), ("VA", -57), ("AT", -77), ("To", -71), ("Ty", -40)] {
        assert_eq!(face_and_kern(&path, &data, pair), expected, "{pair} GPOS kern in font units");
    }
}

/// The rest of the kern-table families the corpus can name: every value is from the file's own
/// `kern` table (fontTools), so a regression in the subtable parser fails per family.
#[test]
fn every_kern_table_face_returns_its_recorded_pairs() {
    let families: &[(&str, &[(&str, i32)])] = &[
        ("times.ttf", &[("AV", -264), ("AT", -227), ("To", -143)]),
        ("timesbd.ttf", &[("AV", -264), ("AT", -152), ("To", -188)]),
        ("arial.ttf", &[("AV", -152), ("AT", -152), ("To", -227)]),
        ("arialbd.ttf", &[("AV", -152), ("AT", -152), ("To", -152)]),
        ("trebuc.ttf", &[("AV", -180), ("AT", -199), ("To", -255)]),
        ("verdana.ttf", &[("AV", -60), ("AT", -120), ("To", -220)]),
        ("impact.ttf", &[("AV", -45), ("AT", -113)]),
    ];
    let mut checked = 0;
    for (file, pairs) in families {
        let Some(path) = wine_font("windows/Fonts", file) else { continue };
        let data = std::fs::read(&path).unwrap();
        let face = Face::parse(&data).unwrap();
        if !face.kern.has_subtables() {
            eprintln!("skipped {file}: no legacy kern subtable");
            continue;
        }
        for (pair, expected) in *pairs {
            assert_eq!(face_and_kern(&path, &data, pair), *expected, "{file} {pair}");
        }
        checked += 1;
    }
    assert!(checked >= 3, "checked only {checked} kern-table families; the font prefix looks incomplete");
}
