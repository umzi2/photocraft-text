//! Area (paragraph) text: paragraph splitting, word wrapping and justification.

use std::ops::Range;

use crate::composer::{Breaker, Extent, Pieces, Piece};
use crate::font::Face;
use crate::layout::{shape, GlyphPos, TextStyle};
use crate::runs::{shape_runs_attrs, FontSet, GlyphAttrs, RunStyle};
use crate::typography::{hanging_adjust, Kinsoku};

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
    pub fn from_psd(value: i32) -> Self {
        match value {
            1 => Self::Right,
            2 => Self::Center,
            3 => Self::JustifyLastCenter,
            4 => Self::JustifyLastRight,
            5 => Self::JustifyAll,
            6 => Self::JustifyLastLeft,
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
/// added to the box top.
///
/// It is not the `hhea` ascender: measured against Photoshop 20.0.2, CC Wild But
/// Not Crazy places the first baseline at 0.768 x size while its `hhea` ascender
/// is 0.838. The exact rule is still unknown (it lives in `Composer@PSSLO`), so
/// known fonts are calibrated here and everything else falls back to `hhea`.
pub fn first_baseline_ratio(face: &Face<'_>) -> f32 {
    // Measured from Photoshop's PDF exports (`tools/measure_baseline.py`):
    // the offset is exactly `ratio * size` and constant across sizes.
    // The metric behind the ratio is still unknown — it is not `hhea`, OS/2
    // `typoAscender`, `usWinAscent` or cap height for every font — so known
    // fonts are tabulated and the rest fall back to `hhea`.
    const CALIBRATED: [(&str, f32); 4] = [
        ("vCCWildButNotCrazy", 0.768),
        ("NotoSans", 0.76),
        ("NotoSerif", 0.76),
        ("Arial", 0.716),
    ];
    // Family names come with spaces ("Noto Sans"), so compare with spaces and
    // separators stripped.
    let key = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    if let Some(name) = face.family_name() {
        let name = key(&name);
        for (family, ratio) in CALIBRATED {
            if name == key(family) {
                return ratio;
            }
        }
    }
    let upm = face.units_per_em() as f32;
    face.table(*b"hhea")
        .map(|h| i16::from_be_bytes([h[4], h[5]]) as f32 / upm)
        .unwrap_or(0.8)
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

fn piece_of(g: &GlyphPos, para: &ParagraphStyle, size: f32) -> Piece {
    let adv = g.advance.max(0.0);
    let [ws0, ws1, ws2] = para.word_spacing;
    let [ls0, ls1, ls2] = para.letter_spacing;
    let [gs0, gs1, gs2] = para.glyph_spacing;
    // [INFERENCE] glyph spacing is a fraction of the advance; letter spacing is
    // a fraction of the em.
    let (shrink, stretch) = if g.ch == ' ' {
        (adv * (ws1 - ws0).max(0.0), adv * (ws2 - ws1).max(0.0))
    } else {
        (
            adv * (gs1 - gs0).max(0.0) + size * (ls1 - ls0).max(0.0),
            adv * (gs2 - gs1).max(0.0) + size * (ls2 - ls1).max(0.0),
        )
    };
    Piece { ch: g.ch, advance: g.advance, shrink: shrink.min(adv), stretch }
}

/// Shared inputs of [`place_line`].
struct Ctx<'a, 'b> {
    para: &'a ParagraphStyle,
    breaker: &'a Breaker<'b>,
    origin_x: f32,
}

/// Places paragraphs stacked from the box top. Each paragraph, empty or not,
/// steps one leading; lines whose baseline falls outside the box are dropped.
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
    let kinsoku = para.hanging.then(|| Kinsoku { enabled: true, ..Kinsoku::default() });
    let justify = para.justification.is_justify();
    let base_avail = width - para.start_indent - para.end_indent;
    let mut out = AreaLayout::default();
    // Baselines step by the leading of the line that follows: a line's own runs
    // decide the distance to it, which is how Photoshop stacks mixed sizes.
    let mut baseline = origin.1 + first_off;
    let mut from_top = first_off;
    for items in paras {
        if items.is_empty() {
            baseline -= style.leading_pt();
            from_top += style.leading_pt();
            continue;
        }
        let pieces = Pieces::new(items.iter().map(|(g, _)| piece_of(g, para, style.size)).collect());
        let breaker = Breaker {
            avail: [base_avail - para.first_line_indent, base_avail],
            justify,
            hang: kinsoku.as_ref(),
        };
        let ends = if para.every_line_composer {
            let count = breaker.greedy(&pieces).len();
            breaker.optimal(&pieces, count)
        } else {
            breaker.greedy(&pieces)
        };
        let ctx = Ctx { para, breaker: &breaker, origin_x: origin.0 };
        let mut start = 0;
        for (k, &end) in ends.iter().enumerate() {
            let line = place_line(&ctx, &pieces, items, (start, end), k, baseline);
            // Photoshop hides lines whose baseline falls outside the box.
            if from_top <= height {
                out.lines.push(line);
            }
            start = end;
            let next_leading = items
                .get(end)
                .map(|(_, attrs)| attrs.leading)
                .unwrap_or_else(|| items.first().map(|(_, a)| a.leading).unwrap_or(style.leading_pt()));
            baseline -= next_leading;
            from_top += next_leading;
        }
    }
    out
}

/// Places line `range` (paragraph line `k`) with its alignment, justification
/// and hanging shift.
fn place_line(
    ctx: &Ctx<'_, '_>,
    pieces: &Pieces,
    items: &[(GlyphPos, GlyphAttrs)],
    (start, end): (usize, usize),
    k: usize,
    baseline: f32,
) -> Line {
    let para = ctx.para;
    let breaker = ctx.breaker;
    let avail = breaker.avail_for(k) as f32;
    let ext: Extent = breaker.extent(pieces, start, end, k);
    let content_w = ext.width as f32;
    let is_last = end == items.len();

    let mut deltas = vec![0f32; end - start];
    // `JustifyAll` stretches the last line too; the other justify variants
    // leave it aligned per their last-line rule.
    if breaker.justify && (!is_last || para.justification == Justification::JustifyAll) {
        if para.every_line_composer {
            deltas = breaker.distribute(pieces, start, end, k);
        } else {
            // Photoshop distributes the slack over the interior spaces.
            let content = &items[start..ext.end];
            let gaps = content.iter().filter(|(g, _)| g.ch == ' ').count();
            if gaps > 0 {
                let extra = (avail - content_w) / gaps as f32;
                for (d, (g, _)) in deltas.iter_mut().zip(content) {
                    if g.ch == ' ' {
                        *d = extra;
                    }
                }
            }
        }
    }

    let indent = para.start_indent + if k == 0 { para.first_line_indent } else { 0.0 };
    let mut x = ctx.origin_x + indent;
    // A stretched line fills the width, so its origin is the left edge; the
    // remaining cases align the line as a whole.
    let align = if breaker.justify && !is_last {
        Justification::Left
    } else {
        match para.justification {
            Justification::JustifyLastCenter => Justification::Center,
            Justification::JustifyLastRight => Justification::Right,
            other => other,
        }
    };
    match align {
        Justification::Right => x += avail - content_w,
        Justification::Center => x += (avail - content_w) / 2.0,
        _ => {}
    }

    let adjusted: Vec<(GlyphPos, GlyphAttrs)> = items[start..end]
        .iter()
        .zip(&deltas)
        .map(|((g, a), d)| (GlyphPos { advance: g.advance + d, ..*g }, *a))
        .collect();
    if let Some(kin) = breaker.hang {
        let content: Vec<GlyphPos> = adjusted[..ext.end - start].iter().map(|(g, _)| *g).collect();
        x += hanging_adjust(&content, avail, kin);
    }

    let x0 = x;
    let mut pen = x;
    let mut glyphs = Vec::with_capacity(adjusted.len());
    let mut attrs = Vec::with_capacity(adjusted.len());
    for (g, a) in adjusted {
        glyphs.push(GlyphPos { x: pen, y: baseline + g.y, ..g });
        attrs.push(a);
        pen += g.advance;
    }
    Line { glyphs, baseline, width: pen - x0, attrs }
}
