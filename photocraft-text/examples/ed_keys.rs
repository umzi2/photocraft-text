//! Prints the key sets of a PSD's TySh `EngineData`, to see what Photoshop
//! stores for a layer's runs, paragraphs and renderer settings.

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};
use photocraft_text::engine_data::{self, Value};

fn keys(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Dict(d)) => d.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    for (i, layer) in file.layers().iter().enumerate() {
        let Some(tysh) = layer.block(b"TySh") else {
            continue;
        };
        let (engine, _) = VersionedDescriptor::parse_prefix(&tysh.data[52..])?;
        let Some(DescValue::RawData(raw)) = engine.descriptor.get("EngineData") else {
            continue;
        };
        let ed = engine_data::parse(raw)?;
        let Some(engine_dict) = ed.get("EngineDict") else {
            continue;
        };
        println!("layer {i} {}", layer.name());
        println!("  engine: {:?}", keys(Some(engine_dict)));
        println!("  renderer keys: {:?}", keys(engine_dict.get("Renderer")));
        println!("  renderer: {:?}", engine_dict.get("Renderer"));
        let para = engine_dict.get("ParagraphRun");
        println!("  para keys: {:?}", keys(para));
        println!(
            "  para default: {:?}",
            para.and_then(|p| p.get("DefaultRunData"))
        );
        if let Some(Value::Array(items)) = para.and_then(|p| p.get("RunArray")) {
            for (j, item) in items.iter().enumerate().take(2) {
                println!("  para run {j}: {item:?}");
            }
        }
        let style = engine_dict.get("StyleRun");
        println!("  style keys: {:?}", keys(style));
        if let Some(Value::Array(items)) = style.and_then(|s| s.get("RunArray")) {
            for (j, item) in items.iter().enumerate().take(4) {
                let sheet = item.get("StyleSheet").and_then(|s| s.get("StyleSheetData"));
                println!("  run {j}: {:?}", keys(sheet));
                if j == 0 {
                    println!("  run 0 data: {:?}", sheet);
                }
            }
        }
    }
    Ok(())
}
