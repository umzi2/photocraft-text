use photocraft_text::font::Face;
use photocraft_text::layout::{shape, TextStyle};
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let text = "The quick brown ";
    let bytes = std::fs::read(&p).unwrap();
    let face = Face::parse(&bytes).unwrap();
    let style = TextStyle { size: 47.997, ..Default::default() };
    let mut sum = 0.0;
    for g in shape(&face, text, &style) {
        println!("{:?} gid {} adv {:.4} sum {:.4}", g.ch, g.gid, g.advance, sum);
        sum += g.advance;
    }
    println!("total {sum:.4}");
}
