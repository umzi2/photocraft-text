//! Writes a PSD's `Txt2` block to stdout as escaped text, for diffing two files.

use photocraft_psd::PsdFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let block = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .ok_or("no Txt2 block")?;
    for &b in &block.data {
        match b {
            b'\n' => println!(),
            b'\t' => print!("\\t"),
            0x20..=0x7e => print!("{}", b as char),
            _ => print!("\\x{b:02x}"),
        }
    }
    Ok(())
}
