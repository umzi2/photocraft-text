//! Prints the shaped glyph id of every glyph of one string for one font/size.
//!
//! `cargo run --example glyph_gids -- CCGrimNGritty-Regular 32 "fi"`
//!
//! Used by `tools/capture_ligature_atlas.py`: ligatures and other substituted
//! glyphs have no codepoint of their own, so the capture tool keys their
//! cells by the glyph id this prints.

use std::path::PathBuf;

use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::{self, FontSet, RunStyle};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: glyph_gids <font> <size> <text>".into());
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
    let key = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let wanted = key(font);
    let mut bytes = None;
    for dir in &dirs {
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
        // The Photoshop font family name and the file name need not match
        // (`CCGrimNGritty-Regular` lives in `CCGrimNGritty Regular.otf`), so
        // fall back to matching the parsed family name.
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("otf" | "ttf")) {
                continue;
            }
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let Ok(face) = Face::parse(&data) else {
                continue;
            };
            if face.family_name().is_some_and(|name| key(&name) == wanted) {
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
        println!("{}\t{}\t{:.4}\t{:.4}", g.gid, g.ch, g.x, g.advance);
    }
    Ok(())
}
