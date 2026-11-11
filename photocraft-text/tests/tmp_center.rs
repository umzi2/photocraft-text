use photocraft_text::area::{Justification, ParagraphStyle};
use photocraft_text::layer_render::{FaceStore, LayerTextStyle, RenderOptions};
use photocraft_text::layout::TextStyle;
use photocraft_text::runs::RunStyle;

#[test]
fn center_point_text() {
    let mut store = FaceStore::new();
    store.load_dir(std::path::Path::new("/home/umzi/.local/share/photocraft/wine/drive_c/windows/Fonts"));
    let runs = vec![RunStyle { len: 6, font: Some("NotoSans-Regular".into()), size: Some(48.0), ..RunStyle::default() }];
    let layer = LayerTextStyle {
        style: TextStyle { size: 48.0, ..Default::default() },
        paragraph: ParagraphStyle { justification: Justification::Center, ..Default::default() },
        box_size: (0.0, 0.0),
        origin: (0.0, 0.0),
        color: [0, 0, 0],
    };
    let px = store.render_spec_with("Hi\rall", &runs, &layer, RenderOptions { aa: None, use_atlas: false }).unwrap();
    println!("{}x{} offset {:?}", px.width, px.height, px.offset);
}
