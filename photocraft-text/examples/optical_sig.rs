use std::path::PathBuf;
use photocraft_text::font::Face;
use photocraft_text::optical::{optical_signature, optical_target_gap, optical_pair, optical_pair_chars};
fn main() {
    let home = std::env::var("HOME").unwrap();
    let p = PathBuf::from(home).join(".local/share/photocraft/wine/drive_c/Program Files/Adobe/Adobe Photoshop CC 2019/Required/PDFL/Resource/Fonts/MyriadPro-Regular.otf");
    let bytes = std::fs::read(&p).unwrap();
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let face = Face::parse(leaked).unwrap();
    println!("upm {} asc {} desc {} cap {:?} xh {:?}", face.units_per_em(), face.hhea.ascender, face.hhea.descender,
             face.os2.and_then(|o| o.s_cap_height), face.os2.and_then(|o| o.s_x_height));
    println!("signature {:?}", optical_signature(&face));
    println!("target@24 {:.6}  target@48 {:.6}", optical_target_gap(&face, 24.0), optical_target_gap(&face, 48.0));
    for pair in ["AV", "To", "Ty.", "Wa"] {
        let mut cs = pair.chars();
        let (a, b) = (cs.next().unwrap(), cs.next().unwrap());
        let (ga, gb) = (face.glyph_index(a).unwrap(), face.glyph_index(b).unwrap());
        println!("{pair}: model {:.4} pt @24, chars {:.4} @24, chars {:.4} @48",
                 optical_pair(&face, ga, gb, 24.0), optical_pair_chars(&face, ga, gb, 24.0, a, b),
                 optical_pair_chars(&face, ga, gb, 48.0, a, b));
    }
}
