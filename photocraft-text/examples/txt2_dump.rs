//! Prints the `Txt2` run table of a PSD: one line per run with the raw run
//! dictionary text, so per-property keys can be compared across files.
//!
//! `cargo run --example txt2_dump -- corpus/plain_latin.psd`

use photocraft_psd::PsdFile;
use photocraft_text::txt2;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let block = file
        .global_block(b"Txt2")
        .ok_or("no Txt2 block")?
        .data
        .clone();
    let n = txt2::run_count(&block)?;
    println!("runs {n} lengths {:?}", txt2::run_lengths(&block)?);
    for i in 0..n {
        let path = txt2::run_path_for(i);
        let (s, e) = txt2::value_range(&block, &path)?;
        println!("run {i}: {}", String::from_utf8_lossy(&block[s..e]));
    }
    Ok(())
}
