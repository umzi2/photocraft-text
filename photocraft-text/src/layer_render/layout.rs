//! The layout a rendered text layer was rasterised from.
//!
//! [`TextLayout`] is not a second layout engine: it is a view of the glyph
//! placement [`super::paint`] consumed for the layer's pixels. The entry
//! points in [`super`] (`layout_from_engine_data*`, `layout_vertical_from_engine_data*`)
//! run the same `prepare` stage as `render_from_engine_data*` and stop before
//! rasterisation, so an editor's caret cannot drift from what is drawn.
//!
//! # Coordinates
//!
//! Horizontal text is reported in the space the renderer draws in:
//!
//! * `x` is the pen x of the line, the same number `paint` adds to the canvas
//!   column (points, rightwards, including the layout origin and the paragraph
//!   indents);
//! * `y` is measured downwards from the layout's **first baseline**, the same
//!   reference [`super::LayerPixels::offset`] uses, so
//!   `y = first_baseline - glyph.y` for the engine's y-up baseline. The first
//!   line's baseline is therefore `y = 0` and a glyph inside the box sits at a
//!   positive `y`. Above the first baseline (box top, accents) `y` is negative.
//!
//! `anchor` is the document position of the layout origin for the in-rect
//! entry points; the document position of a point is `anchor + (x, y)`
//! (`anchor` is `(0, 0)` for the non-rect entry points).
//!
//! Vertical text keeps `y` as the down-the-column coordinate (reading
//! direction) and `x` across the column; [`TextLayout::vertical`] is `true`.
//! Caret geometry follows: a horizontal caret is a vertical bar at `x`
//! spanning the line's ascent/descent, a vertical caret is a horizontal bar at
//! `y` spanning the column.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::area::{EmptyParagraph, Line};
use crate::font::Face;

use super::horizontal::Prepared;
use super::vertical::PreparedVertical;

/// A glyph the renderer drew, with the layer text it renders.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedGlyph {
    /// Byte range of the layer text behind the glyph. A ligature covers all its
    /// components; glyphs added by the layout itself (a hyphenation hyphen,
    /// caps-expanded letters) share or cover their source character's range.
    pub range: Range<usize>,
    /// Glyph id in the run's face.
    pub gid: u16,
    /// Index into the layer's style runs (`EngineData` `StyleRun`).
    pub run: usize,
    /// Pen x of the glyph origin, points, rightwards.
    pub x: f32,
    /// Pen y, points, downwards from the layout's first baseline.
    pub y: f32,
    /// Distance to the next glyph on the line, points (includes justification).
    pub advance: f32,
    /// Fill colour of the run.
    pub color: [u8; 3],
}

/// One laid-out line.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// Bytes of the layer text the line draws.
    ///
    /// Paragraph separators are **not** part of a line: the break byte between
    /// two lines belongs to neither, and [`TextLayout::caret`] on it draws at
    /// the end of this line. Empty paragraphs keep their separator (it is their
    /// only byte). Consecutive lines therefore need not cover the text exactly;
    /// separators are line breaks, not text.
    pub range: Range<usize>,
    /// Baseline y, points, downwards from the layout's first baseline
    /// (`0.0` on the first line).
    pub baseline: f32,
    /// Left edge of the line's ink: the first glyph's pen x (with the paragraph
    /// indents and hanging punctuation applied), or the paragraph's caret x.
    pub x0: f32,
    /// Right edge of the line's ink: the last glyph's pen plus its advance.
    pub x1: f32,
    /// End of the line along its inline axis, exclusive. For horizontal text
    /// this equals `x1`; for vertical text it is the next position down the
    /// column (one em after the last glyph).
    pub end: f32,
    /// Distance from the baseline up to the line's tallest ascent, points.
    pub ascent: f32,
    /// Distance from the baseline down to the line's deepest descent, points.
    pub descent: f32,
    /// Index of the paragraph the line belongs to, in text order.
    pub paragraph: usize,
}

/// A caret stop: one grapheme cluster of the layer text.
///
/// Paragraph separators have no stop — a break byte maps to the end of the line
/// before it. A ligature contributes one stop per component; the first owns the
/// ligature's advance and the following ones are zero-advance stops at its end.
#[derive(Clone, Debug, PartialEq)]
pub struct CaretCluster {
    /// Byte range of the grapheme cluster in the layer text.
    pub range: Range<usize>,
    /// Index into [`TextLayout::lines`].
    pub line: usize,
    /// Position along the line's inline axis: `x` for horizontal text (points,
    /// rightwards), `y` for vertical text (points, downwards). A character the
    /// face has no glyph for still gets a stop here (a zero-advance cluster).
    pub x: f32,
    /// Distance to the next caret stop on the same line, points.
    pub advance: f32,
    /// Right-to-left cluster. The engine lays out single-direction runs only,
    /// so this is always `false` today; it is part of the contract for the
    /// caller's bidi handling.
    pub rtl: bool,
}

/// Axis-aligned rectangle in the layout's coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
}

/// A caret rectangle with the line it belongs to.
///
/// Horizontal text: a zero-width vertical bar (`x0 == x1`). Vertical text: a
/// zero-height horizontal bar (`y0 == y1`) spanning the column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Caret {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub line: usize,
}

impl Caret {
    pub fn rect(&self) -> Rect {
        Rect {
            x0: self.x0,
            y0: self.y0,
            x1: self.x1,
            y1: self.y1,
        }
    }
}

/// One layer's laid-out text: the glyphs, lines and caret stops the renderer
/// used.
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    /// The layer text the ranges index (paragraph separators are `\r`, `\n` or
    /// U+2029, as stored).
    pub text: String,
    /// Lines in visual order.
    pub lines: Vec<TextLine>,
    /// Every drawn glyph, in line order.
    pub glyphs: Vec<PlacedGlyph>,
    /// Caret stops in text order, one per grapheme cluster of drawn text
    /// (paragraph separators have none). See [`CaretCluster`].
    pub clusters: Vec<CaretCluster>,
    /// Vertical (`Ornt = Vrtc`) text: the inline axis is `y`.
    pub vertical: bool,
    /// Size of the text box the layout ran in; for point text the measured
    /// size, exactly as the renderer sized its canvas.
    pub box_size: (f32, f32),
    /// Document position of the layout origin for the in-rect entry points,
    /// `(0, 0)` otherwise. Document position of a point is `anchor + (x, y)`.
    pub anchor: (f32, f32),
    /// The engine's y-up baseline of the first line (the reference `y` is
    /// flipped around); useful to map back onto `LayerPixels::offset`.
    pub first_baseline: f32,
}

impl TextLayout {
    /// Number of bytes of layer text the layout covers.
    pub fn text_len(&self) -> usize {
        self.text.len()
    }

    /// No line was placed (empty text, or a box that dropped everything).
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Snaps a byte index to the nearest preceding character boundary, clamped
    /// to the text, so callers never have to check for mid-character offsets.
    pub fn snap(&self, byte: usize) -> usize {
        let mut at = byte.min(self.text.len());
        while at > 0 && !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Index of the line `byte` belongs to.
    ///
    /// Paragraph separators are not part of a line: a byte on a separator (or
    /// any byte no line draws) belongs to the line whose text ends at or before
    /// it, so `caret(break_byte)` stays at the end of the line before the
    /// break. An empty paragraph's line owns its separator bytes (they are the
    /// only bytes a caret can sit on between two paragraphs). `None` only when
    /// no line was placed.
    pub fn line_of(&self, byte: usize) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let at = self.snap(byte);
        if at >= self.text.len() {
            return Some(self.lines.len() - 1);
        }
        // A break byte sits at the end of the line it belongs to, even when the
        // next line also starts there (a soft wrap).
        for (i, line) in self.lines.iter().enumerate() {
            if line.range.end == at && line.range.start < at {
                return Some(i);
            }
        }
        for (i, line) in self.lines.iter().enumerate() {
            if line.range.start <= at && at < line.range.end {
                return Some(i);
            }
        }
        // A separator run between lines belongs to the line before it.
        let mut before = 0;
        for (i, line) in self.lines.iter().enumerate() {
            if line.range.end <= at {
                before = i;
            }
        }
        Some(before)
    }

    /// First (`end == false`) or last (`end == true`) byte of the line `byte`
    /// is on; `0` when no line was placed.
    ///
    /// The end is a **break byte**: the text the line draws ends there, and the
    /// separator (a paragraph break) or the next line's first byte (a soft
    /// wrap) follows. `caret` on that byte stays on this line.
    pub fn line_edge(&self, byte: usize, end: bool) -> usize {
        let Some(i) = self.line_of(byte) else {
            return 0;
        };
        let line = &self.lines[i];
        if end {
            if i + 1 == self.lines.len() {
                self.text.len()
            } else {
                line.range.end
            }
        } else {
            line.range.start
        }
    }

    /// Byte index of the `forward`-th line from the line `byte` is on, keeping
    /// the column `column` (an x for horizontal text, a y for vertical text).
    ///
    /// Clamps at the text: stepping back from the first line yields `0`,
    /// stepping forward from the last yields `len()`. Vertical text has one
    /// column today, so a forward step goes to the text end and a backward one
    /// to its start.
    pub fn line_step(&self, byte: usize, forward: bool, column: f32) -> usize {
        let Some(current) = self.line_of(byte) else {
            return byte;
        };
        if self.vertical {
            // Vertical text: one line per column, so a line step is a column
            // step; `column` is the along-the-column y to keep.
            let target = if forward {
                if current + 1 >= self.lines.len() {
                    return self.text.len();
                }
                current + 1
            } else if current == 0 {
                return 0;
            } else {
                current - 1
            };
            let placed = &self.lines[target];
            let x = (placed.x0 + placed.x1) * 0.5;
            let (_, inline) = self.caret_position(self.snap(byte));
            let _ = column;
            return self.hit_test(x, inline);
        }
        let target = if forward {
            if current + 1 >= self.lines.len() {
                self.text.len()
            } else {
                current + 1
            }
        } else if current == 0 {
            0
        } else {
            current - 1
        };
        if forward && current + 1 >= self.lines.len() {
            return target;
        }
        if !forward && current == 0 {
            return target;
        }
        self.hit_test(column, self.lines[target].baseline)
    }

    /// Caret rectangle for a byte index, with the text-end case.
    ///
    /// A byte at the end of a line (the break byte of a paragraph separator or
    /// of a soft wrap) draws at that line's end, and `caret(text_len)` draws
    /// past the last cluster of the last line.
    pub fn caret(&self, byte: usize) -> Caret {
        let at = self.snap(byte);
        let (line_index, inline) = self.caret_position(at);
        let Some(line) = self.lines.get(line_index) else {
            return Caret::default();
        };
        if self.vertical {
            Caret {
                x0: line.x0,
                y0: inline,
                x1: line.x1,
                y1: inline,
                line: line_index,
            }
        } else {
            Caret {
                x0: inline,
                y0: line.baseline - line.ascent,
                x1: inline,
                y1: line.baseline + line.descent,
                line: line_index,
            }
        }
    }

    /// Byte index of the caret stop nearest to a point in layout space.
    ///
    /// The y picks the line (the nearest one when the point is outside every
    /// line box) and x picks the caret stop on it; the answer is always a
    /// character boundary, and `caret(hit_test(x, y))` lands where the point
    /// was. Past the last stop of a line the answer is that line's break byte,
    /// which hit-tests to itself.
    pub fn hit_test(&self, x: f32, y: f32) -> usize {
        self.hit_test_line(x, y).0
    }

    /// [`Self::hit_test`] with the index of the line the point resolved to.
    pub fn hit_test_line(&self, x: f32, y: f32) -> (usize, usize) {
        let Some(line_index) = self.line_at(x, y) else {
            return (0, 0);
        };
        if self.vertical {
            return (self.vertical_hit_in(line_index, y), line_index);
        }
        let line = &self.lines[line_index];
        for cluster in &self.clusters {
            if cluster.line != line_index {
                continue;
            }
            if x < cluster.x + cluster.advance * 0.5 {
                return (cluster.range.start, line_index);
            }
        }
        if line_index + 1 == self.lines.len() {
            (self.text.len().max(line.range.end), line_index)
        } else {
            (line.range.end, line_index)
        }
    }

    /// Word start in the given direction: skips non-alphanumeric characters,
    /// then a run of alphanumerics (an underscore, a hyphen and a space all end
    /// a word, which is what Photoshop's word-wise cursor moves do).
    pub fn word_boundary(&self, byte: usize, forward: bool) -> usize {
        let at = self.snap(byte);
        if forward {
            let mut pos = at;
            while let Some(c) = self.text[pos..].chars().next() {
                if c.is_alphanumeric() {
                    break;
                }
                pos += c.len_utf8();
            }
            while let Some(c) = self.text[pos..].chars().next() {
                if !c.is_alphanumeric() {
                    break;
                }
                pos += c.len_utf8();
            }
            pos
        } else {
            let mut pos = at;
            while pos > 0 {
                let prev = self.prev_char_start(pos);
                if self.text[prev..pos]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
                {
                    break;
                }
                pos = prev;
            }
            while pos > 0 {
                let prev = self.prev_char_start(pos);
                if !self.text[prev..pos]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
                {
                    break;
                }
                pos = prev;
            }
            pos
        }
    }

    /// Bounding box of the placed lines, `None` only when nothing was placed.
    ///
    /// Glyph-less lines (an empty paragraph, an empty layer) are included: a
    /// click on an empty point-text layer must still land on its caret line.
    /// The vertical extent is the line boxes' ascent/descent, not the ink
    /// outlines; use the layer's pixels for exact ink.
    pub fn bounds(&self) -> Option<Rect> {
        if self.lines.is_empty() {
            return None;
        }
        let mut out: Option<Rect> = None;
        for line in &self.lines {
            let (x0, x1) = (line.x0, line.x1.max(line.x0));
            let (y0, y1) = if self.vertical {
                (line.baseline, line.end)
            } else {
                (line.baseline - line.ascent, line.baseline + line.descent)
            };
            let rect = Rect { x0, y0, x1, y1 };
            out = Some(match out {
                None => rect,
                Some(r) => Rect {
                    x0: r.x0.min(rect.x0),
                    y0: r.y0.min(rect.y0),
                    x1: r.x1.max(rect.x1),
                    y1: r.y1.max(rect.y1),
                },
            });
        }
        out
    }

    /// The text did not fit: lines were dropped.
    ///
    /// Whitespace after the last drawn glyph (a trailing paragraph separator, a
    /// space the wrap dropped) does not count as overflow; only dropped text
    /// does.
    pub fn overflows(&self) -> bool {
        match self.lines.last() {
            Some(line) => self
                .text
                .get(line.range.end.min(self.text.len())..)
                .is_none_or(|tail| tail.chars().any(|c| !c.is_whitespace())),
            None => !self.text.is_empty(),
        }
    }

    /// Byte index of the line nearest to a point: the line whose box contains
    /// `y` (horizontal text) or `x` (vertical text, where columns are picked
    /// across the column), or the nearest one.
    fn line_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let mut best = (f32::INFINITY, 0usize);
        for (i, line) in self.lines.iter().enumerate() {
            let (lo, hi, at) = if self.vertical {
                (line.x0, line.x1, x)
            } else {
                (
                    line.baseline - line.ascent,
                    line.baseline + line.descent,
                    y,
                )
            };
            let distance = if at < lo {
                lo - at
            } else if at > hi {
                at - hi
            } else {
                0.0
            };
            if distance < best.0 {
                best = (distance, i);
            }
        }
        Some(best.1)
    }

    /// `(line, inline position)` of the caret at a snapped byte index.
    fn caret_position(&self, at: usize) -> (usize, f32) {
        let Some(last) = self.lines.len().checked_sub(1) else {
            return (0, 0.0);
        };
        if at >= self.text.len() {
            return (last, self.lines[last].end);
        }
        // A byte at a line's end is that line's break byte: keep the caret on
        // the line the text was drawn on, not at the start of the next one.
        for (i, line) in self.lines.iter().enumerate() {
            if line.range.end == at && line.range.start < at {
                return (i, line.end);
            }
        }
        for cluster in &self.clusters {
            if cluster.range.start <= at && at < cluster.range.end {
                if at == cluster.range.start {
                    return (cluster.line, cluster.x);
                }
                return (cluster.line, cluster.x + cluster.advance);
            }
        }
        // A byte in a separator run maps to the end of the line before it.
        let line = self.line_of(at).unwrap_or(last);
        let placed = &self.lines[line];
        if at < placed.range.start {
            (line, placed.x0)
        } else {
            (line, placed.end)
        }
    }

    /// Vertical text: the nearest caret stop along the given column.
    fn vertical_hit_in(&self, line: usize, y: f32) -> usize {
        for cluster in &self.clusters {
            if cluster.line != line {
                continue;
            }
            if y < cluster.x + cluster.advance * 0.5 {
                return cluster.range.start;
            }
        }
        // Past the column's last stop the hit is the column's break byte.
        match self.lines.get(line) {
            Some(placed) if line + 1 == self.lines.len() => placed.range.end.max(self.text.len()),
            Some(placed) => placed.range.end,
            None => self.text.len(),
        }
    }

    fn prev_char_start(&self, pos: usize) -> usize {
        let mut at = pos.saturating_sub(1);
        while at > 0 && !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }
}

/// Vertical metrics of a face at a size: `(ascent, descent)` in points.
fn face_metrics(face: &Face<'_>, size: f32) -> (f32, f32) {
    let upm = f32::from(face.units_per_em());
    if upm <= 0.0 {
        return (size, size * 0.25);
    }
    let ascent = f32::from(face.hhea.ascender) / upm * size;
    let descent = -f32::from(face.hhea.descender) / upm * size;
    let ascent = if ascent.is_finite() && ascent > 0.0 {
        ascent
    } else {
        size
    };
    let descent = if descent.is_finite() && descent > 0.0 {
        descent
    } else {
        size * 0.25
    };
    (ascent, descent)
}

/// A row of the layout: a drawn line or an empty paragraph's caret row.
struct Row {
    start: usize,
    /// Last byte the row itself draws or owns (a dropped tail is beyond it).
    content_end: usize,
    baseline: f32,
    x0: f32,
    x1: f32,
    end: f32,
    ascent: f32,
    descent: f32,
    paragraph: usize,
}

fn line_row(prepared: &Prepared<'_>, line: &Line, text: &str) -> Row {
    let mut ascent = 0.0f32;
    let mut descent = 0.0f32;
    for attrs in &line.attrs {
        let run = prepared.style_runs.get(attrs.run);
        let size = run.and_then(|r| r.size).unwrap_or(prepared.spec.style.size);
        let face = run
            .and_then(|r| r.font.as_deref())
            .and_then(|name| prepared.faces.get(name))
            .unwrap_or_else(|| prepared.faces.default_face());
        let (a, d) = face_metrics(face, size);
        ascent = ascent.max(a);
        descent = descent.max(d);
    }
    if ascent <= 0.0 {
        ascent = prepared.spec.style.size;
    }
    if descent <= 0.0 {
        descent = prepared.spec.style.size * 0.25;
    }
    let x0 = line.glyphs.first().map(|g| g.x).unwrap_or(0.0);
    // The pen walks over the spaces a wrap dropped too; the line's ink ends at
    // the last non-whitespace glyph, and `end` keeps the full pen so a caret
    // after those spaces still has a position.
    let pen_end = line.glyphs.last().map(|g| g.x + g.advance).unwrap_or(x0);
    let drawn_end = line
        .glyphs
        .iter()
        .zip(&line.attrs)
        .filter(|(_, attrs)| {
            text.get(attrs.source.0..attrs.source.1)
                .is_none_or(|slice| slice.chars().any(|c| !c.is_whitespace()))
        })
        .map(|(glyph, _)| glyph.x + glyph.advance)
        .fold(x0, f32::max);
    let x1 = drawn_end.max(x0);
    Row {
        start: if line.glyphs.is_empty() {
            0
        } else {
            line.range_start
        },
        content_end: line.range_end,
        baseline: line.baseline,
        x0,
        x1,
        end: pen_end.max(x1),
        ascent,
        descent,
        paragraph: line.paragraph,
    }
}

fn empty_row(prepared: &Prepared<'_>, empty: &EmptyParagraph) -> Row {
    let run = prepared.style_runs.first();
    let size = run.and_then(|r| r.size).unwrap_or(prepared.spec.style.size);
    let face = run
        .and_then(|r| r.font.as_deref())
        .and_then(|name| prepared.faces.get(name))
        .unwrap_or_else(|| prepared.faces.default_face());
    let (ascent, descent) = face_metrics(face, size);
    Row {
        start: empty.range_start,
        content_end: empty.range_end,
        baseline: empty.baseline,
        x0: empty.x,
        x1: empty.x,
        end: empty.x,
        ascent,
        descent,
        paragraph: empty.paragraph,
    }
}

/// Builds the public layout of a horizontal (point/box/area) layer.
pub(super) fn layout_of(prepared: &Prepared<'_>, text: &str, anchor: (f32, f32)) -> TextLayout {
    let first_baseline = prepared.lines.first().map(|l| l.baseline).unwrap_or(0.0);
    let mut glyphs = Vec::new();
    for line in &prepared.lines {
        for (glyph, attrs) in line.glyphs.iter().zip(&line.attrs) {
            glyphs.push(PlacedGlyph {
                range: attrs.source.0..attrs.source.1,
                gid: glyph.gid,
                run: attrs.run,
                x: glyph.x,
                y: first_baseline - glyph.y,
                advance: glyph.advance,
                color: attrs.color,
            });
        }
    }
    let mut rows: Vec<Row> = prepared
        .lines
        .iter()
        .map(|l| line_row(prepared, l, text))
        .collect();
    rows.extend(
        prepared
            .empty
            .iter()
            .map(|empty| empty_row(prepared, empty)),
    );
    // Text order is visual order for horizontal text; an empty paragraph's
    // separator bytes sort it between the lines around it.
    rows.sort_by_key(|row| row.start);
    if rows.is_empty() {
        // An empty point-text layer still has one caret line at its origin, so
        // a click on the (glyph-less) layer lands somewhere.
        let run = prepared.style_runs.first();
        let size = run.and_then(|r| r.size).unwrap_or(prepared.spec.style.size);
        let face = run
            .and_then(|r| r.font.as_deref())
            .and_then(|name| prepared.faces.get(name))
            .unwrap_or_else(|| prepared.faces.default_face());
        let (ascent, descent) = face_metrics(face, size);
        rows.push(Row {
            start: 0,
            content_end: 0,
            baseline: 0.0,
            x0: prepared.spec.origin.0,
            x1: prepared.spec.origin.0,
            end: prepared.spec.origin.0,
            ascent,
            descent,
            paragraph: 0,
        });
    }
    let lines = placed_lines(&mut rows, text, first_baseline);
    let clusters = caret_clusters(&lines, &glyphs, text, false);
    TextLayout {
        text: text.to_string(),
        lines,
        glyphs,
        clusters,
        vertical: false,
        box_size: prepared.spec.box_size,
        anchor,
        first_baseline,
    }
}

/// Builds the public layout of a vertical (`Ornt = Vrtc`) layer.
///
/// One column per hard-break line: a column's `x0`/`x1` are its ink extent
/// across the column (the column centre is their midpoint), `end` is how far
/// it runs down, and `baseline` is `0` for every column. Columns advance in
/// `-x` in this space, which the editor maps to document left: vertical type
/// reads right to left.
pub(super) fn layout_vertical_of(
    prepared: &PreparedVertical<'_>,
    text: &str,
    anchor: (f32, f32),
) -> TextLayout {
    let size = prepared.spec.style.size;
    let descent = prepared
        .style_runs
        .first()
        .and_then(|r| r.font.as_deref())
        .and_then(|name| prepared.faces.get(name))
        .map_or(size * 0.25, |face| face_metrics(face, size).1);
    let mut glyphs = Vec::new();
    let mut lines = Vec::with_capacity(prepared.lines.len());
    for (index, column) in prepared.lines.iter().enumerate() {
        let mut x0 = f32::INFINITY;
        let mut x1 = f32::NEG_INFINITY;
        let mut end = 0.0f32;
        for (glyph, attrs) in column.glyphs.iter().zip(&column.attrs) {
            let y = (-glyph.y).max(0.0);
            glyphs.push(PlacedGlyph {
                range: attrs.source.0..attrs.source.1,
                gid: glyph.gid,
                run: attrs.run,
                x: glyph.x,
                y,
                advance: glyph.advance,
                color: attrs.color,
            });
            x0 = x0.min(glyph.x);
            x1 = x1.max(glyph.x + glyph.advance);
            end = end.max(y + size);
        }
        let range = if column.range_end > column.range_start {
            column.range_start.min(text.len())..column.range_end.min(text.len())
        } else {
            column.range_start.min(text.len())..column.range_start.min(text.len())
        };
        let (x0, x1) = if x0.is_finite() && x1.is_finite() && x1 > x0 {
            (x0, x1)
        } else {
            // A glyph-less column (an empty paragraph): a caret width to sit in,
            // on the same right-anchored ladder the glyph columns use.
            let centre = prepared.first_centre - index as f32 * prepared.pitch;
            (centre - size * 0.25, centre + size * 0.25)
        };
        let half = ((x1 - x0) * 0.5).max(descent * 0.5);
        lines.push(TextLine {
            range,
            baseline: 0.0,
            x0,
            x1,
            end: end.max(if column.glyphs.is_empty() { 0.0 } else { size }),
            ascent: half,
            descent: half,
            paragraph: column.paragraph,
        });
    }
    if lines.is_empty() && !text.is_empty() {
        lines.push(TextLine {
            range: 0..0,
            baseline: 0.0,
            x0: -size * 0.25,
            x1: size * 0.25,
            end: size,
            ascent: descent,
            descent,
            paragraph: 0,
        });
    }
    let clusters = caret_clusters(&lines, &glyphs, text, true);
    TextLayout {
        text: text.to_string(),
        lines,
        glyphs,
        clusters,
        vertical: true,
        box_size: prepared.spec.box_size,
        anchor,
        first_baseline: 0.0,
    }
}

/// The bytes a row draws, with paragraph separators trimmed off.
///
/// Lines do not own their paragraph separator: the break byte between two
/// lines is a caret position, not text. An empty paragraph is the exception —
/// its separator is the only byte inside it, so it keeps it (that is where the
/// caret of an empty paragraph sits).
fn row_range(row: &Row, text: &str) -> Range<usize> {
    let start = row.start.min(text.len());
    let end = row.content_end.max(row.start).min(text.len());
    if end <= start {
        return start..start;
    }
    let slice = &text[start..end];
    if slice.chars().all(is_separator) {
        return start..end;
    }
    let mut a = start;
    while a < end {
        let c = text[a..end].chars().next().unwrap_or_default();
        if !is_separator(c) {
            break;
        }
        a += c.len_utf8();
    }
    let mut b = end;
    while b > a {
        let c = text[..b].chars().next_back().unwrap_or_default();
        if !is_separator(c) {
            break;
        }
        b -= c.len_utf8();
    }
    a..b
}

/// Paragraph separators the engine accepts (`\r`, `\n`, U+2029).
fn is_separator(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{2029}')
}

fn placed_lines(rows: &mut [Row], text: &str, first_baseline: f32) -> Vec<TextLine> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        let range = row_range(row, text);
        out.push(TextLine {
            range,
            baseline: first_baseline - row.baseline,
            x0: row.x0,
            x1: row.x1,
            end: row.end,
            ascent: row.ascent,
            descent: row.descent,
            paragraph: row.paragraph,
        });
    }
    out
}

/// Caret stops for every grapheme cluster, with the pen position taken from
/// the glyphs shaped from it.
///
/// Paragraph separators get no stop (they are line breaks, see
/// [`TextLayout::caret`]). A ligature's first cluster owns the ligature's
/// advance; the components after it are zero-advance stops at the ligature's
/// end, which is where a caret between them draws.
fn caret_clusters(
    lines: &[TextLine],
    glyphs: &[PlacedGlyph],
    text: &str,
    vertical: bool,
) -> Vec<CaretCluster> {
    let mut out = Vec::new();
    let inline = |g: &PlacedGlyph| if vertical { g.y } else { g.x };
    let mut next_glyph = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let slice = match text.get(line.range.clone()) {
            Some(slice) => slice,
            None => continue,
        };
        let base = line.range.start;
        let mut pen = line.x0;
        let mut owner: Option<usize> = None;
        for (offset, cluster) in slice.grapheme_indices(true) {
            let start = base + offset;
            let end = start + cluster.len();
            if cluster.chars().all(is_separator) {
                continue;
            }
            while next_glyph < glyphs.len() && glyphs[next_glyph].range.end <= start {
                next_glyph += 1;
            }
            let current = next_glyph
                .lt(&glyphs.len())
                .then(|| &glyphs[next_glyph])
                .filter(|g| g.range.start <= start && start < g.range.end);
            let x = match current {
                Some(glyph) if owner == Some(next_glyph) => inline(glyph) + glyph.advance,
                Some(glyph) => inline(glyph),
                None => pen,
            };
            owner = current.map(|_| next_glyph);
            out.push(CaretCluster {
                range: start..end,
                line: index,
                x,
                advance: 0.0,
                rtl: false,
            });
            pen = x;
        }
    }
    // Advances are pen distances to the next stop on the same line. A zero-width
    // stop at a ligature's end (or the second stop of a shared glyph) keeps
    // zero; the first stop of the glyph carries the whole advance.
    let mut advances = Vec::with_capacity(out.len());
    for i in 0..out.len() {
        let target = match out.get(i + 1) {
            Some(next) if next.line == out[i].line => next.x,
            _ => lines[out[i].line].end,
        };
        advances.push((target - out[i].x).max(0.0));
    }
    for (cluster, advance) in out.iter_mut().zip(advances) {
        cluster.advance = advance;
    }
    out
}
