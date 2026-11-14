//! Prints the optical model's per-pair residual against Photoshop, as JSON.
//!
//! `cargo run --offline --release --example optical_residual > /tmp/optical_residual.json`
//!
//! Two blocks: the 14 class-calibrated pairs of `tests/optical_pairs.rs` through the engine
//! path (`optical_pair_chars`, 48 pt Myriad) and the 730-pair corpus at 24/48 pt through
//! `optical_pair`. Values are points; `residual = model - photoshop`.

use std::path::PathBuf;

use photocraft_text::font::Face;
use photocraft_text::optical::{optical_pair, optical_pair_chars};

fn face(path: PathBuf) -> Option<Face<'static>> {
    let bytes = std::fs::read(path).ok()?;
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    Face::parse(leaked).ok()
}

fn myriad() -> Option<Face<'static>> {
    let home = std::env::var_os("HOME")?;
    face(PathBuf::from(home).join(
        ".local/share/photocraft/wine/drive_c/Program Files/Adobe/Adobe Photoshop CC 2019/Required/PDFL/Resource/Fonts/MyriadPro-Regular.otf",
    ))
}

/// Photoshop's optical kerning, points at 48 pt (`tests/optical_pairs.rs`).
const PAIRS: &[(&str, f32)] = &[
    ("AV", -3.0), ("VA", -3.0), ("AT", -5.0), ("To", -4.0), ("Ty", -4.0),
    ("Wa", -2.0), ("Ve", -2.0), ("Yo", -4.0), ("Te", -4.0), ("Fa", -3.0),
    ("LT", -5.0), ("P.", -7.0),
];

fn main() {
    let mut out = String::from("{\n \"pairs_48pt\": [\n");
    if let Some(face) = myriad() {
        for (i, (pair, want)) in PAIRS.iter().enumerate() {
            let mut cs = pair.chars();
            let (a, b) = (cs.next().unwrap(), cs.next().unwrap());
            let (ga, gb) = (face.glyph_index(a).unwrap(), face.glyph_index(b).unwrap());
            let got = optical_pair_chars(&face, ga, gb, 48.0, a, b);
            let comma = if i + 1 == PAIRS.len() { "" } else { "," };
            out.push_str(&format!(
                "  {{\"pair\": \"{pair}\", \"photoshop\": {want:.4}, \"model\": {got:.4}, \"residual\": {:.4}}}{comma}\n",
                got - want
            ));
        }
    }
    out.push_str(" ],\n \"corpus\": {\n");
    let home = std::env::var("HOME").unwrap();
    let dir = PathBuf::from(&home).join(".local/share/photocraft/wine/drive_c/windows/Fonts");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut blocks = Vec::new();
    for size in [24.0f32, 48.0] {
        let text = std::fs::read_to_string(manifest.join("tests/assets").join("optical_corpus.txt")).unwrap();
        let mut entries = Vec::new();
        let mut collect = false;
        let mut current = String::new();
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = t.split_whitespace().collect();
            if f.len() == 2 && f[0].ends_with(".ttf") {
                collect = (f[1].parse::<f32>().unwrap() - size).abs() < 0.01;
                current = f[0].to_string();
                continue;
            }
            if !collect || f.len() != 2 {
                continue;
            }
            let (a, b) = (f[0].chars().next().unwrap(), f[0].chars().nth(1).unwrap());
            let Some(face) = face(dir.join(&current)) else { continue };
            let (Some(ga), Some(gb)) = (face.glyph_index(a), face.glyph_index(b)) else { continue };
            let want: f32 = f[1].parse().unwrap();
            let got = optical_pair(&face, ga, gb, size);
            entries.push(format!(
                "  {{\"font\": \"{}\", \"pair\": \"{}\", \"size\": {size}, \"photoshop\": {want:.4}, \"model\": {got:.4}, \"residual\": {:.4}}}",
                current, f[0], got - want
            ));
        }
        blocks.push(format!("  \"{size}pt\": [\n{}\n  ]", entries.join(",\n")));
    }
    out.push_str(&blocks.join(",\n"));
    out.push_str(" }\n}\n");
    print!("{out}");
}
