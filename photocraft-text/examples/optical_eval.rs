//! Evaluates `layer_render`'s optical pair model for a list of pairs, for the
//! spacing specification's residual table.
//!
//! `cargo run --release --example optical_eval -- <font-file> <size> <pairs...>`

use std::path::Path;

use photocraft_text::font::Face;
use photocraft_text::layer_render::FaceStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = Path::new(&args[1]);
    let size: f32 = args[2].parse()?;
    let bytes = std::fs::read(path)?;
    let face = Face::parse(&bytes)?;
    // The store only matters for `first_baseline_offset_of`; the pair model takes the face.
    let _store = FaceStore::new();
    for pair in &args[3..] {
        let chars: Vec<char> = pair.chars().collect();
        let (Some(a), Some(b)) = (face.glyph_index(chars[0]), face.glyph_index(chars[1])) else {
            println!("{pair} 0");
            continue;
        };
        let v = photocraft_text::optical::optical_pair(&face, a, b, size);
        println!("{pair} {v:.6}");
    }
    Ok(())
}
