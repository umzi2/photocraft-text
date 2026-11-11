//! Splits a text layer's single style run into two and saves the PSD.
//!
//! `cargo run --example runs_psd -- in.psd out.psd [split]`
//!
//! The second run gets a larger size, a red fill and extra tracking so the
//! result is visibly different in both layout and raster. Photoshop is the
//! oracle: open the output, save it again and see whether the runs survive.

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};
use photocraft_text::engine_data;
use photocraft_text::runs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: runs_psd <in.psd> <out.psd> [split]");
        return Ok(());
    }
    let split: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(3);
    let bytes = std::fs::read(&args[1])?;
    let mut file = PsdFile::from_bytes(&bytes)?;

    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let block = layer
        .blocks
        .iter_mut()
        .find(|b| &b.key == b"TySh")
        .ok_or("no TySh block")?;
    let data = &block.data;
    let version = u16::from_be_bytes([data[0], data[1]]);
    let transform = &data[2..2 + 48];
    let text_version = u16::from_be_bytes([data[50], data[51]]);
    let (mut text_desc, used) = VersionedDescriptor::parse_prefix(&data[52..])?;
    let warp_at = 52 + used;
    let warp_version = u16::from_be_bytes([data[warp_at], data[warp_at + 1]]);
    let (warp_desc, warp_used) = VersionedDescriptor::parse_prefix(&data[warp_at + 2..])?;
    let tail = &data[warp_at + 2 + warp_used..];

    let Some(DescValue::RawData(raw)) = text_desc.descriptor.get("EngineData").cloned() else {
        return Err("no EngineData".into());
    };
    let mut ed = engine_data::parse(&raw)?;
    let runs = runs::from_engine_data(&ed);
    if runs.is_empty() {
        return Err("no style runs".into());
    }
    let base = runs[0].clone();
    let total: usize = runs.iter().map(|r| r.len).sum();
    if split == 0 || split >= total {
        return Err(format!("split {split} out of range for {total} units").into());
    }

    // First run keeps the original sheet, second gets a new one built from it.
    let mut first = base.clone();
    first.len = split;
    let mut second = base.clone();
    second.len = total - split;
    second.size = Some(base.size.unwrap_or(12.0) * 1.5);
    second.color = Some([200, 30, 30]);
    second.tracking = Some(80);
    second.baseline_shift = Some(4.0);
    second.font_index = base.font_index;
    let new_runs = if runs.len() == 1 {
        vec![first, second]
    } else {
        let mut v = runs.clone();
        v[0] = first;
        v[1] = second;
        v
    };

    let mut style_run = runs::to_engine_data(&new_runs);
    // Keep Photoshop's own `DefaultRunData`: rebuilding it would change bytes
    // that have nothing to do with the styling we are testing.
    let original_default = ed
        .get("EngineDict")
        .and_then(|e| e.get("StyleRun"))
        .and_then(|s| s.get("DefaultRunData"))
        .cloned();
    if let (Some(default), Some(entries)) = (original_default, style_run.as_dict_mut()) {
        if let Some(slot) = entries.iter_mut().find(|(k, _)| k == "DefaultRunData") {
            slot.1 = default;
        }
    }
    if let Some(engine) = ed.get_mut("EngineDict").and_then(|e| e.get_mut("StyleRun")) {
        *engine = style_run;
    }

    let out_bytes = engine_data::write(&ed);
    for (key, value) in text_desc.descriptor.items.iter_mut() {
        if key.is("EngineData") {
            *value = DescValue::RawData(out_bytes.clone());
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

    std::fs::write(&args[2], file.to_bytes()?)?;
    println!(
        "wrote {} with {} runs (sizes {:?})",
        &args[2],
        new_runs.len(),
        new_runs.iter().map(|r| r.size).collect::<Vec<_>>()
    );
    Ok(())
}
