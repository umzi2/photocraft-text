//! Area (paragraph) text: paragraph splitting, word wrapping and justification.

use std::ops::Range;

use crate::composer::{Breaker, Extent, Piece, Pieces};
use crate::font::Face;
use crate::layout::{shape, GlyphPos, TextStyle};
use crate::runs::{shape_runs_attrs, FontSet, GlyphAttrs, RunStyle};
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
            Self::JustifyLastLeft | Self::JustifyLastCenter | Self::JustifyLastRight | Self::JustifyAll
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
            3 => Self::JustifyLastCenter,
            4 => Self::JustifyLastRight,
            5 => Self::JustifyAll,
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
}

/// Result of laying out one paragraph inside a box.
#[derive(Clone, Debug, Default)]
pub struct AreaLayout {
    pub lines: Vec<Line>,
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
    let style = &TextStyle { auto_leading: para.auto_leading, ..*style };
    let plain = GlyphAttrs {
        run: 0,
        color: [0; 3],
        vertical_scale: 1.0,
        x_scale: 1.0,
        faux_italic: false,
        leading: style.leading_pt(),
    };
    let paras: Vec<Vec<(GlyphPos, GlyphAttrs)>> = paragraph_spans(text)
        .into_iter()
        .map(|span| {
            let slice = &text[span.bytes];
            if slice.is_empty() {
                return Vec::new();
            }
            shape(face, slice, style).into_iter().map(|g| (g, plain)).collect()
        })
        .collect();
    layout_paragraphs(&paras, style, para, width, height, origin, ascender_ratio)
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
    // Auto leading is a paragraph property, so the shaping pass needs it on the
    // style it resolves each run against.
    let style = &TextStyle { auto_leading: para.auto_leading, ..*style };
    let paras: Vec<Vec<(GlyphPos, GlyphAttrs)>> = paragraph_spans(text)
        .into_iter()
        .map(|span| {
            let slice = &text[span.bytes];
            if slice.is_empty() {
                return Vec::new();
            }
            let (sub, map) = sub_runs(runs, span.utf16);
            let mut glyphs = shape_runs_attrs(faces, slice, &sub, style);
            for (_, attr) in &mut glyphs {
                attr.run = map[attr.run];
            }
            glyphs
        })
        .collect();
    layout_paragraphs(&paras, style, para, width, height, origin, ascender_ratio)
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
        out.push(ParagraphSpan { bytes: bstart..b, utf16: ustart..u });
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
    out.push(ParagraphSpan { bytes: bstart..text.len(), utf16: ustart..u });
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
            out.push(RunStyle { len: hi - lo, ..r.clone() });
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
        return Piece { ch: g.ch, advance: base, shrink: shrink.min(base), stretch };
    }
    if !justified {
        return Piece { ch: g.ch, advance: adv, shrink: 0.0, stretch: 0.0 };
    }
    let base = adv * gs1 + size * ls1;
    let shrink = adv * (gs1 - gs0).max(0.0) + size * (ls1 - ls0).max(0.0);
    let stretch = adv * (gs2 - gs1).max(0.0) + size * (ls2 - ls1).max(0.0);
    Piece { ch: g.ch, advance: base, shrink: shrink.min(base), stretch }
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
    breaker: &'a Breaker,
    origin_x: f32,
}

/// Places paragraphs stacked from the box top. Each paragraph, empty or not,
/// steps one leading; lines whose baseline falls outside the box are dropped.
///
/// Paragraph geometry: the first line of a paragraph is offset by
/// `start_indent + first_line_indent`, later lines by `start_indent` (measured:
/// `out/exp/area_indent.pdf` puts both paragraph first lines of
/// `area_indent.psd`, which stores `/5/1 24`, `/5/2 18`, `/5/3 12`, at
/// `x = 19.998 + 42` with a 430 pt box; Photoshop's own recomposition in
/// `out/exp/area_indent_resaved.psd` has the same glyph-run origin 42.0).
/// `end_indent` narrows the measure of every line the same way (the sum is the
/// only indentation model in the Txt2 sheet; no corpus layer wraps against a
/// right indent, so its effect is unmeasured — `docs/txt2-map.md` marks
/// `/5/3` NEEDS PROBE).
///
/// Space before/after: `space_after` of a paragraph plus `space_before` of the
/// next are added to the one leading that already separates the paragraphs.
/// [INFERENCE]: every corpus paragraph has `SpaceBefore = SpaceAfter = 0`
/// (measured) and the CC 2019 DOM setters are no-ops (`out/probe5/mD.json`,
/// `docs/txt2-map.md`), so no Photoshop artefact exercises these values.
fn layout_paragraphs(
    paras: &[Vec<(GlyphPos, GlyphAttrs)>],
    style: &TextStyle,
    para: &ParagraphStyle,
    width: f32,
    height: f32,
    origin: (f32, f32),
    ascender_ratio: f32,
) -> AreaLayout {
    let first_off = first_baseline_offset(ascender_ratio, style.size);
    let base_avail = width - para.start_indent - para.end_indent;
    let justify = para.justification.is_justify();
    let mut out = AreaLayout::default();
    // Baselines step by the leading of the line that follows: a line's own runs
    // decide the distance to it, which is how Photoshop stacks mixed sizes.
    let mut baseline = origin.1 + first_off + para.space_before;
    let mut from_top = first_off + para.space_before;
    let mut seen_paragraph = false;
    for items in paras {
        if items.is_empty() {
            baseline -= style.leading_pt();
            from_top += style.leading_pt();
            continue;
        }
        if seen_paragraph {
            let gap = para.space_after + para.space_before;
            baseline -= gap;
            from_top += gap;
        }
        seen_paragraph = true;
        let mut work: Vec<(GlyphPos, GlyphAttrs)> = items.clone();
        let avail = [base_avail - para.first_line_indent, base_avail];
        let pieces = Pieces::new(work.iter().map(|(g, _)| piece_of(g, para, style.size, justify)).collect());
        let breaker = Breaker::new(avail, justify, para.hanging);
        let ends = if para.every_line_composer {
            let count = breaker.greedy(&pieces).len();
            breaker.optimal(&pieces, count)
        } else {
            breaker.greedy(&pieces)
        };
        // The measured every-line tracking nudge is applied after the breaks
        // were chosen from the true metrics; it is small enough not to change
        // Photoshop's own break decisions (out/composer_every.pdf breaks match
        // the unnudged layout, and the advances match the nudged one).
        if para.every_line_composer && ends.len() > 1 {
            apply_every_line_tracking(&mut work, style.size);
        }
        let pieces = Pieces::new(work.iter().map(|(g, _)| piece_of(g, para, style.size, justify)).collect());
        let ctx = Ctx { para, breaker: &breaker, origin_x: origin.0 };
        let mut start = 0;
        for (k, &end) in ends.iter().enumerate() {
            let line = place_line(&ctx, &pieces, &work, (start, end), k, baseline);
            // Photoshop hides lines whose baseline falls outside the box.
            if from_top <= height {
                out.lines.push(line);
            }
            start = end;
            let next_leading = work
                .get(end)
                .map(|(_, attrs)| attrs.leading)
                .unwrap_or_else(|| work.first().map(|(_, a)| a.leading).unwrap_or(style.leading_pt()));
            baseline -= next_leading;
            from_top += next_leading;
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
fn place_line(
    ctx: &Ctx<'_>,
    pieces: &Pieces,
    items: &[(GlyphPos, GlyphAttrs)],
    (start, end): (usize, usize),
    k: usize,
    baseline: f32,
) -> Line {
    let para = ctx.para;
    let breaker = ctx.breaker;
    let avail = breaker.avail_for(k) as f32;
    let ext: Extent = breaker.extent(pieces, start, end);
    let content_w = ext.width as f32;
    let is_last = end == items.len();
    let justify_line = breaker.justify && (!is_last || para.justification == Justification::JustifyAll);

    // Desired spacing values may move the base advance away from the shaped one;
    // justification then distributes the remaining slack over the pieces'
    // min/max capacity.
    let mut deltas: Vec<f32> = (start..end).map(|i| pieces.items[i].advance - items[i].0.advance).collect();
    if justify_line && ext.end > ext.start {
        let slack = breaker.distribute(pieces, start, end, k);
        for (d, s) in deltas.iter_mut().zip(slack) {
            *d += s;
        }
    }

    // Left-hang prefix: shift all prefix glyphs left by the width they hang, so
    // the rest of the line starts where the fit test measured it.
    let hang_left: f32 = items[start..ext.start].iter().map(|(g, _)| left_hang_shift(g.ch, g.advance)).sum();

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
    match align {
        Justification::Right => margin += avail - content_w,
        Justification::Center => margin += (avail - content_w) / 2.0,
        _ => {}
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
        let mut placed = GlyphPos { x: pen, y: baseline + g.y, ..*g };
        placed.advance = advance;
        glyphs.push(placed);
        attrs.push(*a);
        pen += advance;
    }
    Line { glyphs, baseline, width: pen - x0, attrs }
}
