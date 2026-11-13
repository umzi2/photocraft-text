//! Renders text with our own engine into a PSD preview.
//!
//! `cargo run --example preview -- <font.ttf> <out_prefix> [count] [seed]`
//!
//! Each preview is a flat image: a gradient background computed here, plus the
//! text laid out by `layout_area`, rasterised by `raster::Canvas` and composited.
//! Writing it as a PSD makes Photoshop show exactly the pixels this program
//! produced, so our layout can be compared with Photoshop's own rendering.

use photocraft_psd::builder::{LayerSpec, PixelData, PsdBuilder};
use photocraft_psd::{ColorMode, Compression};
use photocraft_text::area::{Justification, ParagraphStyle, layout_area};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::outline::glyph_contours;
use photocraft_text::raster::Canvas;

const W: u32 = 1200;
const H: u32 = 800;

const TEXTS: [&str; 4] = [
    "Привет, мир! Это текст в рамке, который переносится по словам.",
    "Дизайн и типографика: случайная коробка, случайный кегль, случайное выравнивание.",
    "Проверка вёрстки абзаца. Перенос строк, трекинг и интерлиньяж работают вместе.",
    "Текст на русском языке в произвольном месте холста, обрезанный по границам бокса.",
];

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / 16_777_216.0
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.unit() * (hi - lo) as f32) as i32
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.range(0, items.len() as i32) as usize]
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn background(seed: u32) -> Canvas {
    let mut rng = Rng(seed | 1);
    let gx = W as f32 * (0.25 + 0.5 * rng.unit());
    let gy = H as f32 * (0.25 + 0.5 * rng.unit());
    let radius = H as f32 * (0.35 + 0.25 * rng.unit());
    let mut canvas = Canvas::new(W, H);
    canvas.fill_with(|x, y| {
        let fx = x as f32 / W as f32;
        let fy = y as f32 / H as f32;
        let mut r = 24.0 + (236.0 - 24.0) * smoothstep(fy);
        let mut g = 30.0 + (214.0 - 30.0) * smoothstep(fy.powf(0.85));
        let mut b = 64.0 + (172.0 - 64.0) * smoothstep(fy.powf(0.7));
        let dx = x as f32 - gx;
        let dy = y as f32 - gy;
        let d = (dx * dx + dy * dy).sqrt() / radius;
        let glow = (1.0 - smoothstep(d)) * 0.5;
        r += (255.0 - r) * glow;
        g += (246.0 - g) * glow * 0.9;
        b += (210.0 - b) * glow * 0.6;
        let cx = fx - 0.5;
        let cy = fy - 0.5;
        let v = 1.0 - 0.5 * (cx * cx + cy * cy).sqrt() * 1.3;
        [
            (r * v).clamp(0.0, 255.0) as u8,
            (g * v).clamp(0.0, 255.0) as u8,
            (b * v).clamp(0.0, 255.0) as u8,
        ]
    });
    canvas
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: preview <font.ttf> <out_prefix> [count] [seed]");
        return Ok(());
    }
    let font_bytes = std::fs::read(&args[1])?;
    let prefix = args[2].clone();
    let count: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(4);
    let seed: u32 = args
        .get(4)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x51ee_d0);
    let face = Face::parse(&font_bytes)?;
    let upm = face.units_per_em() as f32;
    let ascender = photocraft_text::area::first_baseline_ratio(&face);
    let mut rng = Rng(seed | 1);

    for i in 1..=count {
        let size = rng.pick(&[28.0, 32.0, 36.0, 42.0, 48.0, 56.0, 64.0]);
        let box_w = rng.range(320, 720) as f32;
        let box_h = rng.range(140, 380) as f32;
        let x = rng.range(60, (W as f32 - box_w - 40.0) as i32) as f32;
        let y = rng.range(80, (H as f32 - box_h + 100.0) as i32) as f32;
        let justification = rng.pick(&[
            Justification::Left,
            Justification::Center,
            Justification::Right,
            Justification::JustifyLastLeft,
        ]);
        let tracking = rng.pick(&[0i16, 0, 20, -15, 50]);
        let text = rng.pick(&TEXTS);
        let color: [u8; 3] = rng.pick(&[
            [27, 27, 27],
            [248, 244, 232],
            [43, 29, 58],
            [255, 255, 255],
            [255, 92, 51],
        ]);
        let leading = size * rng.pick(&[1.2f32, 1.35, 1.6]);

        let style = TextStyle {
            size,
            tracking,
            leading: Some(leading),
            ..Default::default()
        };
        let para = ParagraphStyle {
            justification,
            ..Default::default()
        };
        let layout = layout_area(&face, text, &style, &para, box_w, box_h, (x, y), ascender);

        let bg_seed = seed.wrapping_add(i as u32).wrapping_mul(2_654_435_761);
        let mut canvas = background(bg_seed);
        let bg_pixels = canvas.pixels.clone();
        // One mask per glyph; the canvas composites each over the background.
        // The layout uses Photoshop's y-up space; the canvas is y-down, so the
        // baseline of line i sits `leading * i` below the first baseline.
        let first_baseline = layout.lines.first().map(|l| l.baseline).unwrap_or(0.0);
        for line in &layout.lines {
            let screen_baseline = y + ascender * size + (first_baseline - line.baseline);
            for g in &line.glyphs {
                let contours = glyph_contours(&face, g.gid)?;
                if contours.is_empty() {
                    continue;
                }
                canvas.fill_contours(&contours, size / upm, g.x, screen_baseline, color, 0.92);
            }
        }

        // Background on its own: the Photoshop side opens it and adds a text
        // layer, so both renders share identical pixels underneath.
        let mut bg = PsdBuilder::new(W, H)
            .color_mode(ColorMode::Rgb)
            .depth(8)
            .compression(Compression::Rle)
            .resolution(72.0);
        bg.push_layer(LayerSpec::new(
            "background",
            0,
            0,
            W,
            H,
            PixelData::Rgba8(bg_pixels.clone()),
        ));
        bg.composite(PixelData::Rgba8(bg_pixels));
        std::fs::write(format!("{prefix}_{i}_bg.psd"), bg.to_bytes()?)?;

        let out = format!("{prefix}_{i}.psd");
        let mut builder = PsdBuilder::new(W, H)
            .color_mode(ColorMode::Rgb)
            .depth(8)
            .compression(Compression::Rle)
            .resolution(72.0);
        builder.push_layer(LayerSpec::new(
            "render",
            0,
            0,
            W,
            H,
            PixelData::Rgba8(canvas.pixels.clone()),
        ));
        builder.composite(PixelData::Rgba8(canvas.pixels.clone()));
        let bytes = builder.to_bytes()?;
        std::fs::write(&out, &bytes)?;

        // PPM next to it, for eyeballing without Photoshop.
        let mut ppm = Vec::with_capacity((W * H * 3 + 32) as usize);
        ppm.extend_from_slice(format!("P6\n{W} {H}\n255\n").as_bytes());
        for px in canvas.pixels.chunks_exact(4) {
            ppm.extend_from_slice(&px[..3]);
        }
        std::fs::write(format!("{prefix}_{i}.ppm"), &ppm)?;
        // Params for the Photoshop side of the comparison.
        let hex = format!("{:02x}{:02x}{:02x}", color[0], color[1], color[2]);
        let just = match justification {
            Justification::Left => "left",
            Justification::Center => "center",
            Justification::Right => "right",
            Justification::JustifyLastLeft => "justify",
            Justification::JustifyLastCenter => "justify-center",
            Justification::JustifyLastRight => "justify-right",
            Justification::JustifyAll => "justify-all",
        };
        let spec = format!(
            "{{\"text\":{text:?},\"size\":{size},\"x\":{x},\"y\":{y},\"width\":{box_w},\"height\":{box_h},\"color\":\"{hex}\",\"justification\":\"{just}\",\"tracking\":{tracking},\"leading\":{leading},\"seed\":{}}}",
            bg_seed
        );
        std::fs::write(format!("{prefix}_{i}.json"), &spec)?;
        println!(
            "demo {i}: size {size} box {box_w}x{box_h} at ({x},{y}) just {just} tracking {tracking} lines {} -> {out}",
            layout.lines.len()
        );
    }
    Ok(())
}
