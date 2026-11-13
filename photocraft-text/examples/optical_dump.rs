//! Prints the shape-based optical kerning model for the pairs of
//! `AVATAR To Ty.` at a given size; compare with Photoshop's own kerning.
use photocraft_text::font::Face;
use photocraft_text::optical::optical_pair;
use std::path::PathBuf;

fn main() {
    let home = std::env::var("HOME").unwrap();
    let path: PathBuf = [
        &home,
        ".local/share/photocraft/wine/drive_c/windows/Fonts",
        "NotoSans-Regular.ttf",
    ]
    .iter()
    .collect();
    let bytes = std::fs::read(&path).unwrap();
    let face = Face::parse(&bytes).unwrap();
    let size: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(26.0);
    let text = "AVATAR To Ty.";
    let chars: Vec<char> = text.chars().collect();
    for w in chars.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ga, gb) = (face.glyph_index(a).unwrap(), face.glyph_index(b).unwrap());
        println!(
            "{a}{b}: pair={:+.3} pt  ({:+.0}/1000 em)",
            optical_pair(&face, ga, gb, size),
            optical_pair(&face, ga, gb, size) / size * 1000.0
        );
    }
}
