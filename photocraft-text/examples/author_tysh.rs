//! Writes per-run styles into `TySh/EngineData` only, leaving `Txt2` alone.
//!
//! `cargo run --example author_tysh -- in.psd out.psd runs.json`

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::Value as DescValue;
use photocraft_psd::descriptor::VersionedDescriptor;
use photocraft_text::engine_data::{self, Value};
use photocraft_text::runs::{self, RunStyle};

#[derive(serde::Deserialize)]
struct Run {
    len: usize,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    tracking: Option<i16>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let mut file = PsdFile::from_bytes(&std::fs::read(&a[1])?)?;
    let spec: Vec<Run> = serde_json::from_str(&std::fs::read_to_string(&a[3])?)?;
    let runs_in: Vec<RunStyle> = spec
        .into_iter()
        .map(|r| RunStyle {
            len: r.len,
            size: r.size,
            tracking: r.tracking,
            ..Default::default()
        })
        .collect();

    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let block = layer
        .blocks
        .iter_mut()
        .find(|b| &b.key == b"TySh")
        .ok_or("no TySh")?;
    let data = &block.data;
    let version = u16::from_be_bytes([data[0], data[1]]);
    let transform = &data[2..50];
    let text_version = u16::from_be_bytes([data[50], data[51]]);
    let (mut text_desc, used) = VersionedDescriptor::parse_prefix(&data[52..])?;
    let warp_at = 52 + used;
    let warp_version = u16::from_be_bytes([data[warp_at], data[warp_at + 1]]);
    let (warp_desc, warp_used) = VersionedDescriptor::parse_prefix(&data[warp_at + 2..])?;
    let tail = &data[warp_at + 2 + warp_used..];

    if let Some(DescValue::RawData(raw)) = text_desc.descriptor.get("EngineData").cloned() {
        let mut ed = engine_data::parse(&raw)?;
        let new_runs = runs::to_engine_data(&runs_in);
        let style_run = ed
            .get_mut("EngineDict")
            .and_then(|e| e.get_mut("StyleRun"))
            .ok_or("no StyleRun")?;
        if let Value::Dict(d) = style_run {
            d.retain(|(k, _)| k != "RunArray" && k != "RunLengthArray");
            if let Value::Dict(entries) = new_runs {
                d.extend(entries);
            }
        }
        let bytes = engine_data::write(&ed);
        for (key, value) in text_desc.descriptor.items.iter_mut() {
            if key.is("EngineData") {
                *value = DescValue::RawData(bytes.clone());
            }
        }
    }

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(transform);
    out.extend_from_slice(&text_version.to_be_bytes());
    out.extend_from_slice(&text_desc.to_bytes());
    out.extend_from_slice(&warp_version.to_be_bytes());
    out.extend_from_slice(&warp_desc.to_bytes());
    out.extend_from_slice(tail);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    block.data = out;
    block.padding = None;
    std::fs::write(&a[2], file.to_bytes()?)?;
    println!("wrote {}", a[2]);
    Ok(())
}
