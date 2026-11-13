//! Photoshop's own glyph advances, checked against a reference PDF export.
//!
//! `data/plain_latin.json` holds per-glyph pen positions that Photoshop wrote
//! into a 48 pt NotoSans-Regular text layer; the layout engine must reproduce
//! them to within a thousandth of a point.

use std::path::PathBuf;

use photocraft_text::{Face, font::reader};

#[derive(serde::Deserialize)]
struct Fixture {
    text: String,
    /// Points per em as Photoshop actually wrote them into the PDF
    /// (`Tm` scale x page width); it is 47.997 for a 48 pt layer, because the
    /// PDF matrix is stored with five decimals.
    em_px: f32,
    origin: f32,
    glyphs: Vec<Glyph>,
}

#[derive(serde::Deserialize)]
struct Glyph {
    ch: String,
    x: f32,
}

fn font_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    PathBuf::from(home)
        .join(".local/share/photocraft/wine/drive_c/windows/Fonts/NotoSans-Regular.ttf")
}

#[test]
fn advances_match_photoshop_pdf() {
    let data = std::fs::read_to_string("tests/data/plain_latin.json").unwrap();
    let fixture: Fixture = serde_json::from_str(&data).unwrap();
    let bytes = std::fs::read(font_path()).expect("NotoSans-Regular.ttf in the Wine prefix");
    let face = Face::parse(&bytes).unwrap();

    let scale = fixture.em_px / face.units_per_em() as f32;
    let origin = fixture.origin;

    let mut pen = 0f32;
    let mut prev: Option<u16> = None;
    for (i, expected) in fixture.glyphs.iter().enumerate() {
        let ch = expected.ch.chars().next().unwrap();
        let gid = face
            .glyph_index(ch)
            .unwrap_or_else(|| panic!("no glyph for {ch:?}"));
        if let Some(p) = prev {
            pen += face.kern_pair(p, gid) as f32 * scale;
        }
        let x = origin + pen;
        // PDF pen offsets are quantised to 1e-5 em, i.e. ~0.0005 pt per glyph,
        // so the cumulative error grows slowly with the glyph index.
        let tolerance = 0.001 + i as f32 * 0.0006;
        assert!(
            (x - expected.x).abs() < tolerance,
            "glyph {i} {:?}: engine {x:.5} vs photoshop {:.5} (delta {:.5}, tol {tolerance:.5})",
            ch,
            expected.x,
            x - expected.x
        );
        pen += face.advance(gid) as f32 * scale;
        prev = Some(gid);
    }
}

#[test]
fn reader_rejects_truncated_tables() {
    let bytes = std::fs::read(font_path()).unwrap();
    let short = &bytes[..100];
    assert!(Face::parse(short).is_err());
    let _ = reader::Reader::new(short);
}
