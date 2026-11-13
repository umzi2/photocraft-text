//! Upright vertical point-text composition.

use crate::area::ParagraphStyle;
use crate::engine_data::Value;
use crate::error::Error;
use crate::font::Face;
use crate::layout::TextStyle;
use crate::runs;

use super::horizontal::text_style_of;
use super::paint::paint;
use super::{FaceStore, LayerPixels, LayerTextStyle, RectTarget};

/// Cap-height ratio (`OS/2.sCapHeight` over the em square), 0.72 fallback.
fn cap_height_ratio(face: &Face<'_>) -> f32 {
    face.os2
        .and_then(|o| o.s_cap_height)
        .map(|c| f32::from(c) / f32::from(face.head.units_per_em))
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(0.72)
}

fn vertical_impl(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    target: Option<RectTarget>,
) -> Result<LayerPixels, Error> {
    let mut style_runs = runs::from_engine_data(ed);
    let aa = runs::antialias(ed);
    if style_runs.is_empty() {
        return Err(Error::EngineData("no style runs"));
    }
    // `TySh` stores the paragraph terminator as a NUL and drops it from the
    // text, while `EngineData`'s run lengths still count it; trim the trailing
    // units so shaping covers exactly the string we draw.
    let text_units = text.encode_utf16().count();
    let mut excess = style_runs
        .iter()
        .map(|r| r.len)
        .sum::<usize>()
        .saturating_sub(text_units);
    while excess > 0 {
        let Some(last) = style_runs.last_mut() else {
            break;
        };
        let take = last.len.min(excess);
        last.len -= take;
        excess -= take;
        if last.len == 0 {
            style_runs.pop();
        }
    }
    if style_runs.is_empty() {
        return Err(Error::EngineData("no style runs"));
    }
    let mut wanted: Vec<String> = style_runs.iter().filter_map(|r| r.font.clone()).collect();
    wanted.sort();
    wanted.dedup();
    let faces = store.font_set(&wanted)?;
    let base = TextStyle {
        size: style_runs[0].size.unwrap_or(12.0),
        ..Default::default()
    };
    let style = text_style_of(&style_runs[0], &base);
    let primary = style_runs[0]
        .font
        .as_deref()
        .and_then(|n| faces.get(n))
        .unwrap_or_else(|| faces.default_face());
    let size = style.size.max(1.0);
    // Measured on `10_all_features.psd` ("11 vertical text", NotoSans 24 pt):
    // first baseline 20.57 pt below the top of the em column = (size + cap)/2;
    // each following upright glyph advances exactly one em. The glyph itself is
    // centred in the column: pen x = origin.x - advance/2 (NotoSans half
    // advances V 7.2, e 6.768, r 4.956, t 4.332, i 3.096, c 5.76 ...).
    let first_off = (size + cap_height_ratio(primary) * size) * 0.5;
    let shaped = runs::shape_runs_attrs(&faces, text, &style_runs, &style);
    let mut glyphs = Vec::new();
    let mut attrs = Vec::new();
    let mut placed = 0usize;
    for (mut g, a) in shaped {
        if matches!(g.ch, '\r' | '\n' | '\0') {
            continue;
        }
        g.x = -g.advance * 0.5;
        g.y = -(first_off + placed as f32 * size);
        glyphs.push(g);
        attrs.push(a);
        placed += 1;
    }
    let widest = glyphs.iter().map(|g| g.advance).fold(size, f32::max);
    let total = (placed as f32 * size).max(size);
    let spec = LayerTextStyle {
        style,
        paragraph: ParagraphStyle::default(),
        box_size: (widest, total),
        origin,
        color: style_runs[0].color.unwrap_or([0, 0, 0]),
    };
    let line = crate::area::Line {
        glyphs,
        baseline: 0.0,
        width: widest,
        attrs,
    };
    paint(
        &faces,
        vec![line],
        &style_runs,
        &spec,
        aa,
        store.atlases(),
        target,
        None,
    )
}

/// Lays out a vertical (`Ornt = Vrtc`) point-text layer from its `EngineData`.
///
/// The returned surface is ink-cropped with `origin` being the top of the em
/// column (`TySh` translation), the same placement contract as
/// [`super::render_from_engine_data`].
pub fn render_vertical_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    vertical_impl(store, ed, text, origin, None)
}

/// As [`render_vertical_from_engine_data`] but rasterised straight into
/// `target`, with `target.anchor` = the `TySh` translation.
pub fn render_vertical_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    vertical_impl(store, ed, text, origin, Some(target))
}
