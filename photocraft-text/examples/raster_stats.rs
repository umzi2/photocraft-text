fn main() -> Result<(), Box<dyn std::error::Error>> {
    let f = photocraft_psd::PsdFile::from_bytes(&std::fs::read(std::env::args().nth(1).unwrap())?)?;
    for i in 0..f.text_layer_count() {
        let Some(r) = f.text_layer_raster(i) else {
            continue;
        };
        let r = r?;
        let buf: Vec<u8> = match &r.raster {
            photocraft_psd::text_layer::TextRaster::Rgba8(v) => v.clone(),
            _ => vec![],
        };
        let px = buf.chunks_exact(4);
        let nz = px.clone().filter(|p| p[3] != 0).count();
        let max = px.map(|p| p[3]).max().unwrap_or(0);
        println!(
            "text {i}: rect {:?} samples {} alpha!=0 {nz} max {max}",
            r.rect,
            buf.len() / 4
        );
    }
    Ok(())
}
