//! Byte-exactness against Photoshop, through a captured glyph atlas.
//!
//! `corpus/plain_latin.psd` holds Photoshop's own prerender of
//! "Hamburgefonstiv AVATAR To Ty." at 48 pt, Sharp. With `out/atlas/noto48`
//! (captured by `tools/make_glyph_atlas.py`) our render must be identical —
//! not "close", identical. The test skips when the atlas has not been
//! captured, because building one needs a Photoshop session.

use std::path::PathBuf;

use photocraft_psd::PsdFile;
use photocraft_text::layer_render::{FaceStore, LayerTextStyle, RenderOptions};
use photocraft_text::runs::RunStyle;

const TEXT: &str = "Hamburgefonstiv AVATAR To Ty.";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn fonts_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME");
    [&home, ".local/share/photocraft/wine/drive_c/windows/Fonts"]
        .iter()
        .collect()
}

/// The type layer of `corpus/plain_latin.psd` as straight RGBA (transparent
/// where the layer has no alpha channel).
fn photoshop_layer() -> (u32, u32, Vec<u8>) {
    let bytes = std::fs::read(repo().join("corpus/plain_latin.psd")).expect("corpus psd");
    let file = PsdFile::from_bytes(&bytes).expect("parse corpus");
    let depth = file.header.depth;
    let version = file.header.version;
    let layer = file
        .layers()
        .iter()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .expect("text layer");
    let (w, h) = (layer.rect.width() as usize, layer.rect.height() as usize);
    let red = layer.decode_channel(0, depth, version).expect("R");
    let green = layer.decode_channel(1, depth, version).expect("G");
    let blue = layer.decode_channel(2, depth, version).expect("B");
    let alpha = layer
        .channel(-1)
        .map(|_| layer.decode_channel(-1, depth, version).expect("A"));
    assert_eq!(red.len(), w * h, "unexpected channel size");
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        rgba.extend_from_slice(&[
            red[i],
            green[i],
            blue[i],
            alpha.as_ref().map(|a| a[i]).unwrap_or(255),
        ]);
    }
    (w as u32, h as u32, rgba)
}

#[test]
fn atlas_render_is_byte_identical_to_photoshop() {
    let atlas_dir = repo().join("out/atlas/noto48");
    if !atlas_dir.join("atlas.txt").exists() {
        eprintln!("skipping: {} not captured", atlas_dir.display());
        return;
    }
    let mut store = FaceStore::new();
    assert!(store.load_dir(&fonts_dir()) > 0, "no fonts");
    store.set_atlas(photocraft_text::atlas::Atlas::load(&atlas_dir).expect("atlas"));

    let run = RunStyle {
        len: TEXT.encode_utf16().count(),
        font: Some("NotoSans-Regular".into()),
        size: Some(48.0),
        ..RunStyle::default()
    };
    let spec = LayerTextStyle {
        style: photocraft_text::layout::TextStyle {
            size: 48.0,
            ..Default::default()
        },
        paragraph: Default::default(),
        box_size: (0.0, 0.0),
        origin: (0.0, 0.0),
        color: [0, 0, 0],
    };
    let options = RenderOptions {
        aa: None,
        use_atlas: true,
    };
    let ours = store
        .render_spec_with(TEXT, &[run], &spec, options)
        .expect("render");
    let (w, h, expected) = photoshop_layer();
    assert_eq!((ours.width, ours.height), (w, h), "layer size differs");
    let differing = ours
        .rgba
        .chunks_exact(4)
        .zip(expected.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differing, 0, "{differing} pixels differ from Photoshop");
}
