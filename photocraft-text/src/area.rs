//! Area (paragraph) text: paragraph splitting, word wrapping and justification.

use std::ops::Range;

use crate::composer::{Breaker, Extent, Piece, Pieces};
use crate::font::Face;
use crate::layout::{GlyphPos, TextStyle, shape};
use crate::runs::{FontSet, GlyphAttrs, RunStyle, shape_runs_attrs};
use crate::typography::{self, left_hang_shift};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Justification {
    #[default]
    Left,
    Right,
    Center,
    /// `FULLJUSTIFYLASTLINELEFT`: justify, last line flush left (Photoshop's
    /// "Justify last left").
    JustifyLastLeft,
    /// `FULLJUSTIFYLASTLINECENTER`: justify, last line centred.
    JustifyLastCenter,
    /// `FULLJUSTIFYLASTLINERIGHT`: justify, last line flush right.
    JustifyLastRight,
    /// `FULLJUSTIFY`: justify every line, including the last.
    JustifyAll,
}

impl Justification {
    /// True for the four justify variants, which stretch interior spaces.
    pub fn is_justify(self) -> bool {
        matches!(
            self,
            Self::JustifyLastLeft
                | Self::JustifyLastCenter
                | Self::JustifyLastRight
                | Self::JustifyAll
        )
    }
}

impl Justification {
    /// Photoshop's stored code (`Justification` in EngineData, `/0` in `Txt2`).
    ///
    /// Code 6 is `Justification.FULLYJUSTIFIED` (docs/txt2-map.md) and it
    /// really does stretch the last line: in `out/probe5/h2.pdf` the area
    /// paragraph set with `hanging: true, just: "full"` has its last line
    /// `and then left.` stretched from 216.08 to 300.01 pt, with the line's
    /// hanging period appended after the justified content. Codes 3/4/5 are
    /// unreachable on CC 2019 (`FULLJUSTIFYLASTLINE*` is "Invalid enumeration
    /// value", `out/probe5/mC1.json`), so `JustifyLastLeft` has no PSD code.
    pub fn from_psd(value: i32) -> Self {
        match value {
            1 => Self::Right,
            2 => Self::Center,
            3 => Self::JustifyLastLeft,
            4 => Self::JustifyLastRight,
            5 => Self::JustifyLastCenter,
            6 => Self::JustifyAll,
            _ => Self::Left,
        }
    }
}

/// Paragraph formatting, units in points (EngineData stores 1/1000 em for the
/// spacing multipliers and points for the indents).
#[derive(Clone, Copy, Debug)]
pub struct ParagraphStyle {
    pub justification: Justification,
    pub first_line_indent: f32,
    pub start_indent: f32,
    pub end_indent: f32,
    pub space_before: f32,
    pub space_after: f32,
    /// [minimum, desired, maximum] word spacing as multipliers of the space width.
    pub word_spacing: [f32; 3],
    pub letter_spacing: [f32; 3],
    pub glyph_spacing: [f32; 3],
    /// Auto-leading fraction (`AutoLeading` in the paragraph sheet, 1.2 by
    /// default); runs without an explicit leading step by `size x this`.
    pub auto_leading: f32,
    /// Knuth-Plass style every-line composer instead of first-fit breaking.
    pub every_line_composer: bool,
    /// Hanging punctuation: opening glyphs hang past the left edge, closing
    /// glyphs at a line end may hang past the right edge.
    pub hanging: bool,
    /// `AutoHyphenate`: place hyphens at the dictionary's break points.
    pub hyphenate: bool,
    /// `HyphenatedWordSize`: only hyphenate words longer than this.
    pub words_longer_than: i64,
    /// `PreHyphen`: letters required before the hyphen.
    pub pre_hyphen: i64,
    /// `PostHyphen`: letters required after the hyphen.
    pub post_hyphen: i64,
    /// `ConsecutiveHyphens`: maximum number of consecutive hyphenated lines.
    pub consecutive_hyphens: i64,
    /// `Zone`: hyphenation zone in points (single-line composer).
    pub hyphen_zone: f32,
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        Self {
            justification: Justification::Left,
            first_line_indent: 0.0,
            start_indent: 0.0,
            end_indent: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            word_spacing: [0.8, 1.0, 1.33],
            letter_spacing: [0.0, 0.0, 0.0],
            glyph_spacing: [1.0, 1.0, 1.0],
            auto_leading: 1.2,
            every_line_composer: false,
            hanging: false,
            hyphenate: false,
            words_longer_than: 6,
            pre_hyphen: 2,
            post_hyphen: 2,
            consecutive_hyphens: 8,
            hyphen_zone: 36.0,
        }
    }
}

/// One placed line. `attrs` is parallel to `glyphs`.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub glyphs: Vec<GlyphPos>,
    pub baseline: f32,
    pub width: f32,
    pub attrs: Vec<GlyphAttrs>,
    /// Bytes of the layer text the line's glyphs were shaped from, without the
    /// paragraph separator (`attrs`' source ranges, min start .. max end).
    /// Empty when the line has no glyphs.
    pub range_start: usize,
    pub range_end: usize,
    /// Index of the paragraph the line belongs to, in text order.
    pub paragraph: usize,
}

/// A paragraph that produced no line because it holds no text.
///
/// It still occupies one leading of vertical space, so a caret needs a
/// baseline to sit on; [`AreaLayout::lines`] cannot carry it (no glyphs).
#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyParagraph {
    /// Index of the paragraph in text order.
    pub paragraph: usize,
    /// The paragraph's separator bytes in the layer text (empty for a trailing
    /// empty paragraph).
    pub range_start: usize,
    pub range_end: usize,
    /// Where the paragraph's first line would have started: its baseline in the
    /// layout's y-up space (before the empty paragraph steps one leading).
    pub baseline: f32,
    /// X of the paragraph's first-line start (origin plus the indents).
    pub x: f32,
    /// The leading the empty paragraph occupies (`size x auto_leading`).
    pub leading: f32,
}

/// Result of laying out one paragraph inside a box.
#[derive(Clone, Debug, Default)]
pub struct AreaLayout {
    pub lines: Vec<Line>,
    /// Empty paragraphs in text order, with the baseline a caret must use.
    pub empty: Vec<EmptyParagraph>,
}

impl AreaLayout {
    pub fn glyphs(&self) -> impl Iterator<Item = &GlyphPos> {
        self.lines.iter().flat_map(|l| l.glyphs.iter())
    }
}

/// Photoshop's first-baseline ratio for a font: the fraction of the point size
/// added to the box top before the first baseline.
///
/// The ratio is the ink `yMax` of `d` over the em square, which is what
/// [`Face::ascent_ratio`] computes (falling back to `b`, `h`, `H`, then
/// `hhea`). Measured against Photoshop 20.0.2 this reproduces every probed font
/// within 0.0012: 14 fonts in `tests/data/font_ascent.json` (ArialMT 0.716,
/// NotoSans-Regular 0.760, CCGrimNGritty-Bold 0.801, ...) and the 13 fonts of
/// `out/probe5/font_ratios.json` (Vampire95 0.465 vs the exact d ratio 0.46603,
/// v_AgencyFB 0.7644, ...). It is not `hhea`/OS/2 ascendancy: CC Wild But Not
/// Crazy places its first baseline at 0.77 x size while its `hhea` ascender is
/// 0.838.
pub fn first_baseline_ratio(face: &Face<'_>) -> f32 {
    face.ascent_ratio()
}

/// Photoshop's choice of the metric that places the first baseline in area
/// text (the Paragraph panel's "First baseline").
///
/// `Txt2` stores it per frame as `/0 /8 /0 [frame] /0 /2 /10 /0`; a frame
/// without the record uses Photoshop's default, [`FirstBaseline::Ascent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstBaseline {
    /// The default (frame record absent or `1`): Photoshop's Ascent, the ink
    /// top of `d`, measured at `0.69499 * size` for Times New Roman 33 pt.
    Ascent,
    /// The cap height (`2` in the frame record): the shorter ink top of `H`
    /// and `d`, what [`first_baseline_ratio`] has always returned.
    CapHeight,
}

/// First-baseline ratio of `face` for the [`FirstBaseline`] choice.
pub fn first_baseline_ratio_kind(face: &Face<'_>, kind: FirstBaseline) -> f32 {
    match kind {
        FirstBaseline::Ascent => face.default_ascent_ratio(),
        FirstBaseline::CapHeight => face.ascent_ratio(),
    }
}

/// Distance from the top of an area-text box to its first baseline, in points.
///
/// This is the formula Photoshop uses for area text: the ink top of `d` (see
/// [`first_baseline_ratio`]) times the point size.
pub fn first_baseline(face: &Face<'_>, size: f32) -> f32 {
    first_baseline_offset(face.ascent_ratio(), size)
}

/// Distance from the top of an area-text box to the first baseline, in points.
///
/// Photoshop offsets the first line by the font's ascender; `ascender_ratio`
/// is that ascender as a fraction of the em square.
pub fn first_baseline_offset(ascender_ratio: f32, size: f32) -> f32 {
    ascender_ratio * size
}

/// Lays out `text` in a box of `width` points starting at `origin`, with the
/// first line's top edge at `origin.y`. Single face, single style: all glyphs
/// carry run 0, black, unscaled attributes.
pub fn layout_area(
    face: &Face<'_>,
    text: &str,
    style: &TextStyle,
    para: &ParagraphStyle,
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
) -> AreaLayout {
    layout_area_paragraphs(
        face,
        text,
        style,
        std::slice::from_ref(para),
        width,
        height,
        origin,
        ascender_ratio,
    )
}

/// One shaped paragraph ready for line breaking.
struct Para {
    items: Vec<(GlyphPos, GlyphAttrs)>,
    /// Bytes of the layer text this paragraph covers, separator excluded.
    span: (usize, usize),
    /// Indexed by line end (0 ..= items.len()): the hyphen Photoshop draws when
    /// the line breaks there. `None` means no hyphenation point.
    soft: Vec<Option<SoftBreak>>,
    style: ParagraphStyle,
}

/// A hyphenation hyphen, drawn only when a line actually ends at its point.
#[derive(Clone, Copy, Debug)]
struct SoftBreak {
    gid: u16,
    advance: f32,
    /// Attributes (run, colour, scale) of the preceding glyph.
    attrs: GlyphAttrs,
    /// Baseline shift of the preceding glyph.
    shift: f32,
}

fn plain_attrs(style: &TextStyle) -> GlyphAttrs {
    GlyphAttrs {
        run: 0,
        color: [0; 3],
        vertical_scale: 1.0,
        x_scale: 1.0,
        faux_italic: false,
        leading: style.leading_pt(),
        rotate: false,
        source: (0, 0),
    }
}

/// Paragraph `i`'s style: the list is indexed by paragraph, a short list
/// repeats its last entry and an empty one means the default sheet.
fn paragraph_style(paragraphs: &[ParagraphStyle], i: usize) -> ParagraphStyle {
    paragraphs
        .get(i)
        .or_else(|| paragraphs.last())
        .copied()
        .unwrap_or_default()
}

/// Lays out single-face `text` with one [`ParagraphStyle`] per paragraph.
pub fn layout_area_paragraphs(
    face: &Face<'_>,
    text: &str,
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
) -> AreaLayout {
    layout_area_paragraphs_with_advances(
        face,
        text,
        style,
        paragraphs,
        width,
        height,
        origin,
        ascender_ratio,
        &[],
    )
}

/// As [`layout_area_paragraphs`] with per-glyph advance corrections.
///
/// `advances` holds one extra advance per shaped glyph in text order
/// (spaces included, dictionary hyphens excluded); it is how a caller applies
/// Txt2's composed cache: the every-line composer's per-run deltas and optical
/// kerning. When it is non-empty the uniform every-line nudge is not applied.
#[allow(clippy::too_many_arguments)]
pub fn layout_area_paragraphs_with_advances(
    face: &Face<'_>,
    text: &str,
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    advances: &[f32],
) -> AreaLayout {
    let mut paras = Vec::new();
    let mut offset = 0usize;
    for (i, span) in paragraph_spans(text).into_iter().enumerate() {
        let pstyle = paragraph_style(paragraphs, i);
        let slice = &text[span.bytes.clone()];
        let shape_style = TextStyle {
            auto_leading: pstyle.auto_leading,
            ..*style
        };
        let mut items: Vec<(GlyphPos, GlyphAttrs)> = if slice.is_empty() {
            Vec::new()
        } else {
            shape(face, slice, &shape_style)
                .into_iter()
                .map(|g| (g, plain_attrs(&shape_style)))
                .collect()
        };
        apply_advances(&mut items, advances, offset, |_, _| {
            shape_style.tracking as f32 / 1000.0 * shape_style.size * shape_style.horizontal_scale
        });
        offset += items.len();
        let mut soft = vec![None; items.len() + 1];
        if pstyle.hyphenate {
            let hyphen = |_: &GlyphAttrs| hyphen_glyph(face, &shape_style);
            insert_hyphens(&items, &mut soft, &pstyle, hyphen);
        }
        paras.push(Para {
            items,
            span: (span.bytes.start, span.bytes.end),
            soft,
            style: pstyle,
        });
    }
    layout_paragraphs(
        &paras,
        style,
        width,
        height,
        origin,
        ascender_ratio,
        advances.is_empty(),
        false,
    )
}

/// Lays out multi-run `text`. `runs` lengths are UTF-16 units covering `text`.
/// Each glyph's [`GlyphAttrs::run`] indexes `runs`.
#[allow(clippy::too_many_arguments)]
pub fn layout_area_runs(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    para: &ParagraphStyle,
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
) -> AreaLayout {
    layout_area_runs_paragraphs(
        faces,
        text,
        runs,
        style,
        std::slice::from_ref(para),
        width,
        height,
        origin,
        ascender_ratio,
    )
}

/// Lays out multi-run `text` with one [`ParagraphStyle`] per paragraph.
///
/// Paragraph styles are indexed by paragraph, in text order; the last entry
/// applies to every remaining paragraph. Auto leading is a paragraph property,
/// so each paragraph is shaped with its own fraction on the [`TextStyle`] the
/// runs resolve against.
#[allow(clippy::too_many_arguments)]
pub fn layout_area_runs_paragraphs(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
) -> AreaLayout {
    layout_area_runs_paragraphs_with_advances(
        faces,
        text,
        runs,
        style,
        paragraphs,
        width,
        height,
        origin,
        ascender_ratio,
        &[],
    )
}

/// Photoshop's composed-cache corrections for one layer (`Txt2` `/3`).
///
/// The cache records two kinds of correction and the engine applies them the
/// way CoolType did:
///
/// * `glyphs` carries a correction per **shaped glyph** in text order (the
///   optical-kerning and tracking sheets); it is applied by
///   [`apply_advances`];
/// * `spaces` carries a correction per **word space** in text order — the
///   every-line composer balances a line by moving its spaces
///   (`05 area indents spacing` records eight `-.0468` values on line 1 and
///   seven `+.0468` on lines 4/5 of its cache), which no per-glyph sheet can
///   express because the spaces move together with the words.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CacheDeltas<'a> {
    /// One correction per shaped glyph in text order; empty when the cache has
    /// no glyph sheet.
    pub glyphs: &'a [f32],
    /// One correction per word space in text order; empty when the cache has
    /// no word-spacing sheet.
    pub spaces: &'a [f32],
}

impl CacheDeltas<'_> {
    /// `true` when there is nothing to correct.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty() && self.spaces.is_empty()
    }
}

/// As [`layout_area_runs_paragraphs`] with per-glyph advance corrections, in
/// points, one per shaped glyph in text order (spaces included).
#[allow(clippy::too_many_arguments)]
pub fn layout_area_runs_paragraphs_with_advances(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    advances: &[f32],
) -> AreaLayout {
    layout_area_runs_paragraphs_impl(
        faces,
        text,
        runs,
        style,
        paragraphs,
        width,
        height,
        origin,
        ascender_ratio,
        CacheDeltas {
            glyphs: advances,
            spaces: &[],
        },
        false,
    )
}

/// As [`layout_area_runs_paragraphs_with_advances`], with both sheets of
/// Photoshop's composed cache (per-glyph and per-space corrections).
#[allow(clippy::too_many_arguments)]
pub fn layout_area_runs_paragraphs_with_cache(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    deltas: CacheDeltas<'_>,
) -> AreaLayout {
    layout_area_runs_paragraphs_impl(
        faces,
        text,
        runs,
        style,
        paragraphs,
        width,
        height,
        origin,
        ascender_ratio,
        deltas,
        false,
    )
}

/// As [`layout_area_runs_paragraphs_with_advances`], but with point-text
/// semantics when `align_around_origin` is set: alignment moves each line
/// against the type origin instead of the frame edge (there is no frame).
#[allow(clippy::too_many_arguments)]
pub fn layout_area_runs_paragraphs_point(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    advances: &[f32],
) -> AreaLayout {
    layout_area_runs_paragraphs_impl(
        faces,
        text,
        runs,
        style,
        paragraphs,
        width,
        height,
        origin,
        ascender_ratio,
        CacheDeltas {
            glyphs: advances,
            spaces: &[],
        },
        true,
    )
}

/// As [`layout_area_runs_paragraphs_point`], with both cache sheets.
#[allow(clippy::too_many_arguments)]
pub fn layout_area_runs_paragraphs_point_with_cache(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    deltas: CacheDeltas<'_>,
) -> AreaLayout {
    layout_area_runs_paragraphs_impl(
        faces,
        text,
        runs,
        style,
        paragraphs,
        width,
        height,
        origin,
        ascender_ratio,
        deltas,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_area_runs_paragraphs_impl(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
    paragraphs: &[ParagraphStyle],
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    deltas: CacheDeltas<'_>,
    align_around_origin: bool,
) -> AreaLayout {
    let mut paras = Vec::new();
    let mut offset = 0usize;
    let mut space_offset = 0usize;
    for (i, span) in paragraph_spans(text).into_iter().enumerate() {
        let pstyle = paragraph_style(paragraphs, i);
        let slice = &text[span.bytes.clone()];
        let shape_style = TextStyle {
            auto_leading: pstyle.auto_leading,
            ..*style
        };
        let mut items: Vec<(GlyphPos, GlyphAttrs)> = if slice.is_empty() {
            Vec::new()
        } else {
            let (sub, map) = sub_runs(runs, span.utf16);
            let mut shaped = shape_runs_attrs(faces, slice, &sub, &shape_style);
            for (_, attr) in &mut shaped {
                attr.run = map[attr.run];
                // Shaping saw the paragraph's own slice; the line ranges must
                // index the layer text, so lift the source range.
                attr.source = (
                    attr.source.0 + span.bytes.start,
                    attr.source.1 + span.bytes.start,
                );
            }
            shaped
        };
        apply_advances(&mut items, deltas.glyphs, offset, |g, attrs| {
            let run = runs.get(attrs.run);
            let size = run.and_then(|r| r.size).unwrap_or(style.size);
            let hscale = run.and_then(|r| r.horizontal_scale).unwrap_or(1.0);
            let tracking = run.and_then(|r| r.tracking).unwrap_or(style.tracking) as f32 / 1000.0
                * size
                * hscale;
            let _ = g;
            tracking
        });
        offset += items.len();
        apply_word_spacing(&mut items, deltas.spaces, &mut space_offset);
        let mut soft = vec![None; items.len() + 1];
        if pstyle.hyphenate {
            let hyphen = |attr: &GlyphAttrs| {
                let run = runs.get(attr.run)?;
                let face = match run.font.as_deref() {
                    Some(name) => faces.get(name)?,
                    None => faces.default_face(),
                };
                hyphen_glyph(face, &shape_style_with(run, style))
            };
            insert_hyphens(&items, &mut soft, &pstyle, hyphen);
        }
        paras.push(Para {
            items,
            span: (span.bytes.start, span.bytes.end),
            soft,
            style: pstyle,
        });
    }
    layout_paragraphs(
        &paras,
        style,
        width,
        height,
        origin,
        ascender_ratio,
        deltas.is_empty(),
        align_around_origin,
    )
}

/// Largest cache correction (`Txt2` `/3`, points) Photoshop writes for purely
/// positional adjustments such as the every-line composer's nudges and
/// justification stretches. Optical kerning is an order of magnitude larger:
/// a tight pair is 2-10 % of the em (layer `02`, 26 pt: `T->y` -1.42 pt,
/// `T->space` -2.10 pt = 5.4/8.1 % em), while the composer never moves a glyph
/// by more than ~0.2 pt in this document (layer `05`, 18 pt: max 0.047 pt;
/// layer `10`'s CCGrim runs, 22 pt: max 0.187 pt). The two populations do not
/// overlap, so this threshold picks the optical layers without being keyed to
/// a layer index or a text.
const OPTICAL_CACHE_THRESHOLD: f32 = 0.5;

/// Sets each glyph's advance from Photoshop's composed-cache correction.
///
/// `advances[i]` is CoolType's `/3` value in points: the spacing it applied on
/// top of the advance its spacing model used. Which model that was is not
/// stored in the PSD (`EngineData` carries the same `AutoKerning: True,
/// Kerning: 0` for a metrics layer and for the layer authored with
/// `AutoKerning: OPTICAL`), but the corrections themselves reveal it:
///
/// * optical kerning *replaces* the font's pair kerning and is measured from
///   the nominal advance, so the advance becomes `nominal_advance + delta`
///   (measured on layer `02`: this reproduces Photoshop's stored columns,
///   6 px tighter than the engine's metrics kerning);
/// * composer/tracking corrections *stack* on the engine's spacing model, so
///   the advance stays `advance + delta` (measured on layers `05` and `10`,
///   whose corrections are sub-0.2 pt).
///
/// A short list keeps the engine advance for the remaining glyphs.
fn apply_advances(
    items: &mut [(GlyphPos, GlyphAttrs)],
    advances: &[f32],
    offset: usize,
    tracking: impl Fn(&GlyphPos, &GlyphAttrs) -> f32,
) {
    // Optical kerning is pair-specific; a uniform correction is tracking
    // (layer `03` records its 300/1000 em step as the same 9.9 pt on every
    // glyph), so it must not switch the spacing model.
    let largest = advances.iter().fold(0.0f32, |m, d| m.max(d.abs()));
    let first_nonzero = advances.iter().find(|d| **d != 0.0).copied();
    let varied = first_nonzero.is_some_and(|first| {
        advances
            .iter()
            .any(|d| *d != 0.0 && (*d - first).abs() > 1.0e-4)
    });
    let optical = largest > OPTICAL_CACHE_THRESHOLD && varied;
    for (i, (g, attrs)) in items.iter_mut().enumerate() {
        if let Some(d) = advances.get(offset + i) {
            // The cache value includes CoolType's tracking (layer `03`: every
            // glyph carries its 300/1000 em step scaled by the 150 % horizontal
            // scale) but not its pair kerning, so the tracking step must come
            // out of the base before the correction goes back in.
            let base = if optical {
                g.nominal_advance
            } else {
                g.advance
            };
            g.advance = (base - tracking(g, attrs)).max(0.0) + *d;
        }
    }
}

/// Applies Photoshop's every-line-composer word-spacing corrections.
///
/// `deltas` holds one value per word space of the whole text, in text order;
/// `offset` tracks the spaces consumed by earlier paragraphs. Spaces the
/// composer leaves alone (and text after the list) keep their advance, so an
/// empty or short list is safe.
fn apply_word_spacing(items: &mut [(GlyphPos, GlyphAttrs)], deltas: &[f32], offset: &mut usize) {
    if deltas.is_empty() {
        return;
    }
    for (glyph, _) in items.iter_mut() {
        if glyph.ch != ' ' {
            continue;
        }
        if let Some(delta) = deltas.get(*offset) {
            glyph.advance = (glyph.advance + *delta).max(0.0);
        }
        *offset += 1;
    }
}

/// The text style a run resolves to for advance computation.
fn shape_style_with(run: &RunStyle, fallback: &TextStyle) -> TextStyle {
    TextStyle {
        size: run.size.unwrap_or(fallback.size),
        tracking: run.tracking.unwrap_or(fallback.tracking),
        kerning: fallback.kerning,
        horizontal_scale: run.horizontal_scale.unwrap_or(1.0),
        baseline_shift: run.baseline_shift.unwrap_or(0.0),
        leading: run.leading.or(fallback.leading),
        auto_leading: fallback.auto_leading,
    }
}

/// The hyphen glyph and its advance for a face and style.
fn hyphen_glyph(face: &Face<'_>, style: &TextStyle) -> Option<(u16, f32)> {
    let gid = face.glyph_index('-')?;
    let upm = face.units_per_em() as f32;
    let hscale = style.horizontal_scale;
    let tracking = style.tracking as f32 / 1000.0 * style.size;
    let advance = face.advance(gid) as f32 * style.size / upm * hscale + tracking * hscale;
    Some((gid, advance))
}

/// Records the dictionary's hyphen points as line-end hyphens.
///
/// Only runs of ASCII letters are considered, one glyph per character. That is
/// the common case (Latin words); a ligature or a combining mark splits the run
/// and leaves the word unhyphenated rather than misplacing a hyphen.
fn insert_hyphens<F>(
    items: &[(GlyphPos, GlyphAttrs)],
    soft: &mut [Option<SoftBreak>],
    para: &ParagraphStyle,
    hyphen: F,
) where
    F: Fn(&GlyphAttrs) -> Option<(u16, f32)>,
{
    let Some(dict) = crate::hyphen::default() else {
        return;
    };
    let left = para.pre_hyphen.max(1) as usize;
    let right = para.post_hyphen.max(1) as usize;
    let mut i = 0usize;
    while i < items.len() {
        if !items[i].0.ch.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < items.len() && items[i].0.ch.is_ascii_alphabetic() {
            i += 1;
        }
        let word: String = items[start..i].iter().map(|(g, _)| g.ch).collect();
        let letters = word.chars().count();
        if (letters as i64) <= para.words_longer_than {
            continue;
        }
        for at in dict.breaks(&word, left, right) {
            let k = word[..at].chars().count();
            if k == 0 || k >= letters {
                continue;
            }
            let slot = start + k;
            let (prev, attrs) = items[slot - 1];
            let Some((gid, advance)) = hyphen(&attrs) else {
                continue;
            };
            soft[slot] = Some(SoftBreak {
                gid,
                advance,
                attrs,
                shift: prev.y,
            });
        }
    }
}

/// One paragraph: byte range in the text and UTF-16 range for run lengths.
struct ParagraphSpan {
    bytes: Range<usize>,
    utf16: Range<usize>,
}

/// Splits on `\r`, `\r\n`, `\n` and U+2029. Break characters belong to no
/// paragraph. Always returns at least one span (possibly empty).
fn paragraph_spans(text: &str) -> Vec<ParagraphSpan> {
    let mut out = Vec::new();
    let mut bstart = 0;
    let mut ustart = 0;
    let mut u = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((b, c)) = chars.next() {
        if !matches!(c, '\r' | '\n' | '\u{2029}') {
            u += c.len_utf16();
            continue;
        }
        out.push(ParagraphSpan {
            bytes: bstart..b,
            utf16: ustart..u,
        });
        u += c.len_utf16();
        let mut next = b + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|&(_, n)| n == '\n') {
            let (nb, nc) = chars.next().expect("peeked");
            next = nb + nc.len_utf8();
            u += nc.len_utf16();
        }
        bstart = next;
        ustart = u;
    }
    out.push(ParagraphSpan {
        bytes: bstart..text.len(),
        utf16: ustart..u,
    });
    out
}

/// Runs restricted to a UTF-16 range, with the index of each in the original list.
fn sub_runs(runs: &[RunStyle], range: Range<usize>) -> (Vec<RunStyle>, Vec<usize>) {
    let mut out = Vec::new();
    let mut map = Vec::new();
    let mut off = 0;
    for (i, r) in runs.iter().enumerate() {
        let (r0, r1) = (off, off + r.len);
        off = r1;
        let (lo, hi) = (r0.max(range.start), r1.min(range.end));
        if hi > lo {
            out.push(RunStyle {
                len: hi - lo,
                ..r.clone()
            });
            map.push(i);
        }
    }
    (out, map)
}

/// One glyph as the breaker sees it.
///
/// Photoshop's paragraph sheet stores word, letter and glyph spacing as
/// `[minimum, desired, maximum]` (`docs/txt2-map.md` keys `/5/17`, `/5/18`,
/// `/5/19`). Word spacing is a multiple of the natural space advance (measured:
/// `corpus/box_justify.psd` stores `[0.8 1.0 1.33]` and the justified PDF's
/// spaces are `natural * 2.3047`, `* 0.2595`, `* 0.6634` on the three lines,
/// matching the `/1` adjustment values in the layer's own layout cache).
/// Glyph spacing is a multiple of the advance, letter spacing a fraction of the
/// em; no probe reached the engine with non-default values (the DOM setters in
/// `out/probe5/jobD.json` were no-ops for word/letter spacing, see
/// `docs/txt2-map.md`), so the *units* of letter/glyph spacing are [INFERENCE].
///
/// `justified` makes the desired values the base advance of the piece; the
/// minimum/maximum values become the shrink/stretch capacity the justification
/// distributes slack over. For ragged text the spacing values do not apply
/// (Photoshop's paragraph spacing options act on justified text).
fn piece_of(g: &GlyphPos, para: &ParagraphStyle, size: f32, justified: bool) -> Piece {
    let adv = g.advance.max(0.0);
    let [ws0, ws1, ws2] = para.word_spacing;
    let [ls0, ls1, ls2] = para.letter_spacing;
    let [gs0, gs1, gs2] = para.glyph_spacing;
    if g.ch == ' ' {
        let base = if justified { adv * ws1 } else { adv };
        let (shrink, stretch) = if justified {
            (adv * (ws1 - ws0).max(0.0), adv * (ws2 - ws1).max(0.0))
        } else {
            (0.0, 0.0)
        };
        return Piece {
            ch: g.ch,
            advance: base,
            shrink: shrink.min(base),
            stretch,
        };
    }
    if !justified {
        return Piece {
            ch: g.ch,
            advance: adv,
            shrink: 0.0,
            stretch: 0.0,
        };
    }
    let base = adv * gs1 + size * ls1;
    let shrink = adv * (gs1 - gs0).max(0.0) + size * (ls1 - ls0).max(0.0);
    let stretch = adv * (gs2 - gs1).max(0.0) + size * (ls2 - ls1).max(0.0);
    Piece {
        ch: g.ch,
        advance: base,
        shrink: shrink.min(base),
        stretch,
    }
}

/// The every-line composer's tracking nudge: every inter-letter gap inside a
/// word grows by `0.0104 em`.
///
/// Measured: `out/composer_every.pdf` vs `out/composer_single.pdf` (same text,
/// same box, Noto Sans 24 pt) differ by +0.2496 pt on every glyph whose next
/// glyph is not a space, and the layer's own layout cache
/// (`out/composer_every.psd`) records `/3 .24959` on every line. At 20 pt the
/// same cache records `/3 .208` (`out/c_every.psd`), i.e. `size * 0.0104`.
/// Point text with the same composer shows no nudge
/// (`out/probe5/p_composer_every.pdf`), so this is applied to area paragraphs
/// that actually break into more than one line. [INFERENCE] for a single-line
/// area paragraph (unmeasured).
fn apply_every_line_tracking(items: &mut [(GlyphPos, GlyphAttrs)], size: f32) {
    let nudge = typography::every_line_nudge(size);
    for i in 0..items.len().saturating_sub(1) {
        if items[i].0.ch != ' ' && items[i + 1].0.ch != ' ' {
            items[i].0.advance += nudge;
        }
    }
}

/// Shared inputs of [`place_line`].
struct Ctx<'a> {
    para: &'a ParagraphStyle,
    /// Index of the paragraph being placed, in text order.
    paragraph: usize,
    breaker: &'a Breaker,
    origin_x: f32,
}

/// Places paragraphs stacked from the box top. Each paragraph, empty or not,
/// steps one leading; lines whose baseline falls outside the box are dropped.
///
/// Paragraph geometry, measured against Photoshop's own PDF exports:
/// the first paragraph's first baseline sits `first_baseline_offset` below the
/// box top (space-before does **not** move the first line of the frame); every
/// following paragraph's first baseline steps down by the *next* paragraph's
/// leading plus `space_after` of the previous paragraph and `space_before` of
/// the next one. `10_all_features.psd` layer `05 area indents spacing`
/// (first paragraph auto leading 1.5 at 18 pt, second 1.2, space after 18,
/// space before 12 on the first paragraph only) puts the second paragraph's
/// first baseline 39.6 pt below the last line of the first: 21.6 (next
/// leading) + 18 + 0. `out/psd_showcase/03_area_paragraphs.pdf` has the same
/// values in both sheets and steps 33 + 18 + 12 = 63.
///
/// The first line of a paragraph is offset by
/// `start_indent + first_line_indent`, later lines by `start_indent` (measured:
/// `out/exp/area_indent.pdf` puts both paragraph first lines of
/// `area_indent.psd`, which stores `/5/1 24`, `/5/2 18`, `/5/3 12`, at
/// `x = 19.998 + 42` with a 430 pt box; Photoshop's own recomposition in
/// `out/exp/area_indent_resaved.psd` has the same glyph-run origin 42.0).
/// `end_indent` narrows the measure of every line the same way (the sum is the
/// only indentation model in the Txt2 sheet).
#[allow(clippy::too_many_arguments)]
fn layout_paragraphs(
    paras: &[Para],
    style: &TextStyle,
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
    uniform_every_line_nudge: bool,
    align_around_origin: bool,
) -> AreaLayout {
    let first_off = first_baseline_offset(ascender_ratio, style.size);
    let mut out = AreaLayout::default();
    let mut baseline = origin.1 + first_off;
    let mut from_top = first_off;
    let mut prev: Option<ParagraphStyle> = None;
    for (pi, para) in paras.iter().enumerate() {
        let pstyle = para.style;
        if pi > 0 {
            let lead = para
                .items
                .first()
                .map(|(_, a)| a.leading)
                .unwrap_or_else(|| style.size * pstyle.auto_leading);
            let gap = lead + prev.map(|p| p.space_after).unwrap_or(0.0) + pstyle.space_before;
            baseline -= gap;
            from_top += gap;
        }
        prev = Some(pstyle);
        if para.items.is_empty() {
            let lead = style.size * pstyle.auto_leading;
            out.empty.push(EmptyParagraph {
                paragraph: pi,
                range_start: para.span.0,
                range_end: para.span.1,
                baseline,
                x: origin.0 + pstyle.start_indent + pstyle.first_line_indent,
                leading: lead,
            });
            baseline -= lead;
            from_top += lead;
            continue;
        }
        let base_avail = width - pstyle.start_indent - pstyle.end_indent;
        let avail = [base_avail - pstyle.first_line_indent, base_avail];
        let justify = pstyle.justification.is_justify();
        let mut work = para.items.clone();
        let soft = para.soft.clone();
        let pieces = Pieces::new(
            work.iter()
                .map(|(g, _)| piece_of(g, &pstyle, style.size, justify))
                .collect(),
        );
        let hyphens: Vec<Option<f32>> = soft.iter().map(|s| s.map(|b| b.advance)).collect();
        let legal = crate::composer::legal_breaks(&pieces, avail[0].max(avail[1]), &hyphens);
        let breaker =
            Breaker::with_hyphens(avail, justify, pstyle.hanging, hyphens).with_legal_breaks(legal);
        // `EveryLineComposer` true selects the multi-line (paragraph)
        // composer; the default single-line composer fits each line on its own.
        // Measured on `10_all_features.psd` layer `04 area justify full
        // hanging` (EveryLineComposer false, hanging): greedy breaks are exactly
        // Photoshop's 45/35/35/44/44/28 glyph lines, while the multi-line
        // optimiser chooses a different first line.
        let forced = std::env::var("PHOTOCRAFT_COMPOSER").ok();
        let greedy = match forced.as_deref() {
            Some("greedy") => true,
            Some("optimal") => false,
            _ => !pstyle.every_line_composer,
        };
        let ends = if greedy {
            breaker.greedy(&pieces)
        } else {
            let count = breaker.greedy(&pieces).len();
            breaker.optimal(&pieces, count)
        };
        // The measured every-line tracking nudge is applied after the breaks
        // were chosen from the true metrics; it is small enough not to change
        // Photoshop's own break decisions (out/composer_every.pdf breaks match
        // the unnudged layout, and the advances match the nudged one).
        if uniform_every_line_nudge && pstyle.every_line_composer && ends.len() > 1 {
            apply_every_line_tracking(&mut work, style.size);
        }
        let pieces = Pieces::new(
            work.iter()
                .map(|(g, _)| piece_of(g, &pstyle, style.size, justify))
                .collect(),
        );
        let ctx = Ctx {
            para: &pstyle,
            paragraph: pi,
            breaker: &breaker,
            origin_x: origin.0,
        };
        let mut start = 0;
        for (k, &end) in ends.iter().enumerate() {
            let line = place_line(
                &ctx,
                &pieces,
                &work,
                &soft,
                (start, end),
                k,
                baseline,
                align_around_origin,
            );
            // Photoshop hides lines whose baseline falls outside the box.
            if from_top <= height {
                out.lines.push(line);
            }
            start = end;
            if k + 1 < ends.len() {
                let next_leading = work
                    .get(end)
                    .map(|(_, attrs)| attrs.leading)
                    .unwrap_or_else(|| style.size * pstyle.auto_leading);
                baseline -= next_leading;
                from_top += next_leading;
            }
        }
    }
    out
}

/// Places line `range` (paragraph line `k`) with its alignment, justification
/// and hanging punctuation.
///
/// Hanging model (measured, `docs/hanging-punctuation.md`): a leading run of
/// left-hang glyphs is drawn shifted left so the glyph after it starts on the
/// line's margin (a half-hanging glyph keeps half its advance in the flow); a
/// trailing run of right-hang glyphs is drawn after the fitted content, so it
/// overflows the right margin. Justified lines distribute the slack over the
/// non-hanging content only; the hanging glyphs are never stretched (measured:
/// `out/probe5/hang4.pdf` keeps the comma advance 9.648 on justified lines).
#[allow(clippy::too_many_arguments)]
fn place_line(
    ctx: &Ctx<'_>,
    pieces: &Pieces,
    items: &[(GlyphPos, GlyphAttrs)],
    soft: &[Option<SoftBreak>],
    (start, end): (usize, usize),
    k: usize,
    baseline: f32,
    align_around_origin: bool,
) -> Line {
    let para = ctx.para;
    let breaker = ctx.breaker;
    let avail = breaker.avail_for(k) as f32;
    let ext: Extent = breaker.extent(pieces, start, end);
    let content_w = ext.width as f32;
    let is_last = end == items.len();
    let justify_line =
        breaker.justify && (!is_last || para.justification == Justification::JustifyAll);

    // Desired spacing values may move the base advance away from the shaped one;
    // justification then distributes the remaining slack over the pieces'
    // min/max capacity.
    let mut deltas: Vec<f32> = (start..end)
        .map(|i| pieces.items[i].advance - items[i].0.advance)
        .collect();
    if justify_line && ext.end > ext.start {
        let slack = breaker.distribute(pieces, start, end, k);
        for (d, s) in deltas.iter_mut().zip(slack) {
            *d += s;
        }
    }

    // Left-hang prefix: shift all prefix glyphs left by the width they hang, so
    // the rest of the line starts where the fit test measured it.
    let hang_left: f32 = items[start..ext.start]
        .iter()
        .map(|(g, _)| left_hang_shift(g.ch, g.advance))
        .sum();

    let indent = para.start_indent + if k == 0 { para.first_line_indent } else { 0.0 };
    let mut margin = ctx.origin_x + indent;
    // A stretched line fills the measure, so its origin is the left edge; the
    // remaining cases align the measured (fitted) width as a whole.
    let align = if justify_line {
        Justification::Left
    } else {
        match para.justification {
            Justification::JustifyLastCenter => Justification::Center,
            Justification::JustifyLastRight => Justification::Right,
            other => other,
        }
    };
    if align_around_origin {
        // Point text has no frame: Photoshop aligns each line against the type
        // origin (the click), not against the widest line of the block
        // (measured on CC 2019, "Hello world" 30 pt at x=150: left 152..292,
        // center 80..220, right 8..148 — each line ends/centres on 150).
        match align {
            Justification::Right => margin -= content_w,
            Justification::Center => margin -= content_w / 2.0,
            _ => {}
        }
    } else {
        match align {
            Justification::Right => margin += avail - content_w,
            Justification::Center => margin += (avail - content_w) / 2.0,
            _ => {}
        }
    }

    let x0 = margin - hang_left;
    let mut pen = x0;
    let mut glyphs = Vec::with_capacity(end - start);
    let mut attrs = Vec::with_capacity(end - start);
    for (i, (g, a)) in items[start..end].iter().enumerate() {
        let advance = g.advance + deltas[i];
        // Store the advance actually used for the pen walk: justified and
        // spacing-adjusted lines move letters and spaces, and consumers that
        // measure a line by `x + advance` (layer bounds, PDF dumps) must see the
        // drawn advance, not the shaped metric one.
        let mut placed = GlyphPos {
            x: pen,
            y: baseline + g.y,
            ..*g
        };
        placed.advance = advance;
        glyphs.push(placed);
        attrs.push(*a);
        pen += advance;
    }
    // A hyphenation hyphen is drawn only on the line that actually breaks at
    // its point; `extent` added the same advance to this line's fit test.
    if let Some(h) = soft.get(end).copied().flatten() {
        let placed = GlyphPos {
            gid: h.gid,
            ch: '-',
            x: pen,
            y: baseline + h.shift,
            advance: h.advance,
            nominal_advance: h.advance,
        };
        glyphs.push(placed);
        attrs.push(h.attrs);
        pen += h.advance;
    }
    // The line's source bytes: the glyphs are in text order, so the first
    // glyph's start and the last glyph's end bound them (a hyphenation hyphen
    // copies the preceding glyph's range and cannot extend the line).
    let range_start = attrs.iter().map(|a| a.source.0).min().unwrap_or(0);
    let range_end = attrs.iter().map(|a| a.source.1).max().unwrap_or(0);
    Line {
        glyphs,
        baseline,
        width: pen - x0,
        attrs,
        range_start,
        range_end,
        paragraph: ctx.paragraph,
    }
}

#[cfg(test)]
mod cache_advance_tests {
    use super::*;

    fn glyph(advance: f32, nominal: f32) -> (GlyphPos, GlyphAttrs) {
        (
            GlyphPos {
                gid: 1,
                ch: 'A',
                x: 0.0,
                y: 0.0,
                advance,
                nominal_advance: nominal,
            },
            GlyphAttrs {
                run: 0,
                color: [0, 0, 0],
                vertical_scale: 1.0,
                source: (0, 0),
                x_scale: 1.0,
                faux_italic: false,
                rotate: false,
                leading: 0.0,
            },
        )
    }

    /// Optical kerning replaces the font's pair kerning, so the correction is
    /// applied to the nominal advance (`nominal_advance + delta`), not on top
    /// of the engine's metrics kerning. Measured on layer `02`: this is what
    /// reproduces Photoshop's stored columns.
    #[test]
    fn word_spacing_corrections_move_only_the_spaces() {
        let mut items = [glyph(10.0, 10.0), glyph(4.0, 4.0), glyph(10.0, 10.0)];
        items[0].0.ch = 'a';
        items[1].0.ch = ' ';
        items[2].0.ch = 'b';
        let mut offset = 0usize;
        apply_word_spacing(&mut items, &[-0.5, 1.0], &mut offset);
        assert!((items[0].0.advance - 10.0).abs() < 1e-6);
        assert!((items[1].0.advance - 3.5).abs() < 1e-6);
        assert!((items[2].0.advance - 10.0).abs() < 1e-6);
        // The text has one space, so one value was consumed.
        assert_eq!(offset, 1);
        // The offset keeps counting spaces across paragraphs: the next space
        // takes the second correction.
        let mut more = [glyph(4.0, 4.0)];
        more[0].0.ch = ' ';
        apply_word_spacing(&mut more, &[-0.5, 1.0], &mut offset);
        assert!((more[0].0.advance - 5.0).abs() < 1e-6);
        assert_eq!(offset, 2);
        // Past the end of the list the spaces keep their advance.
        let mut after = [glyph(4.0, 4.0)];
        after[0].0.ch = ' ';
        apply_word_spacing(&mut after, &[-0.5, 1.0], &mut offset);
        assert!((after[0].0.advance - 4.0).abs() < 1e-6);
        assert_eq!(offset, 3);
        // An empty sheet is a no-op.
        let mut untouched = [glyph(4.0, 4.0)];
        untouched[0].0.ch = ' ';
        let mut zero = 0usize;
        apply_word_spacing(&mut untouched, &[], &mut zero);
        assert!((untouched[0].0.advance - 4.0).abs() < 1e-6);
        assert_eq!(zero, 0);
    }

    /// The every-line composer's own nudge is skipped when the cache supplies
    /// word-spacing corrections (the cache carries the real ones).
    #[test]
    fn cache_corrections_disable_the_uniform_every_line_nudge() {
        let deltas = CacheDeltas {
            glyphs: &[],
            spaces: &[-0.0468],
        };
        assert!(!deltas.is_empty());
        let empty = CacheDeltas::default();
        assert!(empty.is_empty());
    }

    #[test]
    fn pair_specific_large_corrections_replace_kerning() {
        let mut items = [glyph(15.574, 16.614)];
        apply_advances(&mut items, &[-1.1622, 0.1768], 0, |_, _| 0.0);
        assert!((items[0].0.advance - (16.614 - 1.1622)).abs() < 1e-3);
    }

    /// A uniform correction is CoolType's tracking (layer `03` records the
    /// same 9.9 pt on every glyph); the engine's tracking is subtracted before
    /// the correction goes back in, so the advance is unchanged.
    #[test]
    fn uniform_corrections_are_tracking_and_leave_the_advance() {
        let mut items = [glyph(26.598, 28.248)];
        apply_advances(&mut items, &[9.9, 9.9], 0, |_, _| 9.9);
        assert!((items[0].0.advance - 26.598).abs() < 1e-3);
    }

    /// Composer nudges stack on the engine's own spacing model.
    #[test]
    fn small_corrections_stack_on_the_engine_advance() {
        let mut items = [glyph(16.614, 16.614)];
        apply_advances(&mut items, &[-0.047], 0, |_, _| 0.0);
        assert!((items[0].0.advance - (16.614 - 0.047)).abs() < 1e-4);
    }

    /// Photoshop records its tracking in the correction, so an all-zero cache
    /// is "untracked metrics": the engine's tracking step must come out.
    #[test]
    fn all_zero_corrections_drop_the_engine_tracking() {
        let mut items = [glyph(26.598, 28.248)];
        apply_advances(&mut items, &[0.0, 0.0], 0, |_, _| 9.9);
        assert!((items[0].0.advance - (26.598 - 9.9)).abs() < 1e-6);
    }
}
