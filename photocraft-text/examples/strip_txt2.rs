//! Removes the document-level `Txt2` block, so the text layer only carries
//! `TySh`. Says whether Photoshop then falls back to the modern block.
//!
//! `cargo run --example strip_txt2 -- in.psd out.psd`

use photocraft_psd::PsdFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let mut file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let before = file.global_blocks.len();
    file.global_blocks.retain(|b| &b.key != b"Txt2");
    println!("global blocks {before} -> {}", file.global_blocks.len());
    std::fs::write(&a[2], file.to_bytes()?)?;
    println!("wrote {}", a[2]);
    Ok(())
}
