//! CFF/OTF outline tests against fontTools-generated fixtures.
//!
//! The fixture `tests/data/cff_bbox.json` was produced from the real font files
//! with fontTools: `BoundsPen` (exact curve extrema, composite `glyf` expanded)
//! for glyph bounds and `RecordingPen` for contours. Regenerate with:
//!
//! ```text
//! .venv/bin/python - <<'PY'
//! from fontTools.ttLib import TTFont
//! from fontTools.pens.boundsPen import BoundsPen
//! from fontTools.pens.recordingPen import RecordingPen
//! # (see docs/STATUS.md and the cff task report for the full generator)
//! PY
//! ```
//!
//! Every CFF face under `/media/H/anime-set/text_detect/fontas` is covered; the
//! tests skip a font whose file is absent so the suite stays portable, but they
//! require at least ten CFF faces to have been present.

use std::collections::HashMap;
use std::path::Path;

use photocraft_text::font::Face;
use photocraft_text::outline::{Seg, glyph_contours, glyph_path, path_bbox};
use photocraft_text::raster::Canvas;

const FIXTURE: &str = "tests/data/cff_bbox.json";
const FONTAS: &str = "/media/H/anime-set/text_detect/fontas";
const WINE_FONTS: &str = "/home/umzi/.local/share/photocraft/wine/drive_c/windows/Fonts";

#[derive(serde::Deserialize)]
struct Fixture {
    cff_fonts: Vec<FontEntry>,
    reference_fonts: Vec<FontEntry>,
    measured: Vec<Measured>,
}

#[derive(serde::Deserialize)]
struct FontEntry {
    path: String,
    file: String,
    #[allow(dead_code)]
    name: String,
    units_per_em: u16,
    #[allow(dead_code)]
    hhea_ascender: i16,
    #[serde(default)]
    ratios: HashMap<String, Option<f32>>,
    ascent_ratio: Option<f32>,
    #[serde(default)]
    glyph_gids: HashMap<String, u16>,
    #[serde(default)]
    bboxes: HashMap<String, Option<Vec<f32>>>,
    #[serde(default)]
    contours: HashMap<String, GlyphContours>,
}

#[derive(serde::Deserialize)]
struct GlyphContours {
    #[allow(dead_code)]
    glyph: String,
    gid: u16,
    contours: Vec<JsonContour>,
}

#[derive(serde::Deserialize)]
struct JsonContour {
    start: Vec<f32>,
    segs: Vec<Vec<serde_json::Value>>,
}

#[derive(serde::Deserialize)]
struct Measured {
    font: String,
    file: String,
    ratio: f32,
    upm: u16,
}

fn load() -> Fixture {
    let text = std::fs::read_to_string(FIXTURE).expect("read CFF fixture");
    serde_json::from_str(&text).expect("parse CFF fixture")
}

/// Reads a font and checks the file exists; `None` means "skip".
fn read_font(path: &str) -> Option<Vec<u8>> {
    if !Path::new(path).exists() {
        eprintln!("skip: font {path} absent");
        return None;
    }
    Some(std::fs::read(path).expect("read font"))
}

fn close(got: f32, want: f32, tol: f32, what: &str) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: got {got}, expected {want} (tolerance {tol})"
    );
}

/// Every glyph of every CFF face must outline, and its exact curve-extrema
/// bounds must match fontTools. This exercises the whole Type 2 interpreter
/// (subrs, hintmask mask bytes, fixed-point operands) on 24 fonts / 11 646
/// glyphs.
#[test]
fn cff_glyph_bounds_match_fonttools() {
    let fixture = load();
    let mut fonts = 0usize;
    let mut glyphs = 0usize;
    for entry in &fixture.cff_fonts {
        let Some(data) = read_font(&entry.path) else {
            continue;
        };
        let face = Face::parse(&data).unwrap_or_else(|e| panic!("{}: {e:?}", entry.file));
        assert!(face.has_cff(), "{}: CFF table was not parsed", entry.file);
        assert!(!face.has_glyf(), "{}: unexpected glyf table", entry.file);
        assert_eq!(face.units_per_em(), entry.units_per_em, "{}", entry.file);

        // The cmap must agree with fontTools about the glyph ids.
        for (ch, expected_gid) in &entry.glyph_gids {
            let c = ch.chars().next().expect("one character");
            assert_eq!(
                face.glyph_index(c),
                Some(*expected_gid),
                "{}: cmap '{ch}'",
                entry.file
            );
        }

        for (gid, expected) in &entry.bboxes {
            let gid: u16 = gid.parse().expect("gid");
            let path = glyph_path(&face, gid)
                .unwrap_or_else(|e| panic!("{} gid {gid}: outline failed: {e:?}", entry.file));
            let got = path_bbox(&path);
            match expected {
                None => assert!(
                    got.is_none(),
                    "{} gid {gid}: expected an empty glyph, got {got:?}",
                    entry.file
                ),
                Some(want) => {
                    let got = got.unwrap_or_else(|| {
                        panic!(
                            "{} gid {gid}: empty outline, expected bbox {want:?}",
                            entry.file
                        )
                    });
                    for i in 0..4 {
                        close(
                            [got.0, got.1, got.2, got.3][i],
                            want[i],
                            0.01,
                            &format!("{} gid {gid} bbox[{i}]", entry.file),
                        );
                    }
                }
            }
            glyphs += 1;
        }
        fonts += 1;
    }
    assert!(fonts >= 10, "only {fonts} CFF fonts were available");
    println!("checked {glyphs} glyphs in {fonts} CFF fonts");
}

/// Segment-by-segment comparison against fontTools' recording pen: contour
/// order, start points, line and cubic control points.
#[test]
fn cff_contours_match_fonttools() {
    let fixture = load();
    let mut fonts = 0usize;
    for entry in &fixture.cff_fonts {
        let Some(data) = read_font(&entry.path) else {
            continue;
        };
        let face = Face::parse(&data).expect("parse face");
        for (ch, glyph) in &entry.contours {
            let c = ch.chars().next().expect("one character");
            let gid = face
                .glyph_index(c)
                .unwrap_or_else(|| panic!("{}: no cmap entry for '{ch}'", entry.file));
            assert_eq!(gid, glyph.gid, "{}: gid of '{ch}'", entry.file);
            let path =
                glyph_path(&face, gid).unwrap_or_else(|e| panic!("{} '{ch}': {e:?}", entry.file));
            assert_eq!(
                path.len(),
                glyph.contours.len(),
                "{} '{ch}': contour count",
                entry.file
            );
            for (ci, (contour, want)) in path.iter().zip(&glyph.contours).enumerate() {
                close(
                    contour.start.x,
                    want.start[0],
                    0.01,
                    &format!("{} '{ch}' contour {ci} start.x", entry.file),
                );
                close(
                    contour.start.y,
                    want.start[1],
                    0.01,
                    &format!("{} '{ch}' contour {ci} start.y", entry.file),
                );
                assert_eq!(
                    contour.segs.len(),
                    want.segs.len(),
                    "{} '{ch}' contour {ci}: segment count",
                    entry.file
                );
                for (si, (seg, json)) in contour.segs.iter().zip(&want.segs).enumerate() {
                    let tag = json[0].as_str().expect("segment tag");
                    let nums: Vec<f32> = json[1..]
                        .iter()
                        .map(|v| v.as_f64().expect("segment coordinate") as f32)
                        .collect();
                    match *seg {
                        Seg::Line { to } => {
                            assert_eq!(tag, "L", "{} '{ch}' contour {ci} seg {si}", entry.file);
                            close(to.x, nums[0], 0.01, "line x");
                            close(to.y, nums[1], 0.01, "line y");
                        }
                        Seg::Cubic { c1, c2, to } => {
                            assert_eq!(tag, "C", "{} '{ch}' contour {ci} seg {si}", entry.file);
                            close(c1.x, nums[0], 0.01, "cubic c1.x");
                            close(c1.y, nums[1], 0.01, "cubic c1.y");
                            close(c2.x, nums[2], 0.01, "cubic c2.x");
                            close(c2.y, nums[3], 0.01, "cubic c2.y");
                            close(to.x, nums[4], 0.01, "cubic x");
                            close(to.y, nums[5], 0.01, "cubic y");
                        }
                        Seg::Quad { .. } => {
                            panic!("{} '{ch}': CFF produced a quadratic segment", entry.file)
                        }
                    }
                }
            }
        }
        fonts += 1;
    }
    assert!(fonts >= 10, "only {fonts} CFF fonts were available");
}

/// `Face::ascent_ratio` must equal the exact ink `yMax` of 'd' / upm for CFF
/// faces (and for the `glyf` reference faces, where curve extrema matter).
#[test]
fn ascent_ratio_matches_exact_d_bbox() {
    let fixture = load();
    let mut checked = 0usize;
    for entry in fixture.cff_fonts.iter().chain(&fixture.reference_fonts) {
        let Some(data) = read_font(&entry.path) else {
            continue;
        };
        let face = Face::parse(&data).expect("parse face");
        let expected = entry
            .ascent_ratio
            .unwrap_or_else(|| panic!("{}: fixture has no exact ascent ratio", entry.file));
        close(
            face.ascent_ratio(),
            expected,
            2e-4,
            &format!("{} ascent_ratio", entry.file),
        );

        let upm = face.units_per_em() as f32;
        for (ch, want) in &entry.ratios {
            let (Some(want), Some(gid)) = (want, face.glyph_index(ch.chars().next().unwrap()))
            else {
                continue;
            };
            let bbox = face.glyph_bbox(gid).expect("glyph bbox");
            close(
                bbox.3 / upm,
                *want,
                2e-4,
                &format!("{} '{ch}' yMax/upm", entry.file),
            );
        }
        checked += 1;
    }
    assert!(checked >= 20, "only {checked} fonts checked");
}

/// Validate the implemented ratio against Photoshop's own measurements
/// (`out/probe5/font_ratios.json`). One stored value (`CCSpellcaster`,
/// 6.2656) is a mis-associated PDF line, so ratios outside `[0.3, 1.3]` are
/// skipped; the rest agree within the PDF's ~0.0012 resolution.
#[test]
fn ascent_ratio_matches_photoshop_probe() {
    let fixture = load();
    let mut checked = 0usize;
    for m in &fixture.measured {
        if !(0.3..=1.3).contains(&m.ratio) {
            eprintln!("skip mis-associated measurement {} = {}", m.font, m.ratio);
            continue;
        }
        let Some(data) = read_font(&m.file) else {
            continue;
        };
        let face = Face::parse(&data).expect("parse face");
        assert_eq!(face.units_per_em(), m.upm, "{}", m.font);
        close(
            face.ascent_ratio(),
            m.ratio,
            0.0012,
            &format!("{} vs Photoshop probe", m.font),
        );
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} probe fonts checked");
}

/// The five ratios quoted in the task, on the exact font files Photoshop was
/// measured with.
#[test]
fn named_photoshop_ratios() {
    let cases: [(&str, &str, f32); 5] = [
        ("CCGrimNGritty Regular.otf", "CCGrimNGritty-Regular", 0.781),
        ("CCMeanwhile.otf", "CCMeanwhile", 0.813),
        ("CCSpellcaster.otf", "CCSpellcaster", 0.840),
        ("v_AgencyFB_v1.1.ttf", "v_AgencyFB-Reg", 0.7644),
        (
            "VSC_Fix_Teslics_Document_Cyr.otf",
            "TeslicsDocumentCyr-Normal",
            0.826,
        ),
    ];
    for (file, name, want) in cases {
        let path = format!("{FONTAS}/{file}");
        let Some(data) = read_font(&path) else {
            continue;
        };
        let face = Face::parse(&data).expect("parse face");
        close(face.ascent_ratio(), want, 0.0015, name);
    }
}

/// `glyf` bounds must come from the drawn curve, not the glyph header: for
/// Vampire95 'd' the header says `yMax = 467` (0.467 em) while the exact
/// quadratic extrema top is 466.0274 (0.466027 em) — the value Photoshop uses.
#[test]
fn glyf_bbox_uses_curve_extrema() {
    let path = format!("{FONTAS}/vampire.ttf");
    let Some(data) = read_font(&path) else { return };
    let face = Face::parse(&data).expect("parse face");
    let gid = face.glyph_index('d').expect("'d' in Vampire95");
    let (_, _, _, y_max) = face.glyph_bbox(gid).expect("bbox");
    close(y_max, 466.0274, 0.01, "Vampire95 'd' exact yMax");
    close(
        face.ascent_ratio(),
        0.4660274,
        2e-4,
        "Vampire95 ascent ratio",
    );
    assert!(
        y_max < 466.9,
        "glyf header yMax (467) leaked into the bbox: {y_max}; curve extrema were not solved"
    );
}

/// A CFF glyph must produce contours the quadratic-only rasteriser can fill:
/// flattened, all on-curve, with ink where the bbox says it should be.
#[test]
fn cff_glyph_rasters_non_empty() {
    let path = format!("{FONTAS}/CCGrimNGritty Regular.otf");
    let Some(data) = read_font(&path) else { return };
    let face = Face::parse(&data).expect("parse face");
    let gid = face.glyph_index('A').expect("'A'");
    let contours = glyph_contours(&face, gid).expect("CFF contours");
    assert!(!contours.is_empty(), "CFF outline is empty");
    assert!(
        contours.iter().all(|c| c.iter().all(|p| p.on)),
        "flattened CFF contours must be all on-curve"
    );
    assert!(contours.iter().all(|c| c.len() > 1));

    let (x0, y0, x1, y1) = face.glyph_bbox(gid).expect("bbox");
    let upm = face.units_per_em() as f32;
    let size = 64.0f32;
    let scale = size / upm;
    let baseline = 200.0f32;
    let mut canvas = Canvas::new(240, 240);
    canvas.mask_contours(&contours, scale, 20.0, baseline, [0, 0, 0], 1.0);
    let (left, top, right, bottom) = canvas.ink_bounds();
    assert!(right > left && bottom > top, "CFF glyph rendered no ink");

    // The scanline mask covers every pixel column/row the outline touches, so
    // the ink bounds are exactly the floor of the left/top edge and the ceil of
    // the right/bottom edge (partial AA edge pixels are kept). y is measured
    // downward from the canvas top; the baseline is at y=200.
    let want = (
        (20.0 + x0 * scale).floor() as i32,
        (baseline - y1 * scale).floor() as i32,
        (20.0 + x1 * scale).ceil() as i32,
        (baseline - y0 * scale).ceil() as i32,
    );
    assert_eq!(
        (left, top, right, bottom),
        want,
        "CFF ink bounds: bbox ({x0}, {y0}, {x1}, {y1}) at {scale} px/unit"
    );
}

/// CFF and `glyf` faces must be told apart by `Face::has_glyf`/`has_cff`; a
/// `.otf` with `glyf` (Teslics) and a `.ttf` with `CFF ` (VelvetDrop) both
/// exist in the corpus and must take the matching path.
#[test]
fn outline_flavour_dispatch() {
    let cases: [(&str, bool, bool); 4] = [
        ("CCGrimNGritty Regular.otf", true, false),
        ("VelvetDrop.ttf", true, false),
        ("VSC_Fix_Teslics_Document_Cyr.otf", false, true),
        ("v_CCWildButNotCrazy.ttf", false, true),
    ];
    for (file, has_cff, has_glyf) in cases {
        let path = format!("{FONTAS}/{file}");
        let Some(data) = read_font(&path) else {
            continue;
        };
        let face = Face::parse(&data).expect("parse face");
        assert_eq!(face.has_cff(), has_cff, "{file}: has_cff");
        assert_eq!(face.has_glyf(), has_glyf, "{file}: has_glyf");
    }
}

/// Two different CFF fonts at the two known wine-font locations must agree
/// with each other: the same file must parse to the same outlines no matter
/// where it is loaded from.
#[test]
fn cff_same_file_same_outlines() {
    let local = format!("{FONTAS}/CCMeanwhile.otf");
    let installed = format!("{WINE_FONTS}/CCMeanwhile.otf");
    let (Some(a), Some(b)) = (read_font(&local), read_font(&installed)) else {
        return;
    };
    let (fa, fb) = (
        Face::parse(&a).expect("local face"),
        Face::parse(&b).expect("installed face"),
    );
    for ch in ['d', 'o', 'A', 'g'] {
        let (Some(ga), Some(gb)) = (fa.glyph_index(ch), fb.glyph_index(ch)) else {
            continue;
        };
        assert_eq!(
            glyph_path(&fa, ga).expect("local path"),
            glyph_path(&fb, gb).expect("installed path"),
            "'{ch}' differs between font copies"
        );
    }
}
