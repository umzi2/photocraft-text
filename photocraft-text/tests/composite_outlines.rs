//! `f_f_i`-style ligatures are composite `glyf` entries; their components carry
//! no scale, so the implicit transform is the identity.  A regression here made
//! every composite glyph flat (y = 0) and ligatures rendered blank.

use std::path::PathBuf;

use photocraft_text::font::Face;
use photocraft_text::font::gsub::{Gsub, Span};

fn font() -> Face<'static> {
    let path = PathBuf::from(std::env::var("HOME").unwrap())
        .join(".local/share/photocraft/wine/drive_c/windows/Fonts/NotoSans-Regular.ttf");
    let bytes = std::fs::read(path).expect("NotoSans-Regular.ttf in the Wine prefix");
    Face::parse(Box::leak(bytes.into_boxed_slice())).expect("parse NotoSans")
}

#[test]
fn latin_ligature_glyph_is_not_flat() {
    let face = font();
    let f = face.glyph_index('f').unwrap();
    let i = face.glyph_index('i').unwrap();
    let table = Gsub::parse(face.table(*b"GSUB").unwrap()).unwrap();
    let spans = vec![
        Span {
            gid: f,
            start: 0,
            len: 1,
        },
        Span {
            gid: f,
            start: 1,
            len: 1,
        },
        Span {
            gid: i,
            start: 2,
            len: 1,
        },
    ];
    let mut out = spans;
    for lookup in table.lookups(&[*b"latn", *b"DFLT"], &[*b"liga"]) {
        out = table.ligate(lookup, &out);
    }
    assert_eq!(out.len(), 1, "f f i must become one ligature span");
    let (_, y_min, _, y_max) = face.glyph_bbox(out[0].gid).expect("bbox");
    assert!(
        y_max - y_min > 100.0,
        "composite ligature collapsed: bbox {y_min}..{y_max}"
    );
    let contours = photocraft_text::outline::glyph_contours(&face, out[0].gid).unwrap();
    let tallest = contours
        .iter()
        .flatten()
        .map(|p| p.y)
        .fold(f32::MIN, f32::max);
    assert!(tallest > 100.0, "glyph contour points lost their y offsets");
}
