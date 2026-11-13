//! Re-rasterises a text layer's stored pixels after its text or styles change.
//!
//! A type layer keeps a prerendered copy of its glyphs; every viewer that does
//! not run the text engine (thumbnailers, composites before Photoshop
//! re-renders) shows that copy, so it has to match the text data. Photoshop
//! also compares the two when it decides whether a layer needs updating.

use photocraft_psd::descriptor::Value as DescValue;
use photocraft_psd::{Compression, LayerRecord, PsdFile, Version};

use crate::engine_data;
use crate::layer_render::{self, FaceStore};
use crate::txt2::FrameGeometry;

use super::{Error, engine_data::text_descriptor};

/// Re-renders the first text layer of `file` and stores the result in it.
/// Errors leave the file unchanged; only the edited layer is staged.
pub fn refresh_text_layer(file: &mut PsdFile, text: &str, store: &FaceStore) -> Result<(), Error> {
    let depth = file.header.depth;
    let version = file.header.version;
    // The text box lives in `Txt2`; the layer rect is only the ink bounds, and
    // using it as the box would wrap the text differently.
    let frame = file
        .global_blocks
        .iter()
        .find(|b| &b.key == b"Txt2")
        .and_then(|b| crate::txt2::frame_geometry(&b.data));
    let layer_index = file
        .layers()
        .iter()
        .position(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or(Error::MissingTextLayer)?;
    let mut layer = file.layers()[layer_index].clone();
    refresh_layer(&mut layer, text, store, frame, depth, version)?;
    file.layers_mut()[layer_index] = layer;
    Ok(())
}

pub(super) fn refresh_layer(
    layer: &mut LayerRecord,
    text: &str,
    store: &FaceStore,
    frame: Option<FrameGeometry>,
    depth: u16,
    version: Version,
) -> Result<(), Error> {
    let block = layer
        .blocks
        .iter()
        .find(|b| &b.key == b"TySh")
        .map(|b| b.data.as_slice())
        .ok_or(Error::MissingTySh)?;
    let (engine, _) = text_descriptor(block)?;
    let Some(DescValue::RawData(raw)) = engine.descriptor.get("EngineData") else {
        return Err(Error::MissingEngineData);
    };
    let ed = engine_data::parse(raw)?;
    // Area text keeps its frame; point text is measured from the runs because
    // the layer rect is only the ink bounds of the previous render.
    let box_size = match frame {
        Some(f) => ((f.right - f.left).max(1.0), (f.bottom - f.top).max(1.0)),
        None => (0.0, 0.0),
    };
    let pixels = layer_render::render_from_engine_data(store, &ed, text, box_size, (0.0, 0.0))?;
    // `pixels.offset` is measured from the first baseline: x from the text
    // origin, y from that baseline (negative = ink rises above it). Point text
    // is anchored at the `TySh` transform translation (the text origin, whose
    // y is the first baseline); area text at its frame's top-left plus the
    // first-baseline offset.
    if pixels.width == 0 || pixels.height == 0 {
        return Ok(());
    }
    let first_off = layer_render::first_baseline_offset_of(store, &ed).unwrap_or(0.0);
    let (anchor_x, anchor_y) = match frame {
        Some(f) => (f.left, f.top + first_off),
        None => {
            let transform = &block[2..50];
            let tx = f64::from_be_bytes(
                transform[32..40]
                    .try_into()
                    .expect("validated TySh transform"),
            );
            let ty = f64::from_be_bytes(
                transform[40..48]
                    .try_into()
                    .expect("validated TySh transform"),
            );
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
    layer.set_rgba(
        &pixels.rgba,
        pixels.width as usize,
        pixels.height as usize,
        depth,
        version,
    )?;
    for ch in layer.channels.iter_mut() {
        if ch.compression.is_none() {
            ch.compression = Some(Compression::Rle);
        }
    }
    Ok(())
}
