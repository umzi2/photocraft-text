//! Renders one text layer of a PSD from its `TySh` `EngineData` with the crate's
//! own layout/rasteriser and writes a PPM plus a one-line JSON of the layer
//! rectangle.  Used by `tools/compare_render.py` to diff our render against the
//! pixels Photoshop stored for the same layer.
//!
//! `cargo run --example render_psd -- file.psd <text-layer-ordinal> out.ppm [l,t,r,b]`

use std::io::Write;
use std::path::PathBuf;

use photocraft_psd::PsdFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: render_psd <psd> <text-ordinal> <out.ppm> [l,t,r,b]".into());
    }
    let file = PsdFile::from_bytes(&std::fs::read(&args[1])?)?;
    let ordinal: usize = args[2].parse()?;
    let (layer_index, _) = file
        .text_layers()
        .nth(ordinal)
        .ok_or("text layer ordinal out of range")?;
    let block = file
        .layers()
        .get(layer_index)
        .and_then(|l| l.block(b"TySh"))
        .ok_or("no TySh")?
        .data
        .clone();
    let tysh = photocraft_psd::text_layer::TySh::from_bytes(&block)?;
    let text = tysh.text().ok_or("no TySh text")?;
    let ed_bytes = tysh.engine_data().ok_or("no EngineData")?;
    let ed = photocraft_text::engine_data::parse(ed_bytes)?;

    let mut store = photocraft_text::layer_render::FaceStore::new();
    if let Some(home) = std::env::var_os("HOME") {
        store.load_dir(&PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/windows/Fonts"));
    }
    if let Some(dirs) = std::env::var_os("PHOTOCRAFT_FONT_DIRS") {
        for dir in std::env::split_paths(&dirs) {
            store.load_dir(&dir);
        }
    }

    let frame: Option<[f32; 4]> = if args.len() >= 5 {
        let v: Vec<f32> = args[4].split(',').map(|s| s.parse()).collect::<Result<_, _>>()?;
        if v.len() != 4 {
            return Err("frame must be l,t,r,b".into());
        }
        Some([v[0], v[1], v[2], v[3]])
    } else {
        None
    };
    let paras = photocraft_text::runs::paragraphs_from_engine_data(&ed);
    if std::env::var("PHOTOCRAFT_TRACE").is_ok() {
        for (i, p) in paras.iter().enumerate() {
            eprintln!("para {i}: just={:?} fli={} si={} ei={} sb={} sa={} lead={} evl={}", p.justification, p.first_line_indent, p.start_indent, p.end_indent, p.space_before, p.space_after, p.auto_leading, p.every_line_composer);
        }
    }
    let box_size = frame.map(|f| (f[2] - f[0], f[3] - f[1])).unwrap_or((0.0, 0.0));
    let pixels = photocraft_text::layer_render::render_from_engine_data(&store, &ed, &text, box_size, (0.0, 0.0))?;
    let first_off = photocraft_text::layer_render::first_baseline_offset_of(&store, &ed).unwrap_or(0.0);
    let tx = f64::from_be_bytes(block[34..42].try_into().unwrap_or([0; 8])) as f32;
    let ty = f64::from_be_bytes(block[42..50].try_into().unwrap_or([0; 8])) as f32;
    let anchor = match frame {
        Some(f) => (tx + f[0], ty + f[1] + first_off),
        None => (tx, ty),
    };
    let left = anchor.0 + pixels.offset.0;
    let top = anchor.1 + pixels.offset.1;

    let mut out = std::fs::File::create(&args[3])?;
    write!(out, "P6\n{} {}\n255\n", pixels.width, pixels.height)?;
    for px in pixels.rgba.chunks_exact(4) {
        let a = u32::from(px[3]);
        let blend = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        out.write_all(&[blend(px[0]), blend(px[1]), blend(px[2])])?;
    }
    let head: String = text.chars().take(32).map(|c| if c.is_control() { ' ' } else { c }).collect();
    let alpha_path = format!("{}.alpha.pgm", args[3]);
    let mut alpha = std::fs::File::create(alpha_path)?;
    write!(alpha, "P5\n{} {}\n255\n", pixels.width, pixels.height)?;
    for px in pixels.rgba.chunks_exact(4) {
        alpha.write_all(&[px[3]])?;
    }
    println!(
        r#"{{"left": {:.3}, "top": {:.3}, "width": {}, "height": {}, "offset": [{:.3}, {:.3}], "first_baseline": {:.3}, "text": "{}"}}"#,
        left, top, pixels.width, pixels.height, pixels.offset.0, pixels.offset.1, first_off,
        head.replace('\\', "\\\\").replace('"', "\\\"")
    );
    Ok(())
}
