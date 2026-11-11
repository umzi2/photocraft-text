use photocraft_text::area::{layout_area, Justification, ParagraphStyle};
use photocraft_text::font::Face;
use photocraft_text::layout::TextStyle;

fn main() {
    let p = std::env::args().nth(1).unwrap();
    let text = std::env::args().nth(2).unwrap();
    let width: f32 = std::env::args().nth(3).unwrap().parse().unwrap();
    let bytes = std::fs::read(&p).unwrap();
    let face = Face::parse(&bytes).unwrap();
    let size: f32 = std::env::args().nth(5).and_then(|v| v.parse().ok()).unwrap_or(47.997);
    let track: i16 = std::env::args().nth(6).and_then(|v| v.parse().ok()).unwrap_or(0);
    let lead: f32 = std::env::args().nth(7).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let style = TextStyle { size, tracking: track, leading: if lead > 0.0 { Some(lead) } else { None }, ..Default::default() };
    let just = std::env::args().nth(4).unwrap_or_else(|| "left".into());
    let para = ParagraphStyle { justification: match just.as_str() { "center"=>Justification::Center, "right"=>Justification::Right, "justify"=>Justification::JustifyLastLeft, _=>Justification::Left }, ..Default::default() };
    let out = layout_area(&face, &text, &style, &para, width, 200.0, (19.998, 0.0), 0.76);
    for line in &out.lines {
        let s: String = line.glyphs.iter().map(|g| g.ch).collect();
        let upm = face.units_per_em() as f32;
        let mut lo = f32::INFINITY; let mut hi = f32::NEG_INFINITY;
        for g in &line.glyphs {
            if let Ok(c) = photocraft_text::outline::glyph_contours(&face, g.gid) {
                for contour in c.iter() {
                    for p in contour.iter() {
                        lo = lo.min(g.x + p.x * size / upm);
                        hi = hi.max(g.x + p.x * size / upm);
                    }
                }
            }
        }
        println!("w={:8.3} n={:2} base={:8.3} x0={:8.3} ink=[{:8.3},{:8.3}] {:?}", line.width, line.glyphs.len(), line.baseline, line.glyphs.first().map(|g| g.x).unwrap_or(0.0), lo, hi, s);
    }
}
