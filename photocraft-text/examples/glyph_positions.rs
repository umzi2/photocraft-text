//! Prints the pen position of every glyph of one string for one font/size.
//!
//! `cargo run --example glyph_positions -- NotoSans-Regular 22 "H a m"`
//!
//! Used by `tools/capture_atlas_batch.py`: Photoshop places each glyph at the
//! rounded pen position, so the capture tool needs the same positions to lift
//! one glyph's cell out of a multi-glyph text layer.

use std::path::PathBuf;

use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::{self, FontSet, RunStyle};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: glyph_positions <font> <size> <text>".into());
    }
    let font = &args[1];
    let size: f32 = args[2].parse()?;
    let text = &args[3];

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/windows/Fonts"));
    }
    if let Some(list) = std::env::var_os("PHOTOCRAFT_FONT_DIRS") {
        dirs.extend(std::env::split_paths(&list));
    }
    let mut bytes = None;
    for dir in dirs {
        for candidate in [
            format!("{font}.ttf"),
            format!("{font}.otf"),
            format!("{font}.TTF"),
        ] {
            if let Ok(data) = std::fs::read(dir.join(&candidate)) {
                bytes = Some(data);
                break;
            }
        }
        if bytes.is_some() {
            break;
        }
    }
    let data = bytes.ok_or("font not found")?;
    let face = Face::parse(&data)?;
    let faces = FontSet::new(font, face);
    let run = RunStyle {
        len: text.encode_utf16().count(),
        size: Some(size),
        ..Default::default()
    };
    let style = TextStyle {
        size,
        ..Default::default()
    };
    for g in runs::shape_runs(&faces, text, &[run], &style) {
        println!("{}\t{:.4}\t{:.4}", g.ch, g.x, g.advance);
    }
    Ok(())
}
