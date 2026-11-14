//! Horizontal point and area text composition.

use crate::area::{
    CacheDeltas, ParagraphStyle, first_baseline_ratio, layout_area, layout_area_runs,
    layout_area_runs_paragraphs_point_with_cache, layout_area_runs_paragraphs_with_cache,
};
use crate::engine_data::Value;
use crate::error::Error;
use crate::layout::{Kerning, TextStyle};
use crate::runs::{self, FontSet, RunStyle};

use super::paint::{WarpDraw, paint};
use super::{FaceStore, LayerPixels, LayerTextStyle, RectTarget, RunFeatures};

pub(super) fn text_style_of(run: &RunStyle, fallback: &TextStyle) -> TextStyle {
    let kerning = match (run.auto_kerning, run.kerning) {
        (Some(true), _) | (None, None) => Kerning::Metrics,
        (_, Some(k)) => Kerning::Manual(k),
        _ => Kerning::Metrics,
    };
    TextStyle {
        size: run.size.unwrap_or(fallback.size),
        tracking: run.tracking.unwrap_or(fallback.tracking),
        kerning,
        horizontal_scale: run.horizontal_scale.unwrap_or(1.0),
        baseline_shift: run.baseline_shift.unwrap_or(0.0),
        leading: fallback.leading,
        auto_leading: fallback.auto_leading,
    }
}

/// Clears the vertical-only flags a *horizontal* layout must ignore.
///
/// `BaselineDirection 2` (roman text rotated along a vertical column) is a
/// property of vertical type, but Photoshop's document default style sheet can
/// store `2` and a horizontal run that omits its own value inherits it. Shaping
/// such a run as rotated turned every horizontal glyph 90 degrees
/// (`Untitled-15.psd`: the ink became 16 px tall and 7 px wider instead of the
/// font's 24 px ascender), so every horizontal shaping entry drops the flag.
fn horizontal_runs(runs: &[RunStyle]) -> Vec<RunStyle> {
    let mut out = runs.to_vec();
    for run in &mut out {
        run.rotate_roman = None;
    }
    out
}

/// Maps each `EngineData` paragraph run onto the layout's paragraph style.
pub fn paragraph_styles_of(runs: &[runs::ParagraphRun]) -> Vec<ParagraphStyle> {
    runs.iter()
        .map(|p| ParagraphStyle {
            justification: p.justification,
            first_line_indent: p.first_line_indent,
            start_indent: p.start_indent,
            end_indent: p.end_indent,
            space_before: p.space_before,
            space_after: p.space_after,
            word_spacing: p.word_spacing,
            letter_spacing: p.letter_spacing,
            glyph_spacing: p.glyph_scaling,
            auto_leading: p.auto_leading,
            every_line_composer: p.every_line_composer,
            hanging: p.hanging,
            hyphenate: p.hyphenate,
            words_longer_than: p.words_longer_than,
            pre_hyphen: p.pre_hyphen,
            post_hyphen: p.post_hyphen,
            consecutive_hyphens: p.consecutive_hyphens,
            hyphen_zone: p.hyphen_zone,
        })
        .collect()
}

/// Clears the paragraph geometry that must not affect an unwrapped probe:
/// alignment against a huge frame would push centred/indented lines away.
fn probe_paragraphs(paragraphs: &[ParagraphStyle]) -> Vec<ParagraphStyle> {
    let mut out = paragraphs.to_vec();
    if out.is_empty() {
        out.push(ParagraphStyle::default());
    }
    for p in &mut out {
        p.justification = crate::area::Justification::Left;
        p.first_line_indent = 0.0;
        p.start_indent = 0.0;
        p.end_indent = 0.0;
        p.space_before = 0.0;
        p.space_after = 0.0;
    }
    out
}

/// Lays out and rasterises `text` into a canvas the size of the ink.
///
/// `faces` must contain the run fonts; `runs` may be empty, in which case
/// `spec.style` and the default face are used.
pub fn render_text_layer(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
) -> Result<LayerPixels, Error> {
    render_text_layer_with_atlas(faces, text, runs, spec, None, None)
}

/// As [`render_text_layer`], with one captured glyph atlas and an antialias
/// override.
pub fn render_text_layer_with_atlas(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
    atlas: Option<&crate::atlas::Atlas>,
    aa: Option<crate::raster::AntiAlias>,
) -> Result<LayerPixels, Error> {
    render_text_layer_with_atlases(
        faces,
        text,
        runs,
        spec,
        atlas.map(std::slice::from_ref).unwrap_or(&[]),
        aa,
    )
}

/// As [`render_text_layer_with_atlas`], with every captured pen phase attached.
pub fn render_text_layer_with_atlases(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    spec: &LayerTextStyle,
    atlases: &[crate::atlas::Atlas],
    aa: Option<crate::raster::AntiAlias>,
) -> Result<LayerPixels, Error> {
    let face = runs
        .first()
        .and_then(|r| r.font.as_deref())
        .and_then(|name| faces.get(name))
        .unwrap_or_else(|| faces.default_face());
    let ascender = first_baseline_ratio(face);
    let style = runs
        .first()
        .map(|r| text_style_of(r, &spec.style))
        .unwrap_or(spec.style);

    // The multi-run path carries the per-character attributes (vertical scale,
    // faux italic, caps, colour) into `Line::attrs`; the single-style
    // `layout_area` cannot, so anything that changes between characters needs
    // it. `runs` is usually one run from the live editor, but that run still
    // sets vertical scale and faux styles. Horizontal text ignores the
    // vertical-only `rotate_roman` flag (see [`horizontal_runs`]).
    let clean_runs;
    let runs = if runs.is_empty() {
        runs
    } else {
        clean_runs = horizontal_runs(runs);
        clean_runs.as_slice()
    };
    let layout = if runs.is_empty() {
        layout_area(
            faces.default_face(),
            text,
            &style,
            &spec.paragraph,
            spec.box_size.0,
            spec.box_size.1,
            spec.origin,
            ascender,
        )
    } else {
        layout_area_runs(
            faces,
            text,
            runs,
            &style,
            &spec.paragraph,
            spec.box_size.0,
            spec.box_size.1,
            spec.origin,
            ascender,
        )
    };
    paint(
        faces,
        layout.lines,
        runs,
        spec,
        aa.unwrap_or(crate::raster::AntiAlias::Sharp),
        atlases,
        None,
        None,
        Some(layout.first_baseline),
    )
}

/// Lays out a layer straight from its `EngineData` and rasterises it.
///
/// The paragraph sheets are read from the layer's own
/// `EngineDict/ParagraphRun` (every run, with its `RunLengthArray`), so indents,
/// space before/after, auto leading, justification, hanging punctuation,
/// every-line composer and hyphenation are applied per paragraph. Point text
/// (`box_size` zero) is measured unwrapped first.
pub fn render_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    render_from_engine_data_with_paragraphs(store, ed, text, &[], box_size, origin)
}

/// Everything the layout and the rasteriser share for one horizontal layer:
/// shaped runs, resolved style, placed lines and the antialias mode.
///
/// [`render_impl`] paints [`Prepared::lines`]; `layout_from_engine_data*`
/// describes them. Both call [`prepare_impl`], so a caret cannot drift from the
/// pixels.
pub(super) struct Prepared<'a> {
    pub(super) faces: FontSet<'a>,
    pub(super) style_runs: Vec<RunStyle>,
    pub(super) spec: LayerTextStyle,
    pub(super) lines: Vec<crate::area::Line>,
    pub(super) empty: Vec<crate::area::EmptyParagraph>,
    /// The layout's first paragraph baseline, before empty-paragraph stepping;
    /// [`super::layout::layout_of`] and [`paint`] flip `y` around it so a caret
    /// and the pixels agree on the frame's first baseline.
    pub(super) first_baseline: f32,
    pub(super) aa: crate::raster::AntiAlias,
}

/// Lays a layer out (shape, break, justify, place) without rasterising it.
#[allow(clippy::too_many_arguments)]
fn prepare_impl<'a>(
    store: &'a FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    baseline: Option<f32>,
) -> Result<Prepared<'a>, Error> {
    let mut style_runs = horizontal_runs(&runs::from_engine_data(ed));
    for (i, f) in features.iter().enumerate() {
        if let Some(run) = style_runs.get_mut(i) {
            run.old_style |= f.old_style;
        }
    }
    let aa = runs::antialias(ed);
    let stored_paragraphs;
    let paragraphs = if paragraphs.is_empty() {
        stored_paragraphs = runs::paragraphs_from_engine_data(ed);
        stored_paragraphs.as_slice()
    } else {
        paragraphs
    };
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
    let mut style = text_style_of(&style_runs[0], &base);
    let optical_kerning = features.iter().any(|f| f.optical_kerning);
    if optical_kerning {
        // Optical mode replaces the font's pair kerning; keep the parsed sheets
        // and change only the two fields that would force metrics back on.
        style.kerning = crate::layout::Kerning::Optical;
        for run in &mut style_runs {
            run.auto_kerning = None;
            run.kerning = None;
        }
    }
    let paras = paragraph_styles_of(paragraphs);
    let primary = style_runs[0]
        .font
        .as_deref()
        .and_then(|n| faces.get(n))
        .unwrap_or_else(|| faces.default_face());
    let ascender = baseline.unwrap_or_else(|| first_baseline_ratio(primary));

    let advance_deltas: Vec<f32> = features
        .iter()
        .flat_map(|f| f.advance_deltas.iter().copied())
        .collect();
    // The word-spacing sheet is per text, not per run: the first populated
    // entry carries it (see `RunFeatures::word_spacing_deltas`).
    let word_spacing_deltas: Vec<f32> = features
        .iter()
        .map(|f| f.word_spacing_deltas.as_slice())
        .find(|deltas| !deltas.is_empty())
        .unwrap_or_default()
        .to_vec();
    // Point text has no frame: measure it unwrapped and size the canvas to the
    // laid-out lines instead of to the layer's old ink rectangle. Its lines
    // align against the type origin, not against the widest line.
    let point_text = !(box_size.0 > 0.0 && box_size.1 > 0.0);
    let box_size = if box_size.0 > 0.0 && box_size.1 > 0.0 {
        box_size
    } else {
        // Measure flush left: alignment against a 1e7 pt frame would push a
        // centred line millions of points right and the canvas could not hold it.
        let probe_styles = probe_paragraphs(&paras);
        let probe = layout_area_runs_paragraphs_with_cache(
            &faces,
            text,
            &style_runs,
            &style,
            &probe_styles,
            1.0e7,
            1.0e7,
            origin,
            ascender,
            CacheDeltas {
                glyphs: &advance_deltas,
                spaces: &word_spacing_deltas,
            },
        );
        let width = probe
            .lines
            .iter()
            .flat_map(|l| l.glyphs.iter())
            .map(|g| g.x + g.advance)
            .fold(1.0f32, f32::max);
        let (mut top, mut bottom) = (f32::MAX, f32::MIN);
        for line in &probe.lines {
            top = top.min(line.baseline);
            bottom = bottom.max(line.baseline);
        }
        let biggest = style_runs
            .iter()
            .map(|r| r.size.unwrap_or(style.size))
            .fold(style.size, f32::max);
        let height = (bottom - top) + biggest * 2.0;
        (width + 2.0, height + biggest)
    };

    let layout = if point_text {
        layout_area_runs_paragraphs_point_with_cache(
            &faces,
            text,
            &style_runs,
            &style,
            &paras,
            box_size.0,
            box_size.1,
            origin,
            ascender,
            CacheDeltas {
                glyphs: &advance_deltas,
                spaces: &word_spacing_deltas,
            },
        )
    } else {
        layout_area_runs_paragraphs_with_cache(
            &faces,
            text,
            &style_runs,
            &style,
            &paras,
            box_size.0,
            box_size.1,
            origin,
            ascender,
            CacheDeltas {
                glyphs: &advance_deltas,
                spaces: &word_spacing_deltas,
            },
        )
    };
    let spec = LayerTextStyle {
        style,
        paragraph: paras.first().copied().unwrap_or_default(),
        box_size,
        origin,
        color: style_runs[0].color.unwrap_or([0, 0, 0]),
    };
    let trace_level: u32 = std::env::var("PHOTOCRAFT_TRACE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if trace_level >= 2 {
        eprintln!(
            "layout: size={:?} box={:?} origin={:?} asc={:.5} first_off={:.3} paras={}",
            style.size,
            box_size,
            origin,
            ascender,
            crate::area::first_baseline_offset(ascender, style.size),
            paras.len()
        );
        for (i, line) in layout.lines.iter().enumerate() {
            let text: String = line.glyphs.iter().map(|g| g.ch).collect();
            let last = line.glyphs.last().map(|g| g.x + g.advance).unwrap_or(0.0);
            eprintln!(
                "line {i}: baseline={:.3} x0={:.3} xend={:.3} glyphs={:?} {:?}",
                line.baseline,
                line.glyphs.first().map(|g| g.x).unwrap_or(0.0),
                last,
                line.glyphs.len(),
                text
            );
            if trace_level >= 3 {
                for g in &line.glyphs {
                    eprintln!(
                        "   {:?} x={:.3} adv={:.3} y={:.3}",
                        g.ch, g.x, g.advance, g.y
                    );
                }
            }
        }
    }
    Ok(Prepared {
        faces,
        style_runs,
        spec,
        lines: layout.lines,
        empty: layout.empty,
        first_baseline: layout.first_baseline,
        aa,
    })
}

/// Rasterises a prepared layout.
fn render_prepared(
    prepared: Prepared<'_>,
    store: &FaceStore,
    target: Option<RectTarget>,
    warp: Option<&WarpDraw<'_>>,
) -> Result<LayerPixels, Error> {
    paint(
        &prepared.faces,
        prepared.lines,
        &prepared.style_runs,
        &prepared.spec,
        prepared.aa,
        store.atlases(),
        target,
        warp,
        Some(prepared.first_baseline),
    )
}

/// As [`render_from_engine_data`] with paragraph styles supplied by the caller.
///
/// `paragraphs` are indexed by paragraph in text order (`len` in UTF-16 code
/// units, paragraph separators included; the last entry repeats). This is the
/// entry point for a document model that keeps paragraph sheets outside
/// `EngineData`; passing an empty slice falls back to the layer's own sheets.
///
/// Returns the ink-cropped RGBA surface and its offset from the text origin
/// (`LayerPixels::offset`, measured from the first baseline vertically), the
/// same contract as [`render_from_engine_data`].
#[allow(clippy::too_many_arguments)]
pub(super) fn render_impl(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: Option<RectTarget>,
    warp: Option<&WarpDraw<'_>>,
    baseline: Option<f32>,
) -> Result<LayerPixels, Error> {
    let prepared = prepare_impl(
        store, ed, text, paragraphs, features, box_size, origin, baseline,
    )?;
    render_prepared(prepared, store, target, warp)
}

/// Lays a layer out with the render entry points' arguments and returns the
/// layout it would rasterise.
#[allow(clippy::too_many_arguments)]
fn layout_impl(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: Option<RectTarget>,
) -> Result<super::layout::TextLayout, Error> {
    let prepared = prepare_impl(store, ed, text, paragraphs, features, box_size, origin, None)?;
    // The document position of a point is the anchor plus the layout position,
    // with the same fold the rasteriser applies: the canvas column is
    // `target.origin + target.anchor - origin + pen`.
    let anchor = match target {
        Some(t) => (
            t.origin.0 as f32 + t.anchor.0 - origin.0,
            t.origin.1 as f32 + t.anchor.1 - origin.1,
        ),
        None => (0.0, 0.0),
    };
    Ok(super::layout::layout_of(&prepared, text, anchor))
}

/// As [`render_from_engine_data`] with paragraph styles supplied by the caller.
///
/// `paragraphs` are indexed by paragraph in text order (`len` in UTF-16 code
/// units, paragraph separators included; the last entry repeats). This is the
/// entry point for a document model that keeps paragraph sheets outside
/// `EngineData`; passing an empty slice falls back to the layer's own sheets.
///
/// Returns the ink-cropped RGBA surface and its offset from the text origin
/// (`LayerPixels::offset`, measured from the first baseline vertically), the
/// same contract as [`render_from_engine_data`].
pub fn render_from_engine_data_with_paragraphs(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        &[],
        box_size,
        origin,
        None,
        None,
        None,
    )
}

/// Lays out and rasterises a layer straight into a document rectangle.
///
/// This is the entry point for comparing against Photoshop's stored layer
/// pixels: `target.anchor` is the document position of the text origin (the
/// `TySh` translation for point text, the frame origin for area text) and
/// `target.origin`/`target.size` are the layer record's rectangle, so glyphs are
/// rasterised with the same subpixel phase Photoshop used. The returned
/// [`LayerPixels::offset`] is `(0, 0)` — the surface already sits at
/// `target.origin`.
pub fn render_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        &[],
        box_size,
        origin,
        Some(target),
        None,
        None,
    )
}

/// As [`render_from_engine_data_with_paragraphs`], with per-style-run character
/// features that `EngineData` does not carry (`old_style` figures live only in
/// `Txt2` `/6/30`). `features` is indexed by style run; missing entries mean
/// "feature off".
pub fn render_from_engine_data_with_paragraphs_and_features(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<LayerPixels, Error> {
    render_impl(
        store, ed, text, paragraphs, features, box_size, origin, None, None, None,
    )
}

/// As [`render_from_engine_data_with_paragraphs_and_features`], with an
/// explicit first-baseline ratio instead of the font's cap-height default.
///
/// `ascender_ratio` is `Some` for a layer whose `Txt2` frame selects a
/// different First baseline (see [`crate::area::FirstBaseline`]); Photoshop
/// places the first line at `ascender_ratio * size` below the frame top. The
/// layout and the returned [`LayerPixels::offset`] use the same ratio, so the
/// caller anchors the result exactly like the default path.
#[allow(clippy::too_many_arguments)]
pub fn render_from_engine_data_with_paragraphs_and_features_baseline(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    ascender_ratio: Option<f32>,
) -> Result<LayerPixels, Error> {
    render_impl(
        store, ed, text, paragraphs, features, box_size, origin, None, None, ascender_ratio,
    )
}

/// [`render_from_engine_data_in_rect_with_features`] with the explicit
/// first-baseline ratio of
/// [`render_from_engine_data_with_paragraphs_and_features_baseline`].
#[allow(clippy::too_many_arguments)]
pub fn render_from_engine_data_in_rect_with_features_baseline(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
    ascender_ratio: Option<f32>,
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        features,
        box_size,
        origin,
        Some(target),
        None,
        ascender_ratio,
    )
}

/// As [`render_from_engine_data_in_rect`], with the `Txt2`-only character
/// features of [`render_from_engine_data_with_paragraphs_and_features`].
#[allow(clippy::too_many_arguments)]
pub fn render_from_engine_data_in_rect_with_features(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
) -> Result<LayerPixels, Error> {
    render_impl(
        store,
        ed,
        text,
        paragraphs,
        features,
        box_size,
        origin,
        Some(target),
        None,
        None,
    )
}

/// Lays a layer out straight from its `EngineData`, without rasterising it.
///
/// This is [`render_from_engine_data`] with the rasteriser stopped: the same
/// shaping, breaking, justification, indents and leading, so the glyph pens,
/// line boxes and caret stops returned here are exactly what the pixels show.
/// [`super::layout::TextLayout`] documents the coordinate space.
pub fn layout_from_engine_data(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<super::layout::TextLayout, Error> {
    layout_impl(store, ed, text, &[], &[], box_size, origin, None)
}

/// As [`layout_from_engine_data`] with paragraph styles supplied by the caller
/// (the mirror of [`render_from_engine_data_with_paragraphs`]).
pub fn layout_from_engine_data_with_paragraphs(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<super::layout::TextLayout, Error> {
    layout_impl(store, ed, text, paragraphs, &[], box_size, origin, None)
}

/// As [`layout_from_engine_data_with_paragraphs`], with the `Txt2`-only
/// character features of [`render_from_engine_data_with_paragraphs_and_features`].
pub fn layout_from_engine_data_with_paragraphs_and_features(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
) -> Result<super::layout::TextLayout, Error> {
    layout_impl(
        store, ed, text, paragraphs, features, box_size, origin, None,
    )
}

/// As [`layout_from_engine_data`], reporting positions in document space: the
/// layout is the one [`render_from_engine_data_in_rect`] rasterises, and
/// `TextLayout::anchor` is `target.anchor`, so the document position of a point
/// is `anchor + (x, y)`.
#[allow(clippy::too_many_arguments)]
pub fn layout_from_engine_data_in_rect(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
) -> Result<super::layout::TextLayout, Error> {
    layout_impl(
        store,
        ed,
        text,
        paragraphs,
        &[],
        box_size,
        origin,
        Some(target),
    )
}

/// As [`layout_from_engine_data_in_rect`], with the `Txt2`-only character
/// features of [`render_from_engine_data_in_rect_with_features`].
#[allow(clippy::too_many_arguments)]
pub fn layout_from_engine_data_in_rect_with_features(
    store: &FaceStore,
    ed: &Value,
    text: &str,
    paragraphs: &[runs::ParagraphRun],
    features: &[RunFeatures],
    box_size: (f32, f32),
    origin: (f32, f32),
    target: RectTarget,
) -> Result<super::layout::TextLayout, Error> {
    layout_impl(
        store,
        ed,
        text,
        paragraphs,
        features,
        box_size,
        origin,
        Some(target),
    )
}
