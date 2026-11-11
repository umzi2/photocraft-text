//! Renders text with our engine into a transparent PSD layer.
//!
//! `cargo run --example raster_layer -- <font.ttf> <out.psd> <spec.json>`
//!
//! The spec has the same fields the Photoshop harness uses, so the resulting
//! layer can be paired with a live type layer of identical parameters: toggle
//! one against the other to judge how our rendering differs.

use photocraft_psd::builder::{LayerSpec, PixelData, PsdBuilder};
use photocraft_psd::{ColorMode, Compression};
use photocraft_text::area::{Justification, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layer_render::{render_text_layer, LayerTextStyle};
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::FontSet;
use photocraft_text::raster::Canvas;

struct Spec {
    text: String,
    size: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: [u8; 3],
    justification: String,
    tracking: i16,
    leading: f32,
    canvas: (u32, u32),
}

fn parse_just(s: &str) -> Justification {
    match s {
        "center" => Justification::Center,
        "right" => Justification::Right,
        "justify" => Justification::JustifyLastLeft,
        _ => Justification::Left,
    }
}

fn rgb(hex: &str) -> [u8; 3] {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    [((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8]
}

fn load(path: &str) -> Result<Spec, Box<dyn std::error::Error>> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let num = |k: &str, d: f32| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d);
    Ok(Spec {
        text: v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        size: num("size", 36.0),
        x: num("x", 0.0),
        y: num("y", 0.0),
        width: num("width", 600.0),
        height: num("height", 200.0),
        color: rgb(v.get("color").and_then(|x| x.as_str()).unwrap_or("000000")),
        justification: v
            .get("justification")
            .and_then(|x| x.as_str())
            .unwrap_or("left")
            .to_string(),
        tracking: num("tracking", 0.0) as i16,
        leading: num("leading", 0.0),
        canvas: (
            v.get("canvas_width").and_then(|x| x.as_u64()).unwrap_or(1200) as u32,
            v.get("canvas_height").and_then(|x| x.as_u64()).unwrap_or(800) as u32,
        ),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: raster_layer <font.ttf> <out.psd> <spec.json>");
        return Ok(());
    }
    let spec = load(&args[3])?;
    let font_bytes = std::fs::read(&args[1])?;
    let face = Face::parse(&font_bytes)?;

    let style = TextStyle {
        size: spec.size,
        tracking: spec.tracking,
        leading: Some(if spec.leading > 0.0 { spec.leading } else { spec.size * 1.2 }),
        ..Default::default()
    };
    let para = ParagraphStyle { justification: parse_just(&spec.justification), ..Default::default() };
    let layer_spec = LayerTextStyle {
        style,
        paragraph: para,
        box_size: (spec.width, spec.height),
        origin: (0.0, 0.0),
        color: spec.color,
    };
    let faces = FontSet::new("default", face);
    let rendered = render_text_layer(&faces, &spec.text, &[], &layer_spec)?;

    let (w, h) = spec.canvas;
    let lx = (spec.x + rendered.offset.0).round().max(0.0) as i64;
    let ly = (spec.y + rendered.offset.1).round().max(0.0) as i64;

    let mut canvas = Canvas::new(w, h);
    for y in 0..rendered.height as i64 {
        let dy = ly + y;
        if dy < 0 || dy >= h as i64 {
            continue;
        }
        for x in 0..rendered.width as i64 {
            let dx = lx + x;
            if dx < 0 || dx >= w as i64 {
                continue;
            }
            let src = ((y as usize * rendered.width as usize) + x as usize) * 4;
            let dst = ((dy as usize * w as usize) + dx as usize) * 4;
            canvas.pixels[dst..dst + 4].copy_from_slice(&rendered.rgba[src..src + 4]);
        }
    }

    let mut builder = PsdBuilder::new(w, h)
        .color_mode(ColorMode::Rgb)
        .depth(8)
        .compression(Compression::Rle)
        .resolution(72.0);
    builder.push_layer(LayerSpec::new(
        "rust_render",
        lx as i32,
        ly as i32,
        rendered.width,
        rendered.height,
        PixelData::Rgba8(rendered.rgba.clone()),
    ));
    builder.composite(PixelData::Rgba8(canvas.pixels));
    std::fs::write(&args[2], builder.to_bytes()?)?;
    println!("wrote {} ({}x{})", &args[2], rendered.width, rendered.height);
    Ok(())
}
