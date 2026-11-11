//! Prints the size of our EngineData writer output and the first divergence.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap();
    let raw = std::fs::read(&path)?;
    let v = photocraft_text::engine_data::parse(&raw)?;
    let out = photocraft_text::engine_data::write(&v);
    println!("raw {} bytes, write {} bytes", raw.len(), out.len());
    if let Some((i, (a, b))) = raw.iter().zip(out.iter()).enumerate().find(|(_, (a, b))| a != b) {
        let lo = i.saturating_sub(40);
        println!("first diff at {i}");
        println!("raw : {:?}", String::from_utf8_lossy(&raw[lo..(i + 60).min(raw.len())]));
        println!("ours: {:?}", String::from_utf8_lossy(&out[lo..(i + 60).min(out.len())]));
        let _ = (a, b);
    }
    std::fs::write("/tmp/ed_ours.bin", &out)?;
    Ok(())
}
