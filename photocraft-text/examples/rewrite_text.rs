//! Rewrites the text of the first text layer in a PSD and saves the result.
//!
//! `cargo run --example rewrite_text -- in.psd out.psd "New text"`
//!
//! The PSD's `TySh` block is rebuilt from its parts so the EngineData blob and
//! the `Txt ` string stay consistent with the run lengths.

use photocraft_psd::PsdFile;
use photocraft_psd::descriptor::{UnicodeString, Value as DescValue, VersionedDescriptor};
use photocraft_text::engine_data::{self, Value};
use photocraft_text::psd::refresh_text_layer;

#[path = "common/fonts.rs"]
mod fonts;

/// Replace the first occurrence of `open..close` with `open + value + close`.
fn replace_between(src: &str, open: &str, close: &str, value: &str) -> String {
    let Some(start) = src.find(open) else {
        return src.to_string();
    };
    let body = start + open.len();
    let Some(end_rel) = src[body..].find(close) else {
        return src.to_string();
    };
    format!("{}{}{}{}", &src[..body], value, &src[body + end_rel..], "")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: rewrite_text <in.psd> <out.psd> <text>");
        return Ok(());
    }
    let input = std::fs::read(&args[1])?;
    let mut file = PsdFile::from_bytes(&input)?;

    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let layer_rect = layer.rect;
    let block = layer
        .blocks
        .iter_mut()
        .find(|b| &b.key == b"TySh")
        .ok_or("no TySh block")?;

    let data = &block.data;
    // Version.1: u16, transform 6 x f64, text version u16, text descriptor,
    // warp version u16, warp descriptor, then the four i32 bounds.
    let version = u16::from_be_bytes([data[0], data[1]]);
    let transform = &data[2..2 + 48];
    let text_version = u16::from_be_bytes([data[50], data[51]]);
    let (mut text_desc, used) = VersionedDescriptor::parse_prefix(&data[52..])?;
    let warp_at = 52 + used;
    let warp_version = u16::from_be_bytes([data[warp_at], data[warp_at + 1]]);
    let (warp_desc, warp_used) = VersionedDescriptor::parse_prefix(&data[warp_at + 2..])?;
    let tail_at = warp_at + 2 + warp_used;
    let tail = &data[tail_at..];

    let new_text = args[3].clone();
    // Photoshop 20 writes a zero transform and zero bounds in `TySh`, so the
    // box geometry has to come from the layer rectangle: it starts at the text
    // box's top-left corner.
    let _ = tail;
    let _ = layer_rect;

    // EngineData: replace the editor text and keep the run arrays in sync.
    if let Some(DescValue::RawData(raw)) = text_desc.descriptor.get("EngineData").cloned() {
        let mut ed = engine_data::parse(&raw)?;
        let spec = std::env::args().nth(4);
        let size = spec.as_deref().and_then(|v| v.parse::<f64>().ok());
        if let Some(px) = size {
            if let Some(items) = ed
                .get_mut("EngineDict")
                .and_then(|e| e.get_mut("StyleRun"))
                .and_then(|r| r.get_mut("RunArray"))
                .and_then(|a| a.as_array_mut())
            {
                for item in items {
                    if let Some(data) = item
                        .get_mut("StyleSheet")
                        .and_then(|s| s.get_mut("StyleSheetData"))
                    {
                        if let Some((_, v)) = data
                            .as_dict_mut()
                            .and_then(|d| d.iter_mut().find(|(k, _)| k == "FontSize"))
                        {
                            *v = Value::Float(px);
                        }
                    }
                }
            }
        }
        let bytes = engine_data::write(&ed);
        for (key, value) in text_desc.descriptor.items.iter_mut() {
            if key.is("EngineData") {
                *value = DescValue::RawData(bytes.clone());
            } else if key.is("Txt ") {
                *value = DescValue::Text(UnicodeString::new(&new_text));
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
    // Layer blocks are written with an even length; keep it even.
    if out.len() % 2 != 0 {
        out.push(0);
    }
    block.data = out;
    // Length changed: drop any explicit padding so it is recomputed.
    block.padding = None;

    // Photoshop reads the layer's contents from `Txt2`, so it has to change too.
    // Any length works: `replace_text` rebuilds the run tables and empties the
    // composed-layout cache so Photoshop lays the text out again.
    for block in file.global_blocks.iter_mut().filter(|b| &b.key == b"Txt2") {
        block.data = photocraft_text::txt2::replace_text(&block.data, &new_text)?;
        block.padding = None;
    }

    // XMP carries a copy of the layer text; Photoshop compares it with `TySh`
    // and refuses to rebuild the layer when the two disagree.
    for res in file.resources.iter_mut().filter(|r| r.id == 1060) {
        if let Ok(xmp) = std::str::from_utf8(&res.data) {
            let escaped: String = new_text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let patched = replace_between(
                xmp,
                "<photoshop:LayerText>",
                "</photoshop:LayerText>",
                &escaped,
            );
            res.data = patched.into_bytes();
        }
    }

    let store = fonts::load()?;
    refresh_text_layer(&mut file, &new_text, &store)?;

    std::fs::write(&args[2], file.to_bytes()?)?;
    println!(
        "wrote {} ({} bytes)",
        args[2],
        std::fs::metadata(&args[2])?.len()
    );
    Ok(())
}
