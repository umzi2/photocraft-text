use photocraft_text::font::Face;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let ch = std::env::args().nth(2).unwrap().chars().next().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let face = Face::parse(&bytes).unwrap();
    let gid = face.glyph_index(ch).unwrap();
    let c = photocraft_text::outline::glyph_contours(&face, gid).unwrap();
    println!("gid {gid} contours {}", c.len());
    for (i, contour) in c.iter().enumerate() {
        let (mut lo, mut hi, mut ylo, mut yhi) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for p in contour {
            lo = lo.min(p.x);
            hi = hi.max(p.x);
            ylo = ylo.min(p.y);
            yhi = yhi.max(p.y);
        }
        println!(
            "  c{i}: {} pts x[{lo:.0},{hi:.0}] y[{ylo:.0},{yhi:.0}]",
            contour.len()
        );
    }
}
