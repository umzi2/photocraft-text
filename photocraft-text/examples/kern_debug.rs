use photocraft_text::Face;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    let f = Face::parse(&bytes).unwrap();
    let text = "Hxg quick brown ro rg AV AT To";
    let chars: Vec<char> = text.chars().collect();
    for w in chars.windows(2) {
        if let (Some(a), Some(b)) = (f.glyph_index(w[0]), f.glyph_index(w[1])) {
            let k = f.kern_pair(a, b);
            if k != 0 {
                println!("{:?}{:?} gid {} {} kern {}", w[0], w[1], a, b, k);
            }
        }
    }
}
