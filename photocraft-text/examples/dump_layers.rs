//! Prints the text-layer parameters the renderer needs: layer rect, `TySh`
//! transform, orientation, warp settings, frame and first style run.
use photocraft_psd::PsdFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: dump_layers <psd>")?;
    let file = PsdFile::from_bytes(&std::fs::read(&path)?)?;
    for (ordinal, (index, _)) in file.text_layers().enumerate() {
        let record = &file.layers()[index];
        let rect = record.rect;
        let block = record.block(b"TySh").ok_or("no TySh")?.data.clone();
        let tysh = photocraft_psd::text_layer::TySh::from_bytes(&block)?;
        let Some(ed) = tysh
            .engine_data()
            .and_then(|b| photocraft_text::engine_data::parse(b).ok())
        else {
            println!("{ordinal:2} idx={index:2} no EngineData");
            continue;
        };
        let runs = photocraft_text::runs::paragraphs_from_engine_data(&ed);
        let styles = photocraft_text::runs::from_engine_data(&ed);
        let warp = tysh.warp_settings();
        println!(
            "{ordinal:2} idx={index:2} rect=({},{},{},{}) t=({:.3},{:.3}) ornt={:?} warp={{style:{}, value:{}, persp:{}, other:{}, rot:{:?}}} frame={:?} size={:?} font={:?} paras={}",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            tysh.transform[4],
            tysh.transform[5],
            tysh.orientation(),
            warp.style,
            warp.value,
            warp.perspective,
            warp.perspective_other,
            warp.rotate,
            tysh.layout_bounds(),
            styles.first().and_then(|s| s.size),
            styles.first().and_then(|s| s.font.clone()),
            runs.len()
        );
    }
    Ok(())
}
