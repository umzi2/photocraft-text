//! Prints the vertical layout numbers used for the stored-rect rule.
//! `cargo run --example vlayout_debug -- in.psd`
use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::Value as DescValue;
use photocraft_text::{engine_data, layer_render, txt2};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: vlayout_debug <in.psd>")?;
    let bytes = std::fs::read(&path)?;
    let file = PsdFile::from_bytes(&bytes)?;
    let layer = file.layers().iter().find(|l| l.blocks.iter().any(|b| &b.key == b"TySh")).ok_or("no text layer")?;
    let block = layer.blocks.iter().find(|b| &b.key == b"TySh").ok_or("no TySh")?;
    let (desc, _) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(&block.data[52..])?;
    let Some(DescValue::RawData(raw)) = desc.descriptor.get("EngineData") else { return Err("no ed".into()) };
    let ed = engine_data::parse(raw)?;
    let editor = ed.get("EngineDict").and_then(|e| e.get("Editor")).and_then(|e| e.get("Text")).and_then(|t| t.as_str());
    let text = editor.unwrap_or("").trim_end_matches('\r');
    let frame = file.global_blocks.iter().find(|b| &b.key == b"Txt2").and_then(|b| txt2::object_frame_geometry(&b.data, 0));
    let mut store = layer_render::FaceStore::new();
    if let Some(home) = std::env::var_os("HOME") {
        store.load_dir(&std::path::PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/windows/Fonts"));
    }
    let layout = match frame {
        Some(f) => layer_render::layout_vertical_from_engine_data_in_box(&store, &ed, text, (0.0, 0.0), (f.right - f.left, f.bottom - f.top))?,
        None => layer_render::layout_vertical_from_engine_data(&store, &ed, text, (0.0, 0.0))?,
    };
    println!("text {:?} frame {:?}", text, frame.map(|f| (f.left, f.top, f.right, f.bottom)));
    println!("layout box_size {:?} vertical {} lines {}", layout.box_size, layout.vertical, layout.lines.len());
    for (i, l) in layout.lines.iter().enumerate() {
        println!("  line {i}: range {:?} end {:.3} x0 {:.3} x1 {:.3} glyphs {}", l.range, l.end, l.x0, l.x1, layout.glyphs.iter().filter(|g| l.range.contains(&g.range.start)).count());
    }
    let ys: Vec<f32> = layout.glyphs.iter().map(|g| g.y).collect();
    println!("  glyph ys min {:?} max {:?}", ys.iter().cloned().fold(f32::INFINITY, f32::min), ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
    let pixels = match frame {
        Some(f) => layer_render::render_vertical_from_engine_data_in_box(&store, &ed, text, (0.0, 0.0), (f.right - f.left, f.bottom - f.top))?,
        None => layer_render::render_vertical_from_engine_data(&store, &ed, text, (0.0, 0.0))?,
    };
    println!("render {}x{} offset {:?}", pixels.width, pixels.height, pixels.offset);
    Ok(())
}
