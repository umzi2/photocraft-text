//! Prints the line breaks `layout_area` produces for a box.
//!
//! `cargo run --example area_lines -- <text> <width> <size> [every]`

use photocraft_text::area::{Justification, ParagraphStyle, layout_area};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let home = std::env::var("HOME")?;
    let bytes = std::fs::read(format!(
        "{home}/.local/share/photocraft/wine/drive_c/windows/Fonts/NotoSans-Regular.ttf"
    ))?;
    let face = Face::parse(&bytes)?;
    let text = a[1].clone();
    let width: f32 = a[2].parse()?;
    let size: f32 = a[3].parse()?;
    let every = a.get(4).map(String::as_str) == Some("every");
    let style = TextStyle {
        size,
        leading: Some(30.0),
        ..Default::default()
    };
    let para = ParagraphStyle {
        justification: Justification::Left,
        every_line_composer: every,
        ..Default::default()
    };
    let out = layout_area(&face, &text, &style, &para, width, 300.0, (20.0, 0.0), 0.76);
    let verbose = a.get(5).map(String::as_str) == Some("v");
    for line in &out.lines {
        let s: String = line.glyphs.iter().map(|g| g.ch).collect();
        let last = line.glyphs.last().map(|g| g.x + g.advance).unwrap_or(0.0);
        println!("{:>7.2}  {s}", last - 20.0);
        if verbose {
            let adv: Vec<String> = line
                .glyphs
                .iter()
                .map(|g| format!("{:.2}", g.advance))
                .collect();
            println!("        {}", adv.join(" "));
        }
    }
    Ok(())
}
