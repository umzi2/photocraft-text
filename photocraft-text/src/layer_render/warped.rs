//! Flat-to-warped rendering and document placement.

use crate::engine_data::Value;
use crate::error::Error;
use crate::runs;
use crate::warp::{self, FlatGeometry, FlatSurface, Warp};

use super::horizontal::{render_from_engine_data_in_rect_with_features, render_impl};
use super::paint::{WarpAnchor, WarpDraw};
use super::{FaceStore, RectTarget, RunFeatures, WarpedPixels, first_baseline_offset_of};

/// Renders a warped type layer straight into its stored document rectangle.
///
/// Photoshop stores the *flat* layout rectangles (the `TySh` `bounds`
/// descriptor and the transform translation) and bends the rendered glyphs with
/// the `warp` descriptor; the layer's pixel rectangle is the warped result. This
/// wraps the two steps so a caller cannot mix up the flat and warped geometry:
///
/// 1. the text is laid out flat and rasterised on a canvas padded around
///    `target` (the bend pushes ink outside the rectangle — the pad is the
///    first-baseline offset plus 60 px, the same margin the PSD harness uses);
/// 2. [`warp::apply`] remaps that flat raster into `target.size` at
///    `target.origin`.
///
/// Contract:
/// * `flat.bounds` is the flat layout box in **document points** — the `TySh`
///   `bounds` rectangle translated by the transform: `(tx + l, ty + t,
///   tx + r, ty + b)`. It is *not* the warped `boundingBox`. A bounds rectangle
///   with no area is rejected with [`Error::Warp`] instead of silently mapping
///   the text into a one-pixel box.
/// * `flat.baseline` is the document y of the flat first baseline (the `TySh`
///   transform translation `ty` for point text).
/// * `target` is the *stored warped layer rectangle* (`RectTarget::origin` =
///   record `left/top`, `size` = record width/height) and `target.anchor` is the
///   document position of the layout origin (the `TySh` translation for point
///   text, the frame origin plus first baseline for area text), the same anchor
///   [`super::render_from_engine_data_in_rect`] takes.
/// * `box_size`/`origin` follow the normal render contract (point text passes
///   `(0, 0)` for both; area text passes the frame size).
/// * Horizontal type only: the flat pass is the horizontal renderer.
///
/// The returned [`WarpedPixels::rect`] is `target.origin` with `target.size`,
/// and `pixels.offset` is `(0, 0)`.
#[allow(clippy::too_many_arguments)]
pub fn render_from_engine_data_warped(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    warp: &Warp,
    flat: FlatGeometry,
    target: RectTarget,
) -> Result<WarpedPixels, Error> {
    let width = target.size.0.max(1);
    let height = target.size.1.max(1);
    let rect = (target.origin.0, target.origin.1, width, height);
    let (l, t, r, b) = flat.bounds;
    if ![l, t, r, b, flat.baseline].iter().all(|v| v.is_finite()) || r - l <= 0.5 || b - t <= 0.5 {
        return Err(Error::Warp(format!(
            "flat layout box {:?} with baseline {} is empty; pass the TySh \
             `bounds` rectangle translated by the transform",
            flat.bounds, flat.baseline
        )));
    }
    if !warp.is_warped() {
        // No bend to apply: the flat layout is the layer, so render it directly
        // into the stored rectangle.
        let pixels = render_from_engine_data_in_rect_with_features(
            store,
            ed,
            text,
            paragraphs,
            features,
            box_size,
            (0.0, 0.0),
            target,
        )?;
        return Ok(WarpedPixels { pixels, rect });
    }
    let pad = (first_baseline_offset_of(store, ed).unwrap_or(0.0).abs() + 60.0).ceil();
    let pad = pad.clamp(0.0, 4096.0) as i32;
    let flat_origin = (target.origin.0 - pad, target.origin.1 - pad);
    let flat_size = (
        width.saturating_add(2 * pad as u32),
        height.saturating_add(2 * pad as u32),
    );
    let flat_target = RectTarget {
        origin: flat_origin,
        size: flat_size,
        anchor: target.anchor,
    };
    let mut pixels = render_from_engine_data_in_rect_with_features(
        store,
        ed,
        text,
        paragraphs,
        features,
        box_size,
        (0.0, 0.0),
        flat_target,
    )?;
    let surface = FlatSurface {
        origin: flat_origin,
        width: pixels.width.max(1),
        height: pixels.height.max(1),
    };
    // Photoshop warps the glyph *outlines* and rasterises them in the bent
    // space; resampling the flat bitmap instead doubles every sheared stem
    // (the flat hinted pixels carry the unbent stem pattern, which the warp
    // smears across two device columns). The flat pass above supplies the flat
    // ink bounds, which anchor the warped drawing exactly like the bitmap
    // resampler does; the bent outlines themselves are rasterised in the
    // stored rectangle. `PHOTOCRAFT_WARP_RESAMPLE=1` restores the bitmap path.
    let resample = std::env::var("PHOTOCRAFT_WARP_RESAMPLE").is_ok_and(|v| v != "0");
    if !resample {
        if let Some(placement) = warp::placement(&pixels.rgba, surface, flat, warp) {
            let min = placement.min;
            let anchor = match std::env::var("PHOTOCRAFT_WARP_ANCHOR").as_deref() {
                Ok("ink") => WarpAnchor::Ink,
                Ok("stretch") => WarpAnchor::Stretch,
                _ => WarpAnchor::Caller,
            };
            let flat_origin_f = (flat_origin.0 as f32, flat_origin.1 as f32);
            let map = move |cx: f32, cy: f32| {
                let (wx, wy) = placement.forward(flat, cx + flat_origin_f.0, cy + flat_origin_f.1);
                if anchor == WarpAnchor::Caller {
                    (wx - min.0, wy - min.1)
                } else {
                    (wx, wy)
                }
            };
            let warped_target = RectTarget {
                origin: flat_origin,
                size: (width, height),
                anchor: target.anchor,
            };
            if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "14") {
                eprintln!(
                    "warp: bounds={:?} baseline={:.2} flat=({},{}) min=({:.2},{:.2}) anchor={anchor:?} out={:?}",
                    flat.bounds, flat.baseline, flat_origin.0, flat_origin.1, min.0, min.1, rect
                );
            }
            let draw = WarpDraw { map: &map, anchor };
            let mut warped = render_impl(
                store,
                ed,
                text,
                paragraphs,
                features,
                box_size,
                (0.0, 0.0),
                Some(warped_target),
                Some(&draw),
            )?;
            // The layer record is the warped ink's bounding box; keep the
            // surface spanning it even where the outermost stroke rounds to
            // zero coverage, or the exporter crops and shifts the layer.
            warp::fill_border(
                &mut warped.rgba,
                (warped.width.max(1), warped.height.max(1)),
            );
            warped.offset = (0.0, 0.0);
            return Ok(WarpedPixels {
                pixels: warped,
                rect,
            });
        }
    }
    let surface = FlatSurface {
        origin: flat_origin,
        width: pixels.width.max(1),
        height: pixels.height.max(1),
    };
    let rgba = warp::apply(
        &pixels.rgba,
        surface,
        flat,
        warp,
        target.origin,
        (width, height),
    );
    pixels.rgba = rgba;
    pixels.width = width;
    pixels.height = height;
    pixels.offset = (0.0, 0.0);
    Ok(WarpedPixels { pixels, rect })
}
