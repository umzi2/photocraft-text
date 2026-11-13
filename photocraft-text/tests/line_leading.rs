//! Line spacing in the rendered pixels: `layout_area` stacks lines by the
//! leading of the following line, and the rasteriser must not re-add a
//! baseline (a double-counted baseline halved the ink height of a two-line
//! layer and doubled its line gap).

use photocraft_text::engine_data::Value;
use photocraft_text::layer_render::{FaceStore, render_from_engine_data};
use photocraft_text::runs::{RunStyle, to_engine_data};

fn store() -> FaceStore {
    let mut store = FaceStore::new();
    let dir = std::env::var_os("HOME").map(|h| {
        std::path::PathBuf::from(h).join(".local/share/photocraft/wine/drive_c/windows/Fonts")
    });
    if let Some(dir) = dir {
        store.load_dir(&dir);
    }
    store
}

fn engine_data(runs: Vec<RunStyle>, text: &str) -> Value {
    let covered: usize = runs.iter().map(|r| r.len).sum();
    assert_eq!(covered, text.encode_utf16().count());
    Value::dict([
        (
            "EngineDict",
            Value::dict([("StyleRun", to_engine_data(&runs))]),
        ),
        (
            "ResourceDict",
            Value::dict([(
                "FontSet",
                Value::Array(vec![Value::dict([
                    ("Name", Value::Str("NotoSans-Regular".into())),
                    ("Script", Value::Int(0)),
                ])]),
            )]),
        ),
    ])
}

/// Vertical ink bands of a rendered layer, top row first.
fn bands(p: &photocraft_text::layer_render::LayerPixels) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for y in 0..p.height {
        let ink = p.rgba[(y * p.width * 4) as usize..((y + 1) * p.width * 4) as usize]
            .chunks_exact(4)
            .any(|px| px[3] > 8);
        if ink {
            match out.last_mut() {
                Some(last) if last.1 == y - 1 => last.1 = y,
                _ => out.push((y, y)),
            }
        }
    }
    out
}

#[test]
fn two_lines_step_by_the_authored_leading() {
    let text = "Hamburg\rHamburg";
    let runs = vec![RunStyle {
        len: 15,
        size: Some(48.0),
        leading: Some(100.0),
        auto_leading: Some(false),
        ..RunStyle::default()
    }];
    let ed = engine_data(runs.clone(), text);
    let px = render_from_engine_data(&store(), &ed, text, (0.0, 0.0), (0.0, 0.0)).expect("render");
    let bands = bands(&px);
    assert_eq!(bands.len(), 2, "two ink bands, got {bands:?}");
    let step = bands[1].0 as i32 - bands[0].0 as i32;
    assert!((step - 100).abs() <= 1, "line step {step} != leading 100");
    // One step plus the ink of a single line: a double-counted baseline would
    // add another 100.
    let height = px.height as i32;
    assert!(
        height < 190,
        "layer is {height} px tall, the baseline was double-counted"
    );
}

#[test]
fn explicit_leading_disables_auto_leading_in_engine_data() {
    let runs = vec![RunStyle {
        len: 2,
        size: Some(48.0),
        leading: Some(100.0),
        ..RunStyle::default()
    }];
    let value = to_engine_data(&runs);
    // Photoshop (and our own prerender) resolve a missing AutoLeading to
    // 1.2 x size, so an explicit leading has to switch it off.
    let sheet = value
        .get("RunArray")
        .and_then(Value::as_array)
        .and_then(|runs| runs.first())
        .and_then(|run| run.get("StyleSheet"))
        .and_then(|sheet| sheet.get("StyleSheetData"))
        .expect("run 0 style sheet");
    assert_eq!(
        sheet.get("AutoLeading").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(sheet.get("Leading").and_then(Value::as_f32), Some(100.0));
    // The same values must survive the round trip the renderer uses.
    let back = photocraft_text::runs::from_engine_data(&Value::dict([(
        "EngineDict",
        Value::dict([("StyleRun", value.clone())]),
    )]));
    assert_eq!(back[0].leading, Some(100.0));
    assert_eq!(back[0].auto_leading_flag(), Some(false));
}
