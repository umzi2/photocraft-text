//! Renders one glyph's **raw outline** through our rasteriser as a straight
//! alpha mask (PGM), mirroring the CoolType capture harness: 200×240, pen
//! `(x, baseline)`, `scale = size_pt / upem`, no atlas and no hinting.
//!
//! `cargo run --example render_glyph_mask -- <font> <char> <size> <out.pgm> [x y aa]`

use photocraft_text::font::Face;
use photocraft_text::outline::glyph_contours;
use photocraft_text::raster::{AntiAlias, Canvas, Coverage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: render_glyph_mask <font> <char> <size> <out.pgm> [x y aa_code]");
        return Ok(());
    }
    let bytes = std::fs::read(&a[1])?;
    let face = Face::parse(&bytes)?;
    let ch = a[2].chars().next().expect("one character");
    let size: f32 = a[3].parse()?;
    let out = &a[4];
    let x: f32 = a.get(5).map(|s| s.parse()).transpose()?.unwrap_or(40.0);
    let y: f32 = a.get(6).map(|s| s.parse()).transpose()?.unwrap_or(160.0);
    let aa: i64 = a.get(7).map(|s| s.parse()).transpose()?.unwrap_or(4);

    let gid = face.glyph_index(ch).expect("glyph in cmap");
    let scale = size / face.units_per_em() as f32;
    let fit = std::env::var("PHOTOCRAFT_FIT").as_deref() == Ok("1");
    let contours = match (fit, face.cff()) {
        (true, Some(cff)) => {
            let path = cff.fit_stems_to_quarter(gid, scale, x, y, aa as i32)?;
            photocraft_text::outline::path_to_contours(&path)
        }
        _ => glyph_contours(&face, gid)?,
    };
    let w: u32 = a.get(8).map(|s| s.parse()).transpose()?.unwrap_or(200);
    let h: u32 = a.get(9).map(|s| s.parse()).transpose()?.unwrap_or(240);
    let mut canvas = Canvas::new(w, h);
    canvas.aa = AntiAlias::from_code(aa).unwrap_or(AntiAlias::Sharp);
    if std::env::var("PHOTOCRAFT_SNAP").as_deref() == Ok("0") {
        canvas.snap = false;
    }
    if let Ok(v) = std::env::var("PHOTOCRAFT_COV") {
        canvas.coverage = match v.as_str() {
            "4" => Coverage::Scanlines(4),
            "8" => Coverage::Scanlines(8),
            "16" => Coverage::Scanlines(16),
            "area" => Coverage::Area,
            "g16" => Coverage::Grid(16),
            "g15" => Coverage::Grid(15),
            "g8" => Coverage::Grid(8),
            "g32" => Coverage::Grid(32),
            "s2" => Coverage::Scanlines(2),
            _ => canvas.coverage,
        };
    }
    if let Ok(dump) = std::env::var("PHOTOCRAFT_DUMP_JSON") {
        // Fitted device-px polygon dump for cross-checking with an external
        // coverage tool (`out/verify/sync/rasters/fitted/*.json`).
        let mut json = String::from("{\"contours\":[");
        for (ci, c) in contours.iter().enumerate() {
            if ci > 0 {
                json.push(',');
            }
            json.push('[');
            for (pi, p) in c.iter().enumerate() {
                if pi > 0 {
                    json.push(',');
                }
                let dx = x + p.x * scale;
                let dy = y - p.y * scale;
                json.push_str(&format!("[{dx:.4},{dy:.4}]"));
            }
            json.push(']');
        }
        json.push_str("]}");
        std::fs::write(&dump, json)?;
    }
    canvas.mask_contours(&contours, scale, x, y, [0, 0, 0], 1.0);

    let mut pgm = Vec::with_capacity((w * h) as usize + 15);
    pgm.extend_from_slice(format!("P5\n{w} {h}\n255\n").as_bytes());
    for i in 0..(w * h) as usize {
        pgm.push(canvas.pixels[i * 4 + 3]);
    }
    std::fs::write(out, pgm)?;
    println!("gid {gid} contours {} -> {out}", contours.len());
    Ok(())
}
