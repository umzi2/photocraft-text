use photocraft_text::Face;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    let f = Face::parse(&bytes).unwrap();
    println!(
        "upm {} glyphs {} cmap {:?}",
        f.units_per_em(),
        f.num_glyphs(),
        f.cmap_subtable()
    );
    for c in ['H', 'a', 'A', 'ъ'] {
        println!("{c:?} -> {:?}", f.glyph_index(c));
    }
}
