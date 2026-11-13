//! Prints the geometry the warp adapter needs, per text layer: stored rect,
//! `TySh` transform, flat layout bounds (descriptor + trailing floats), ink
//! bounds and warp parameters.
use photocraft_psd::PsdFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: warp_debug <psd>")?;
    let file = PsdFile::from_bytes(&std::fs::read(&path)?)?;
    for (ordinal, (index, _)) in file.text_layers().enumerate() {
        let record = &file.layers()[index];
        let rect = record.rect;
        let Some(block) = record.block(b"TySh") else {
            continue;
        };
        let tysh = photocraft_psd::text_layer::TySh::from_bytes(&block.data)?;
        println!(
            "{ordinal:2} rect=({},{},{},{}) t=({:.3},{:.3}) bounds={:?} check_bounds={:?} ink={:?} warp={:?}",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            tysh.transform[4],
            tysh.transform[5],
            tysh.layout_bounds(),
            tysh.bounds_points(),
            tysh.ink_bounds(),
            tysh.warp_settings(),
        );
    }
    Ok(())
}
