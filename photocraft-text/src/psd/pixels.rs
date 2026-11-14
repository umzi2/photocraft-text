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
    // A vertical layer (`Rendered/Shapes/WritingDirection = 2`, the marker
    // Photoshop itself writes) must go through the vertical renderer: the
    // horizontal entry lays the same runs out left-to-right, which gave every
    // authored vertical layer a horizontal raster and ink bounds (0..712 wide
    // against Photoshop's 24x420 tall). Both entries share the same
    // `LayerPixels::offset` contract (top-left of the ink relative to the text
    // origin), so the placement below stays the same.
    let vertical = ed
        .get("EngineDict")
        .and_then(|e| e.get("Rendered"))
        .and_then(|r| r.get("Shapes"))
        .and_then(|s| s.get("WritingDirection"))
        .and_then(|v| v.as_i64())
        == Some(2);
    let pixels = if vertical {
        match frame {
            Some(_) => layer_render::render_vertical_from_engine_data_in_box(
                store,
                &ed,
                text,
                (0.0, 0.0),
                box_size,
            )?,
            None => layer_render::render_vertical_from_engine_data(store, &ed, text, (0.0, 0.0))?,
        }
    } else {
        layer_render::render_from_engine_data(store, &ed, text, box_size, (0.0, 0.0))?
    };
    // `pixels.offset` is measured from the first baseline: x from the text
    // origin, y from that baseline (negative = ink rises above it). Point text
    // is anchored at the `TySh` transform translation (the text origin, whose
    // y is the first baseline); area text at its frame's top-left plus the
    // first-baseline offset.
    if pixels.width == 0 || pixels.height == 0 {
        return Ok(());
    }
    let transform = &block[2..50];
    let tx = f64::from_be_bytes(
        transform[32..40]
            .try_into()
            .expect("validated TySh transform"),
    ) as f32;
    let ty = f64::from_be_bytes(
        transform[40..48]
            .try_into()
            .expect("validated TySh transform"),
    ) as f32;
    // Vertical text is anchored at its text origin (point) or frame top-left
    // (box) without the horizontal first-baseline offset: the vertical
    // renderer places the first em cell itself, and adding the baseline put
    // every vertical box a baseline too low (measured on the fork's
    // `render_re`, same contract).
    // Area text's frame is stored in text space, so the `TySh` translation
    // (`tx`, `ty`) maps it to the layer: without it every authored area layer
    // landed at the frame's text-space origin (measured: `p_plain` stored
    // (0,-2,418,162) while Photoshop's recompose is (20,80,438,243) = the same
    // rect plus the (20,80) transform). Point text is anchored at its text
    // origin, which already is the transform translation.
    let (anchor_x, anchor_y) = if vertical {
        match frame {
            Some(f) => (tx + f.left, ty + f.top),
            None => (tx, ty),
        }
    } else {
        let first_off = layer_render::first_baseline_offset_of(store, &ed).unwrap_or(0.0);
        match frame {
            Some(f) => (tx + f.left, ty + f.top + first_off),
            None => (tx, ty),
        }
    };
    let left = anchor_x + pixels.offset.0;
    let top = anchor_y + pixels.offset.1;
    // Photoshop sizes a **vertical point** layer's stored rectangle to the
    // column's em-cell extent (measured: "abc def ghi" at 24 pt -> nine cells
    // = 216, stored height 217), while `LayerPixels` is ink-tight (205). The
    // rectangle keeps the ink top and extends to the last cell, with the extra
    // rows transparent; horizontal and box text keep the ink-tight crop.
    let width = pixels.width;
    let mut height = pixels.height;
    let mut rgba = pixels.rgba.clone();
    if vertical && frame.is_none() {
        let layout =
            layer_render::layout_vertical_from_engine_data(store, &ed, text, (0.0, 0.0))?;
        let cells = layout.box_size.1.round();
        let wanted = (cells as i64 + 1).max(0) as u32;
        if wanted > height {
            rgba.resize(width as usize * wanted as usize * 4, 0);
            height = wanted;
        }
    }
    layer.rect = photocraft_psd::Rect {
        left: left.round() as i32,
        top: top.round() as i32,
        right: left.round() as i32 + width as i32,
        bottom: top.round() as i32 + height as i32,
    };
    layer.set_rgba(
        &rgba,
        width as usize,
        height as usize,
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
