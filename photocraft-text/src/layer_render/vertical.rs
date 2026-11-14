//! Upright vertical point-text composition.

use crate::area::{Justification, ParagraphStyle};
use crate::engine_data::Value;
use crate::error::Error;
use crate::font::Face;
use crate::layout::TextStyle;
use crate::runs;
use crate::runs::{FontSet, RunStyle};

use super::horizontal::text_style_of;
use super::paint::paint;
use super::{FaceStore, LayerPixels, LayerTextStyle, RectTarget};

/// Signature of the font metrics a vertical anchor ratio is keyed on:
/// `(units per em, hhea ascender, hhea descender, OS/2 cap height, OS/2 x
/// height)`. Only cheap-to-read fields, so a caller can classify a face without
/// shaping a glyph.
type AnchorSignature = (u16, i16, i16, Option<i16>, Option<i16>);

/// Measured [`vertical_b0_ratio`] per font, from Photoshop's own composed cache.
///
/// Each entry is `((upm, hhea asc/desc, OS/2 cap/x), b0 / size)`. The ratio is
/// a pure font constant: the cache's first `/99 /L` `/10` divided by the run's
/// font size stays identical to ~2e-7 across every size and frame measured, and
/// it does not depend on the text either (Myriad at 48 pt reports 17.90369 for
/// `фывфыв`, `Aamburgefonstiv` and `gamburgefonstiv` alike).
///
/// Measurements (`out/verify/b0/*.psd`, Photoshop CC 2019, recomposed by the
/// application itself: open, nudge tracking by ±0.001, save):
///
/// | font | sizes (pt) | b0 (pt) | ratio |
/// |------|-----------|---------|-------|
/// | MyriadPro-Regular | 24 / 48 / 72 | 8.95184 / 17.90369 / 26.85553 | 0.3729935 |
/// | TimesNewRomanPSMT | 24 / 30 / 48 / 72 | 8.72388 / 10.90485 / 17.44775 / 26.17163 | 0.3634949 |
/// | ArialMT | 48 | 17.18372 | 0.3579942 |
/// | NotoSans-Regular, NotoSerif-Regular | 48 | 19.34399 | 0.4029998 |
/// | CCGrimNGritty-Regular | 48 | 18.76758 | 0.3909912 |
/// | CCMeanwhile | 48 | 18.80457 | 0.3917619 |
/// | CCSpellcaster | 48 | 21.16809 | 0.4410019 |
///
/// No font-table form reproduces these ratios. Halves of the ascent
/// (`hhea.ascender/2`, `sTypoAscender/2`, `usWinAscent/2`), `(1 - cap)/2`,
/// `(asc - cap)/2`, `(asc + desc)/2`, ink ratios (`'H'`, `'d'`, `'f'`, `'b'`,
/// `'x'` tops and bottoms, the whole-font ink bbox) and every linear
/// combination of one, two or three such metrics were fitted across the eight
/// fonts: the best single-metric fit misses by 1.7% of an em and the best
/// two-metric fit by 0.7% (`0.24` pt at 48 pt), so the table is the measurement,
/// not a derivation. Two different fonts that share these metrics share the
/// ratio (Noto Sans / Noto Serif agree to 1e-7), which is why the key is the
/// metric signature and not the name.
const VERTICAL_B0_RATIOS: &[(AnchorSignature, f32)] = &[
    // MyriadPro-Regular (PDFL), upm 1000.
    ((1000, 750, -250, Some(674), Some(484)), 0.3729935),
    // TimesNewRomanPSMT, upm 2048, OS/2 v1 (no cap/x height).
    ((2048, 1825, -443, None, None), 0.3634949),
    // ArialMT, upm 2048, OS/2 v1.
    ((2048, 1854, -434, None, None), 0.3579942),
    // NotoSans-Regular and NotoSerif-Regular share hhea and OS/2 metrics.
    ((1000, 1069, -293, Some(714), Some(536)), 0.4029998),
    // CCGrimNGritty-Regular.
    ((1000, 1100, -300, Some(780), Some(780)), 0.3909912),
    // CCMeanwhile.
    ((1000, 983, -143, Some(849), Some(836)), 0.3917619),
    // CCSpellcaster.
    ((1000, 800, -200, Some(833), Some(652)), 0.4410019),
];

/// `b0 / size` for a metric signature, when the table measured that face.
fn measured_ratio(signature: AnchorSignature) -> Option<f32> {
    VERTICAL_B0_RATIOS
        .iter()
        .find(|(known, _)| *known == signature)
        .map(|(_, ratio)| *ratio)
}

fn anchor_signature(face: &Face<'_>) -> AnchorSignature {
    (
        face.head.units_per_em,
        face.hhea.ascender,
        face.hhea.descender,
        face.os2.and_then(|o2| o2.s_cap_height),
        face.os2.and_then(|o2| o2.s_x_height),
    )
}

/// The first vertical column's across anchor: Photoshop puts the column's em
/// cell centre `b0 = size * vertical_b0_ratio` inside the frame's **right** edge, not
/// half an em (`width - size/2`, which put every column 6 pt too far right at
/// 48 pt and was the whole `Δx` residual in the app-vs-Photoshop comparison).
///
/// Measured on the composed cache: 454 pt box, 48 pt Myriad gives `/10` =
/// 17.90369, and the column's glyph box `/8` = `[412.09631, …]` has its centre
/// at 436.09631 = 454 - 17.90369. Size, frame and text do not move the ratio;
/// see the private `VERTICAL_B0_RATIOS` for the per-font numbers and why they
/// are a table rather than a formula.
pub fn vertical_b0_ratio(face: &Face<'_>) -> f32 {
    measured_ratio(anchor_signature(face)).unwrap_or_else(|| {
        // A face outside the table falls back to half its first-baseline
        // ascent (`min(ink H, ink d)` over the em, the same ascent the
        // horizontal rule uses): over the eight measured fonts that form is
        // within 0.046 em (2.2 pt at 48 pt) where a flat default would miss by
        // 0.142 em. Prefer a cache `/10` when the layer has one.
        0.5 * crate::area::first_baseline_ratio(face)
    })
}

/// Where a column's em cell starts along the frame, from the paragraph's
/// justification (`Left`/top, `Center`, `Right`/bottom).
///
/// Photoshop measures this from its composed caches with zero residual:
/// `Left` columns have `/8` y0 = 0.0 (48 pt Myriad box at 24/48/72 pt); the
/// `Center` u12 layers give exactly `(height - extent) / 2` at 24/30/72 pt over
/// frames 200/179/334.63 pt (`10.0` and `55.0` for a 200 pt frame with 180/90 pt
/// columns, `14.5` / `74.5` / `44.5` for 179 pt with 150/30/90 pt, `77.316` for
/// 334.63 pt with 180 pt). `Justify*` behaves like `Left`: a vertical column is
/// one line, and the last line of a justified paragraph is not stretched.
fn along_justification_offset(justification: Justification, height: f32, extent: f32) -> f32 {
    let free = (height - extent).max(0.0);
    match justification {
        Justification::Center => free * 0.5,
        Justification::Right => free,
        _ => 0.0,
    }
}

/// Cap-height ratio (`OS/2.sCapHeight` over the em square), 0.72 fallback.
fn cap_height_ratio(face: &Face<'_>) -> f32 {
    face.os2
        .and_then(|o| o.s_cap_height)
        .map(|c| f32::from(c) / f32::from(face.head.units_per_em))
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(0.72)
}

/// Everything the vertical layout and the rasteriser share for one layer.
pub(super) struct PreparedVertical<'a> {
    pub(super) faces: FontSet<'a>,
    pub(super) style_runs: Vec<RunStyle>,
    pub(super) spec: LayerTextStyle,
    /// One column per hard-break line (and, when the box height runs out, per
    /// soft wrap). Columns advance in `-x` in this layout space, which the
    /// editor maps to document left (vertical type runs right to left).
    pub(super) lines: Vec<crate::area::Line>,
    /// Distance between column origins: the column's paragraph leading, not the
    /// em (see [`vertical_prepare`]).
    pub(super) pitch: f32,
    /// The first column's centre in layout space (`0.0` for point text), shared
    /// with [`super::layout::layout_vertical_of`] so empty columns sit on the
    /// same ladder as glyph columns.
    pub(super) first_centre: f32,
    pub(super) aa: crate::raster::AntiAlias,
}

fn vertical_impl(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    target: Option<RectTarget>,
    box_size: Option<(f32, f32)>,
) -> Result<LayerPixels, Error> {
    let prepared = vertical_prepare(store, ed, text, origin, box_size)?;
    paint(
        &prepared.faces,
        prepared.lines.clone(),
        &prepared.style_runs,
        &prepared.spec,
        prepared.aa,
        store.atlases(),
        target,
        None,
    )
}

fn vertical_prepare<'a>(
    store: &'a FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    box_size: Option<(f32, f32)>,
) -> Result<PreparedVertical<'a>, Error> {
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
    // Column advance = the leading (see the pitch note below); the ratio is
    // per paragraph, so resolve it from `ParagraphRun` rather than the layer
    // style (which only carries the 1.2 default).
    let paragraphs = runs::paragraphs_from_engine_data(ed);
    let explicit_leading = style
        .leading
        .or_else(|| style_runs.first().and_then(|r| r.leading))
        .filter(|l| l.is_finite() && *l > 0.0);
    let ratio_of = |paragraph: usize| {
        paragraphs
            .get(paragraph)
            .map(|p| p.auto_leading)
            .filter(|r| r.is_finite() && *r > 0.0)
            .unwrap_or(style.auto_leading)
    };
    // Paragraph separators are the column breaks: column `k` holds the glyphs
    // whose source bytes sit after `k` separators, so a hard break starts the
    // next column on the left (in document space). A column with no glyphs (an
    // empty paragraph, or two breaks in a row) still keeps its place so the
    // caret has a column to sit in.
    let separators: Vec<(usize, usize)> = text
        .char_indices()
        .filter(|(_, ch)| matches!(ch, '\r' | '\n' | '\u{2029}'))
        .map(|(at, ch)| (at, at + ch.len_utf8()))
        .collect();
    let column_of = |byte: usize| separators.iter().filter(|(start, _)| *start < byte).count();
    let mut groups: Vec<(
        Vec<crate::layout::GlyphPos>,
        Vec<runs::GlyphAttrs>,
        usize,
        usize,
    )> = (0..=separators.len())
        .map(|_| (Vec::new(), Vec::new(), usize::MAX, 0usize))
        .collect();
    for (glyph, attrs) in shaped {
        if matches!(glyph.ch, '\r' | '\n' | '\u{2029}' | '\0') {
            continue;
        }
        let column = column_of(attrs.source.0).min(separators.len());
        let slot = &mut groups[column];
        slot.2 = slot.2.min(attrs.source.0);
        slot.3 = slot.3.max(attrs.source.1);
        slot.0.push(glyph);
        slot.1.push(attrs);
    }
    // An empty column owns its separator bytes: that is where its caret sits.
    for (index, group) in groups.iter_mut().enumerate() {
        if !group.0.is_empty() {
            continue;
        }
        if let Some((start, end)) = separators.get(index) {
            group.2 = *start;
            group.3 = *end;
        } else {
            group.2 = text.len();
            group.3 = text.len();
        }
    }
    // A column that runs past the box height starts a new one: soft wrap, with
    // the same column advance as a hard break.
    let per_column = box_size
        .map(|(_, height)| height)
        .filter(|height| *height > 0.0)
        .map(|height| ((height / size).floor() as usize).max(1));
    let mut columns: Vec<(
        Vec<crate::layout::GlyphPos>,
        Vec<runs::GlyphAttrs>,
        usize,
        usize,
    )> = Vec::new();
    for (glyphs, attrs, start, end) in groups {
        let chunk = per_column.unwrap_or(usize::MAX);
        if glyphs.is_empty() || chunk >= glyphs.len() {
            columns.push((glyphs, attrs, start, end));
            continue;
        }
        for (glyphs, attrs) in glyphs.chunks(chunk).zip(attrs.chunks(chunk)) {
            let chunk_start = attrs.first().map_or(start, |a| a.source.0);
            let chunk_end = attrs.last().map_or(end, |a| a.source.1);
            columns.push((glyphs.to_vec(), attrs.to_vec(), chunk_start, chunk_end));
        }
    }
    // Column pitch follows the *leading* of the column's own paragraph, not
    // the em: Photoshop advances its vertical columns by the paragraph line
    // height (measured on the user's `hotfix_u12.psd` rasters: 36 px at 30 pt
    // with `AutoLeading 1.2`, 150 px at 5.0, 3 px at 0.1), while the glyph
    // advance *down* a column stays the em. An explicit `Leading` wins;
    // otherwise the paragraph's `AutoLeading` ratio applies.
    let pitch_of = |paragraph: usize| {
        explicit_leading
            .unwrap_or(size * ratio_of(paragraph))
            .max(f32::EPSILON)
    };
    // A vertical paragraph box anchors its first column at the box's **right**
    // edge (columns advance right to left), not at the left edge the layer
    // transform names: anchoring at the left edge drew the whole block one box
    // width outside the frame (user report, `user_vertical_bad.psd`: a
    // 453.69-wide box drew its five 30 px columns at x = -120..0 of the box).
    // Point text has no box width and keeps the `TySh` origin as column 0.
    let first_paragraph = columns.first().map_or(0, |c| column_of(c.2));
    let first_pitch = pitch_of(first_paragraph);
    // The column *width* is one em; only the step uses the leading. Photoshop
    // anchors the first column's cell `b0 = size * b0_ratio` inside the box's
    // right edge (measured, font-specific; see `vertical_b0_ratio`), which lets
    // overhang the edge by `(1/2 - b0_ratio) * size`. Anchoring it half an em
    // inside put every column too far right.
    let b0 = size * vertical_b0_ratio(primary);
    let first_centre = box_size.map_or(0.0, |(width, _)| (width - b0).max(0.0));
    // Along the column Photoshop puts each column's **em cell** at the
    // justification offset inside the frame's along extent, not at the frame
    // top: `Left`/top -> 0, `Center` -> (height - cell extent) / 2, `Right`/
    // bottom -> height - extent. Measured from the composed caches, zero
    // residual: a `Left` 48 pt Myriad box has every column's `/8` y0 = 0.0 at
    // 24/48/72 pt, while the `Center` u12 layers give exactly
    // `(h - extent)/2` at 24/30/72 pt over frames 200/179/334.63 (e.g. 30 pt:
    // 10.0, 55.0 for a 200 pt frame with 180/90 pt columns; 14.5, 74.5, 44.5
    // for 179 pt with 150/30/90 pt; 77.316 for 334.63 pt with 180 pt).
    // `first_off` is our pen's offset *inside* that cell (verified: a `Left`
    // box's ink bbox from this renderer equals Photoshop's stored layer
    // rectangle exactly, delta 0 in y, at 24/48/72 pt Myriad).
    let along_offset = |paragraph: usize, extent: f32| -> f32 {
        let height = box_size.map_or(0.0, |(_, height)| height);
        let justification = paragraphs
            .get(paragraph)
            .map(|p| p.justification)
            .unwrap_or(Justification::Left);
        along_justification_offset(justification, height, extent)
    };
    let mut lines = Vec::new();
    let mut widest = size;
    let mut tallest = 0.0f32;
    let mut column_x = first_centre;
    let mut placed_any = false;
    for (_index, (column_glyphs, attrs, range_start, range_end)) in columns.into_iter().enumerate()
    {
        let paragraph = column_of(range_start);
        let pitch = pitch_of(paragraph);
        if placed_any {
            column_x -= pitch;
        }
        // Photoshop **stops composing** a boxed vertical line at the first column
        // whose em cell lies entirely left of the frame (`cell centre + size/2 <
        // 0`); it never skips a column and keeps going. Measured against the
        // composed caches, zero residual: the u12 30 pt layers (frame 135 pt)
        // keep 4 of 6 columns at a 36 pt pitch (the 5th cell `[-34.905,-4.905]`
        // stops it) and 1 of 3 at a 150 pt pitch (2nd cell right edge
        // `-10.905`), while their 144 pt-wide / pitch-3 sibling keeps all 6 and
        // `box_v` (frame 454 pt, pitch 57.6 pt) keeps all 5 including the first
        // cell that overhangs the right edge by `(1/2 - b0_ratio) * size`. The
        // 8-column discriminator does not contradict this: the frame it carries
        // is 435 pt wide, not the 135 pt `box` the layer was authored with (the
        // writer stored the box's absolute right/bottom: `Txt2` frame x 0..435,
        // y 0..379, the same `/F` Photoshop rewrites on recompose,
        // `out/verify/along/disc_recomp.psd`), and all eight of its 36 pt-pitch
        // cells intersect that 435 pt frame.
        //
        // Only a frame can stop a column: point text (no box) has no across
        // limit. The first column is always composed, so this can never empty
        // a layer.
        if box_size.is_some() && placed_any && column_x + size * 0.5 < 0.0 {
            break;
        }
        placed_any = true;
        // The column's along start: its cell's justification offset inside the
        // frame, then our pen inside the cell.
        let extent = column_glyphs.len() as f32 * size;
        let along_start = first_off + along_offset(paragraph, extent);
        let mut glyphs = Vec::with_capacity(column_glyphs.len());
        for (placed, mut glyph) in column_glyphs.into_iter().enumerate() {
            glyph.x = column_x - glyph.advance * 0.5;
            glyph.y = -(along_start + placed as f32 * size);
            glyphs.push(glyph);
        }
        let width = glyphs.iter().map(|g| g.advance).fold(size, f32::max);
        let along = (glyphs.len() as f32 * size).max(0.0);
        // Columns run leftwards, so the widest cross extent is the leftmost
        // column's far edge (|column_x| + its width), not a running maximum of
        // signed positions.
        widest = widest.max(-column_x + width);
        tallest = tallest.max(along);
        let range_end = if glyphs.is_empty() {
            range_end
        } else {
            range_end.max(range_start)
        };
        lines.push(crate::area::Line {
            glyphs,
            baseline: 0.0,
            width,
            attrs,
            range_start: range_start.min(text.len()),
            range_end: range_end.min(text.len()),
            paragraph: paragraph.min(paragraphs.len().saturating_sub(1)),
        });
    }
    let spec = LayerTextStyle {
        style,
        paragraph: ParagraphStyle::default(),
        box_size: box_size.unwrap_or((widest, tallest.max(size))),
        origin,
        color: style_runs[0].color.unwrap_or([0, 0, 0]),
    };
    Ok(PreparedVertical {
        faces,
        style_runs,
        spec,
        lines,
        pitch: first_pitch,
        first_centre,
        aa,
    })
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
    vertical_impl(store, ed, text, origin, None, None)
}

/// As [`render_vertical_from_engine_data`] but rasterised straight into
/// `target`, with `target.anchor` = the `TySh` translation.
pub fn render_vertical_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    box_size: Option<(f32, f32)>,
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    // The stored-rectangle path must lay the columns out exactly like the box
    // path: a vertical box anchors its first column at the box's right edge, so
    // a caller that renders into the layer's previous rectangle still has to
    // pass the box. Without it every column sat left of the origin, the rerender
    // into a narrow stored rect came back empty, and the editor dropped the
    // layer's pixels (an edited trailing-glyph layer lost its text).
    vertical_impl(store, ed, text, origin, Some(target), box_size)
}

/// As [`render_vertical_from_engine_data`] for a vertical paragraph box: the
/// first column is anchored at the box's right edge (`origin` is the box's
/// top-left and `box_size` its width and height), columns advance leftwards one
/// em apart, and a column that runs past the box height soft-wraps into the
/// next one.
pub fn render_vertical_from_engine_data_in_box(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    box_size: (f32, f32),
) -> Result<LayerPixels, Error> {
    vertical_impl(store, ed, text, origin, None, Some(box_size))
}

/// Lays out a vertical (`Ornt = Vrtc`) point-text layer without rasterising it.
///
/// The mirror of [`render_vertical_from_engine_data`]: every glyph of a run is
/// set upright and stacked one em down the column, the pen centred in it, and
/// the caret stops run down the column (see
/// [`super::layout::TextLayout::vertical`]).
pub fn layout_vertical_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
) -> Result<super::layout::TextLayout, Error> {
    let prepared = vertical_prepare(store, ed, text, origin, None)?;
    Ok(super::layout::layout_vertical_of(
        &prepared,
        text,
        (0.0, 0.0),
    ))
}

/// As [`layout_vertical_from_engine_data`] for a vertical paragraph box: the
/// first column is anchored at the box's right edge (`origin` is the box's
/// top-left, `box_size` its width and height) and a column that runs past the
/// box height starts a new one (soft wrap).
pub fn layout_vertical_from_engine_data_in_box(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    box_size: (f32, f32),
) -> Result<super::layout::TextLayout, Error> {
    let prepared = vertical_prepare(store, ed, text, origin, Some(box_size))?;
    Ok(super::layout::layout_vertical_of(
        &prepared,
        text,
        (0.0, 0.0),
    ))
}

/// As [`layout_vertical_from_engine_data`] but reporting positions in document
/// space: the layout is the one [`render_vertical_from_engine_data_in_rect`]
/// rasterises and `TextLayout::anchor` is `target.anchor`.
pub fn layout_vertical_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    origin: (f32, f32),
    box_size: Option<(f32, f32)>,
    target: RectTarget,
) -> Result<super::layout::TextLayout, Error> {
    let prepared = vertical_prepare(store, ed, text, origin, box_size)?;
    let anchor = (
        target.origin.0 as f32 + target.anchor.0 - origin.0,
        target.origin.1 as f32 + target.anchor.1 - origin.1,
    );
    Ok(super::layout::layout_vertical_of(&prepared, text, anchor))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor points the law was measured on, straight from Photoshop's
    /// composed cache (`/99 /L` `/10` of the first column, see
    /// [`VERTICAL_B0_RATIOS`]): two fonts, four sizes, and not one size-proportional
    /// drift — 24/48/72 pt of the same font give the same ratio to 2e-7.
    #[test]
    fn the_anchor_is_size_proportional_at_every_measured_size() {
        let myr = (1000, 750, -250, Some(674), Some(484));
        let tim = (2048, 1825, -443, None, None);
        for (signature, size, want) in [
            (myr, 24.0, 8.95184),
            (myr, 48.0, 17.90369),
            (myr, 72.0, 26.85553),
            (tim, 24.0, 8.72388),
            (tim, 30.0, 10.90485),
            (tim, 48.0, 17.44775),
            (tim, 72.0, 26.17163),
        ] {
            let got = measured_ratio(signature).expect("measured face") * size;
            assert!(
                (got - want).abs() < 5e-5,
                "{signature:?} at {size} pt: {got} vs Photoshop's {want}"
            );
        }
    }

    /// The first column's centre is `b0` inside the frame's right edge — the
    /// number the app was missing by half an em (6.096 pt at 48 pt), not a
    /// per-size correction.
    #[test]
    fn the_first_column_centres_b0_inside_the_right_edge() {
        let myr = (1000, 750, -250, Some(674), Some(484));
        let width = 454.0f32; // `box_v`'s frame, 48 pt Myriad.
        let centre = width - measured_ratio(myr).expect("measured face") * 48.0;
        assert!((centre - 436.09631).abs() < 5e-4, "centre {centre}");
        // The column's em cell then overhangs the right edge by (1/2 - ratio)
        // em; Photoshop keeps the overhang, it does not clip it.
        assert!(centre + 24.0 > width);
        assert!((centre + 24.0 - 460.09631).abs() < 5e-4);
    }

    #[test]
    fn every_measured_font_round_trips_through_its_metric_signature() {
        for (signature, ratio) in VERTICAL_B0_RATIOS {
            assert_eq!(measured_ratio(*signature), Some(*ratio), "{signature:?}");
        }
        // A face outside the table has no measured anchor.
        assert_eq!(measured_ratio((1000, 1, -1, None, None)), None);
    }

    /// The along start is the column's justification offset inside the frame:
    /// Photoshop's `Center` caches give exactly `(height - extent)/2` and its
    /// `Left` caches `0`, at two fonts and four sizes.
    #[test]
    fn the_along_start_is_the_columns_justification_offset() {
        // 30 pt u12 layers, frame 200 pt, columns of 180 and 90 pt.
        assert_eq!(
            along_justification_offset(Justification::Center, 200.0, 180.0),
            10.0
        );
        assert_eq!(
            along_justification_offset(Justification::Center, 200.0, 90.0),
            55.0
        );
        // 30 pt, frame 179 pt, columns of 150 / 30 / 90 pt.
        assert_eq!(
            along_justification_offset(Justification::Center, 179.0, 150.0),
            14.5
        );
        assert_eq!(
            along_justification_offset(Justification::Center, 179.0, 30.0),
            74.5
        );
        assert_eq!(
            along_justification_offset(Justification::Center, 179.0, 90.0),
            44.5
        );
        // 30 pt, frame 334.63281 pt, 180 pt column (Photoshop: 77.31641).
        assert!(
            (along_justification_offset(Justification::Center, 334.63281, 180.0) - 77.31641).abs()
                < 1e-4
        );
        // `Left` boxes (24/48/72 pt Myriad) start every column at the frame top.
        for size in [24.0f32, 48.0, 72.0] {
            assert_eq!(
                along_justification_offset(Justification::Left, 642.0, 6.0 * size),
                0.0
            );
        }
        // Bottom = `Right`; a justified column behaves like `Left`.
        assert_eq!(
            along_justification_offset(Justification::Right, 200.0, 180.0),
            20.0
        );
        assert_eq!(
            along_justification_offset(Justification::JustifyAll, 200.0, 180.0),
            0.0
        );
        // A column longer than the frame never starts above the frame.
        assert_eq!(
            along_justification_offset(Justification::Center, 100.0, 180.0),
            0.0
        );
    }

    /// The signature is read from the face itself: NotoSans-Regular (checked in)
    /// must classify as the measured Noto ratio, so the anchor is
    /// `0.4029998 * size`, i.e. 19.34399 pt at 48 pt.
    #[test]
    fn a_face_classifies_by_its_own_metrics() {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets/fonts/NotoSans-Regular.ttf"),
        )
        .expect("checked-in font");
        let face = Face::parse(&bytes).expect("font");
        assert_eq!(
            anchor_signature(&face),
            (1000, 1069, -293, Some(714), Some(536))
        );
        let ratio = vertical_b0_ratio(&face);
        assert!((ratio - 0.4029998).abs() < 1e-7, "ratio {ratio}");
        assert!((ratio * 48.0 - 19.34399).abs() < 5e-5);
    }
}
