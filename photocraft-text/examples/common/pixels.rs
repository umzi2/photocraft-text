//! Re-rasterises a text layer's stored pixels after its text or styles change.
//!
//! A type layer keeps a prerendered copy of its glyphs; every viewer that does
//! not run the text engine (thumbnailers, composites before Photoshop
//! re-renders) shows that copy, so it has to match the text data. Photoshop
//! also compares the two when it decides whether a layer needs updating.

use std::path::PathBuf;

use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};
use photocraft_psd::{Compression, PsdFile};
use photocraft_text::engine_data;

/// Re-renders the first text layer of `file` and stores the result in it.
pub fn refresh_text_layer(file: &mut PsdFile, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let fonts = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/photocraft/wine/drive_c/windows/Fonts"));
    let mut store = photocraft_text::layer_render::FaceStore::new();
    if let Some(dir) = fonts {
        store.load_dir(&dir);
    }
    if let Some(atlas) = std::env::var_os("PHOTOCRAFT_ATLAS") {
        store.set_atlas(
            photocraft_text::atlas::Atlas::load(std::path::Path::new(&atlas))
                .map_err(|e| format!("atlas {e}"))?,
        );
    }
    let depth = file.header.depth;
    let version = file.header.version;
    // The text box lives in `Txt2`; the layer rect is only the ink bounds, and
    // using it as the box would wrap the text differently.
    let frame = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .and_then(|b| photocraft_text::txt2::frame_geometry(&b.data));
    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let block = layer
        .blocks
        .iter()
        .find(|b| &b.key == b"TySh")
        .map(|b| b.data.clone())
        .ok_or("no TySh block")?;
    let (engine, _) = VersionedDescriptor::parse_prefix(&block[52..])?;
    let Some(DescValue::RawData(raw)) = engine.descriptor.get("EngineData") else {
        return Err("no EngineData".into());
    };
    let ed = engine_data::parse(raw)?;
    // Area text keeps its frame; point text is measured from the runs because
    // the layer rect is only the ink bounds of the previous render.
    let box_size = match frame {
        Some(f) => ((f.right - f.left).max(1.0), (f.bottom - f.top).max(1.0)),
        None => (0.0, 0.0),
    };
    let pixels = photocraft_text::layer_render::render_from_engine_data(&store, &ed, text, box_size, (0.0, 0.0))?;
    // `pixels.offset` is measured from the first baseline: x from the text
    // origin, y from that baseline (negative = ink rises above it). Point text
    // is anchored at the `TySh` transform translation (the text origin, whose
    // y is the first baseline); area text at its frame's top-left plus the
    // first-baseline offset.
    if pixels.width == 0 || pixels.height == 0 {
        return Ok(());
    }
    let first_off = photocraft_text::layer_render::first_baseline_offset_of(&store, &ed).unwrap_or(0.0);
    let (anchor_x, anchor_y) = match frame {
        Some(f) => (f.left, f.top + first_off),
        None => {
            let transform = &block[2..50];
            let tx = f64::from_be_bytes(transform[32..40].try_into().unwrap_or([0; 8]));
            let ty = f64::from_be_bytes(transform[40..48].try_into().unwrap_or([0; 8]));
            (tx as f32, ty as f32)
        }
    };
    let left = anchor_x + pixels.offset.0;
    let top = anchor_y + pixels.offset.1;
    layer.rect = photocraft_psd::Rect {
        left: left.round() as i32,
        top: top.round() as i32,
        right: left.round() as i32 + pixels.width as i32,
        bottom: top.round() as i32 + pixels.height as i32,
    };
    layer.set_rgba(&pixels.rgba, pixels.width as usize, pixels.height as usize, depth, version)?;
    for ch in layer.channels.iter_mut() {
        if ch.compression.is_none() {
            ch.compression = Some(Compression::Rle);
        }
    }
    Ok(())
}

/// Rewrites the `EngineData` blob of a `TySh` block through `edit`.
///
/// The descriptor wrapper (version, transform, text version, warp, bounds) is
/// kept byte for byte; only the `EngineData` value is re-serialised, and the
/// block's even length is preserved.
pub fn with_engine_data(
    block: &[u8],
    edit: impl FnOnce(&mut engine_data::Value),
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let version = u16::from_be_bytes([block[0], block[1]]);
    let transform = &block[2..50];
    let text_version = u16::from_be_bytes([block[50], block[51]]);
    let (mut text_desc, used) = VersionedDescriptor::parse_prefix(&block[52..])?;
    let warp_at = 52 + used;
    let warp_version = u16::from_be_bytes([block[warp_at], block[warp_at + 1]]);
    let (warp_desc, warp_used) = VersionedDescriptor::parse_prefix(&block[warp_at + 2..])?;
    let tail = &block[warp_at + 2 + warp_used..];

    let Some(DescValue::RawData(raw)) = text_desc.descriptor.get("EngineData").cloned() else {
        return Err("no EngineData".into());
    };
    let mut ed = engine_data::parse(&raw)?;
    edit(&mut ed);
    let bytes = engine_data::write(&ed);
    for (key, value) in text_desc.descriptor.items.iter_mut() {
        if key.is("EngineData") {
            *value = DescValue::RawData(bytes.clone());
        }
    }

    let mut out = Vec::with_capacity(block.len());
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
    Ok(out)
}
