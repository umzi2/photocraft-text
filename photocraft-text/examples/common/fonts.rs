//! Font and atlas discovery for the command-line PSD examples.

use std::path::PathBuf;

use photocraft_text::layer_render::FaceStore;

pub fn load() -> Result<FaceStore, Box<dyn std::error::Error>> {
    let fonts = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/photocraft/wine/drive_c/windows/Fonts"));
    let mut store = photocraft_text::layer_render::FaceStore::new();
    if let Some(dir) = fonts {
        store.load_dir(&dir);
    }
    // Extra directories for fonts Photoshop ships outside the Wine font dir
    // (CJK faces live in `Required/PDFL/Resource/Fonts`), colon-separated.
    if let Some(list) = std::env::var_os("PHOTOCRAFT_FONT_DIRS") {
        for dir in std::env::split_paths(&list) {
            store.load_dir(&dir);
        }
    }
    if let Some(atlas) = std::env::var_os("PHOTOCRAFT_ATLAS") {
        store.set_atlas(
            photocraft_text::atlas::Atlas::load(std::path::Path::new(&atlas))
                .map_err(|e| format!("atlas {e}"))?,
        );
    }
    Ok(store)
}
