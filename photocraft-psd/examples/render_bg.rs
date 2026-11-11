//! Renders a PSD background with our own rasteriser.
//!
//! `cargo run --example render_bg -- out/bg.psd 1200 800 seed`
//!
//! The pixels come from this program, not from Photoshop: a vertical gradient,
//! a radial glow, deterministic grain and a vignette. The file has a single
//! layer plus the merged composite, so Photoshop opens it as a flat background.

use photocraft_psd::builder::{LayerSpec, PixelData, PsdBuilder};
use photocraft_psd::{ColorMode, Compression};

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// xorshift32, so the grain is reproducible from the seed.
fn rng(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x >> 8) as f32 / 16_777_216.0
}

fn render(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    let mut state = seed | 1;
    let w = width as f32;
    let h = height as f32;
    // Glow centre wanders with the seed, but stays well inside the frame.
    let gx = w * (0.25 + 0.5 * rng(&mut state));
    let gy = h * (0.25 + 0.5 * rng(&mut state));
    let radius = h * (0.35 + 0.25 * rng(&mut state));
    for y in 0..height {
        let fy = y as f32 / h;
        for x in 0..width {
            let fx = x as f32 / w;
            // Base gradient: night blue at the top, warm sand at the bottom.
            let mut r = lerp(24.0, 236.0, smoothstep(fy));
            let mut g = lerp(30.0, 214.0, smoothstep(fy.powf(0.85)));
            let mut b = lerp(64.0, 172.0, smoothstep(fy.powf(0.7)));
            // Radial glow.
            let dx = x as f32 - gx;
            let dy = y as f32 - gy;
            let d = (dx * dx + dy * dy).sqrt() / radius;
            let glow = (1.0 - smoothstep(d)) * 0.55;
            r = lerp(r, 255.0, glow);
            g = lerp(g, 246.0, glow * 0.92);
            b = lerp(b, 210.0, glow * 0.65);
            // Vignette.
            let cx = fx - 0.5;
            let cy = fy - 0.5;
            let v = 1.0 - 0.55 * (cx * cx + cy * cy).sqrt() * 1.35;
            r *= v;
            g *= v;
            b *= v;
            // Grain.
            let n = (rng(&mut state) - 0.5) * 7.0;
            out.push((r + n).clamp(0.0, 255.0) as u8);
            out.push((g + n).clamp(0.0, 255.0) as u8);
            out.push((b + n).clamp(0.0, 255.0) as u8);
            out.push(255);
        }
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: render_bg <out.psd> <width> <height> [seed]");
        return Ok(());
    }
    let width: u32 = args[2].parse()?;
    let height: u32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(800);
    let seed: u32 = args
        .get(4)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5eed_1234);
    let pixels = render(width, height, seed);

    let mut builder = PsdBuilder::new(width, height)
        .color_mode(ColorMode::Rgb)
        .depth(8)
        .compression(Compression::Rle)
        .resolution(72.0);
    builder.push_layer(LayerSpec::new(
        "background",
        0,
        0,
        width,
        height,
        PixelData::Rgba8(pixels.clone()),
    ));
    builder.composite(PixelData::Rgba8(pixels));
    let bytes = builder.to_bytes()?;
    std::fs::write(&args[1], &bytes)?;
    println!("wrote {} ({} bytes, seed {})", args[1], bytes.len(), seed);
    Ok(())
}
