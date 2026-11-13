//! Font names in a PSD are PostScript names (`TimesNewRomanPSMT`); installed
//! font files are named by family (`times.ttf`). `FaceStore` has to resolve
//! both, or a layer that names its font the way Photoshop does cannot be
//! rendered by the reimplementation.

use std::path::Path;

use photocraft_text::font::Face;
use photocraft_text::layer_render::FaceStore;

fn wine_fonts() -> String {
    format!(
        "{}/.local/share/photocraft/wine/drive_c/windows/Fonts",
        std::env::var("HOME").expect("HOME")
    )
}

#[test]
fn postscript_and_family_names_resolve_to_the_same_face() {
    let dir = wine_fonts();
    let mut store = FaceStore::new();
    store.load_dir(Path::new(&dir));
    for name in ["TimesNewRomanPSMT", "Times New Roman", "ArialMT", "Arial"] {
        assert!(store.face(name).is_some(), "{name} must resolve");
    }
    let bytes = std::fs::read(format!("{dir}/times.ttf")).expect("times.ttf");
    let face = Face::parse(&bytes).expect("parse");
    assert_eq!(face.postscript_name().as_deref(), Some("TimesNewRomanPSMT"));
    assert_eq!(face.family_name().as_deref(), Some("Times New Roman"));
    assert!(face.full_name().is_some());
}

#[test]
fn separators_in_a_font_name_do_not_matter() {
    let dir = wine_fonts();
    let mut store = FaceStore::new();
    store.load_dir(Path::new(&dir));
    // Photoshop writes `CCGrimNGritty-Regular`; the file is
    // `CCGrimNGritty Regular.otf`.
    assert!(store.face("CCGrimNGritty-Regular").is_some());
    assert!(store.face("CCGrimNGritty Regular").is_some());
}
