//! Renders point text (single baseline, no wrapping) into a PSD layer.
//!
//! `cargo run --example point_render -- <font.ttf> <out.psd> <text> <size> <x> <y> [metrics|optical]`

use photocraft_psd::builder::{LayerSpec, PixelData, PsdBuilder};
use photocraft_psd::{ColorMode, Compression};
use photocraft_text::font::Face;
use photocraft_text::layout::{Kerning, TextStyle, layout_point};
use photocraft_text::outline::glyph_contours;
use photocraft_text::raster::Canvas;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 8 {
        eprintln!("usage: point_render <font> <out.psd> <text> <size> <x> <y> [metrics|optical]");
        return Ok(());
    }
    let bytes = std::fs::read(&a[1])?;
    let face = Face::parse(&bytes)?;
    let text = a[3].clone();
    let size: f32 = a[4].parse()?;
    let x: f32 = a[5].parse()?;
    let y: f32 = a[6].parse()?;
    let kerning = if a.get(7).map(String::as_str) == Some("optical") {
        Kerning::Optical
    } else {
        Kerning::Metrics
    };
    let style = TextStyle {
        size,
        kerning,
        ..Default::default()
    };
    let lines = layout_point(&face, &text, &style, (0.0, 0.0));

    let (w, h) = (1200u32, 400u32);
    let margin = (size.ceil() * 2.0) as f32;
    let mut canvas = Canvas::new(w, h);
    let upm = face.units_per_em() as f32;
    let scale = size / upm;
    for (i, line) in lines.iter().enumerate() {
        let baseline = y + i as f32 * style.leading_pt();
        for g in &line.glyphs {
            let c = glyph_contours(&face, g.gid)?;
            if c.is_empty() {
                continue;
            }
            canvas.mask_contours(&c, scale, g.x + x + margin, baseline, [0, 0, 0], 1.0);
        }
    }
    let (min_x, min_y, max_x, max_y) = canvas.ink_bounds();
    let (ow, oh) = ((max_x - min_x) as u32, (max_y - min_y) as u32);
    let mut rgba = Vec::with_capacity((ow * oh * 4) as usize);
    for yy in min_y..max_y {
        let start = ((yy * w as i32 + min_x) * 4) as usize;
        rgba.extend_from_slice(&canvas.pixels[start..start + (ow * 4) as usize]);
    }
    let lx = min_x - margin as i32;
    let ly = min_y;
    let mut builder = PsdBuilder::new(w, h)
        .color_mode(ColorMode::Rgb)
        .depth(8)
        .compression(Compression::Rle)
        .resolution(72.0);
    builder.push_layer(LayerSpec::new(
        "rust_render",
        lx,
        ly,
        ow,
        oh,
        PixelData::Rgba8(rgba),
    ));
    builder.composite(PixelData::Rgba8(canvas.pixels));
    std::fs::write(&a[2], builder.to_bytes()?)?;
    println!("wrote {} ({}x{} at {}, {})", &a[2], ow, oh, lx, ly);
    Ok(())
}
