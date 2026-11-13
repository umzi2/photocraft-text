//! Prints what the engine reads out of a PSD's text layer.
//!
//! `cargo run --example layer_debug -- in.psd`

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};
use photocraft_text::engine_data;
use photocraft_text::runs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: layer_debug <in.psd>")?;
    let bytes = std::fs::read(&path)?;
    let file = PsdFile::from_bytes(&bytes)?;
    let layer = file
        .layers()
        .iter()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let block = layer
        .blocks
        .iter()
        .find(|b| &b.key == b"TySh")
        .ok_or("no TySh")?;
    let (desc, _) = VersionedDescriptor::parse_prefix(&block.data[52..])?;
    let Some(DescValue::RawData(raw)) = desc.descriptor.get("EngineData") else {
        return Err("no EngineData".into());
    };
    let ed = engine_data::parse(raw)?;
    let runs = runs::from_engine_data(&ed);
    let paras = runs::paragraphs_from_engine_data(&ed);
    println!("layer rect {:?}", layer.rect);
    for (i, r) in runs.iter().enumerate() {
        println!(
            "run {i}: len {} font {:?} idx {:?} size {:?} color {:?} track {:?} liga {:?}",
            r.len, r.font, r.font_index, r.size, r.color, r.tracking, r.underline
        );
    }
    for (i, p) in paras.iter().enumerate() {
        println!(
            "para {i}: len {} just {:?} every-line {} first-indent {} leading {}",
            p.len, p.justification, p.every_line_composer, p.first_line_indent, p.auto_leading
        );
    }
    let frame = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .and_then(|b| photocraft_text::txt2::frame_geometry(&b.data));
    println!(
        "frame {:?}",
        frame.map(|f| (f.left, f.top, f.right, f.bottom))
    );
    let editor = ed
        .get("EngineDict")
        .and_then(|e| e.get("Editor"))
        .and_then(|e| e.get("Text"))
        .and_then(|t| t.as_str());
    println!("text {:?}", editor);

    // Layout as the renderer does, and show the baselines it gets.
    let mut store = photocraft_text::layer_render::FaceStore::new();
    if let Some(home) = std::env::var_os("HOME") {
        store.load_dir(
            &std::path::PathBuf::from(home)
                .join(".local/share/photocraft/wine/drive_c/windows/Fonts"),
        );
    }
    let text = editor.unwrap_or("").trim_end_matches('\r');
    let pixels = photocraft_text::layer_render::render_from_engine_data(
        &store,
        &ed,
        text,
        (430.0, 200.0),
        (0.0, 0.0),
    )?;
    if let Some(home) = std::env::var_os("HOME") {
        let dir = std::path::PathBuf::from(home)
            .join(".local/share/photocraft/wine/drive_c/windows/Fonts");
        let path = dir.join("NotoSans-Regular.ttf");
        if let Ok(bytes) = std::fs::read(&path) {
            let face = photocraft_text::font::Face::parse(&bytes)?;
            println!(
                "NotoSans face: family {:?} ratio {} upm {}",
                face.family_name(),
                photocraft_text::area::first_baseline_ratio(&face),
                face.units_per_em()
            );
        }
    }
    println!(
        "render {}x{} offset {:?} (implied ascent {})",
        pixels.width,
        pixels.height,
        pixels.offset,
        -(pixels.offset.1)
    );
    Ok(())
}
