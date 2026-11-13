//! Point-text layout: glyph advances, tracking, kerning, line breaks.

use crate::font::Face;

/// How gaps between glyph pairs are chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kerning {
    /// Font supplied (`kern` / GPOS) pairs.
    #[default]
    Metrics,
    /// Adobe's shape-based optical kerning (not implemented yet).
    Optical,
    /// Manual value from the style run, in 1/1000 em.
    Manual(i16),
}

/// Character formatting for one run. Units follow Photoshop's EngineData:
/// tracking and manual kerning are 1/1000 em, `size` is in points.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub size: f32,
    pub tracking: i16,
    pub kerning: Kerning,
    pub horizontal_scale: f32,
    pub baseline_shift: f32,
    /// Explicit leading in points; `None` means auto, which is
    /// `size * auto_leading`.
    pub leading: Option<f32>,
    /// Auto-leading fraction from the paragraph sheet (Photoshop default 1.2).
    pub auto_leading: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 12.0,
            tracking: 0,
            kerning: Kerning::Metrics,
            horizontal_scale: 1.0,
            baseline_shift: 0.0,
            leading: None,
            auto_leading: 1.2,
        }
    }
}

impl TextStyle {
    pub fn leading_pt(&self) -> f32 {
        self.leading.unwrap_or(self.size * self.auto_leading)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphPos {
    pub gid: u16,
    pub ch: char,
    /// Pen position of the glyph origin, in the layout's coordinate space.
    pub x: f32,
    pub y: f32,
    /// Distance to the next glyph on the same line, in points.
    pub advance: f32,
    /// Advance from the font's metrics plus this glyph's tracking and faux-bold
    /// step, with no kerning.  Photoshop's composed cache (`Txt2` `/3`) is
    /// measured against this nominal advance, so replacing the spacing model
    /// with `nominal + correction` must not carry the engine's kerning a second
    /// time.
    pub nominal_advance: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub glyphs: Vec<GlyphPos>,
    pub baseline: f32,
    pub width: f32,
}

/// Computes pen positions for a single run.
///
/// `advance` on each glyph is the pen delta to the next glyph, so summing it
/// reproduces the layout exactly (kerning included).
pub fn shape(face: &Face<'_>, text: &str, style: &TextStyle) -> Vec<GlyphPos> {
    let upm = face.units_per_em() as f32;
    let scale = style.size / upm;
    let hscale = style.horizontal_scale;
    let tracking = style.tracking as f32 / 1000.0 * style.size;

    let mut out: Vec<GlyphPos> = Vec::with_capacity(text.chars().count());
    let mut pen = 0f32;
    let mut prev_gid: Option<u16> = None;

    for ch in text.chars() {
        if ch == '\r' || ch == '\n' {
            continue;
        }
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let kern = match (style.kerning, prev_gid) {
            (_, None) => 0.0,
            (Kerning::Metrics, Some(prev)) => face.kern_pair(prev, gid) as f32 * scale,
            (Kerning::Manual(v), Some(_)) => v as f32 / 1000.0 * style.size,
            (Kerning::Optical, Some(prev)) => {
                crate::optical::optical_pair(face, prev, gid, style.size)
            }
        };
        pen += kern * hscale;
        // Photoshop applies the horizontal scale to the whole advance,
        // tracking included: a 300/1000 em tracking step at 150 % is 1.5x as
        // wide (measured on `10_all_features.psd` layer `03 tracking baseline
        // scales`: 12 glyphs, 22 pt, hscale 1.5, tracking 300 give a 304 px
        // ink width; tracking unscaled predicts 268 px).
        let advance = face.advance(gid) as f32 * scale * hscale + tracking * hscale;
        out.push(GlyphPos {
            gid,
            ch,
            x: pen,
            y: 0.0,
            advance,
            nominal_advance: advance,
        });
        pen += advance;
        prev_gid = Some(gid);
    }

    // Convert absolute pen positions into forward deltas.
    let mut deltas = Vec::with_capacity(out.len());
    for i in 0..out.len() {
        let next = out
            .get(i + 1)
            .map(|g| g.x)
            .unwrap_or(out[i].x + out[i].advance);
        deltas.push(next - out[i].x);
    }
    for (g, d) in out.iter_mut().zip(deltas) {
        g.advance = d;
        g.x = 0.0;
    }
    out
}

/// Lays out one line of text made of consecutive styled runs.
///
/// The pen continues across runs (a run boundary adds no advance of its own);
/// each run's baseline shift moves only its own glyphs.
pub fn layout_point_runs(face: &Face<'_>, runs: &[(&str, TextStyle)], origin: (f32, f32)) -> Line {
    let mut line = Line {
        baseline: origin.1,
        ..Default::default()
    };
    let mut pen = origin.0;
    for (text, style) in runs {
        let y = origin.1 + style.baseline_shift;
        if let Some(first) = line.glyphs.first_mut() {
            first.y = first.y.min(y);
        }
        line.baseline = line.baseline.max(y);
        for g in shape(face, text, style) {
            pen += g.advance;
            line.glyphs.push(GlyphPos {
                x: pen - g.advance,
                y,
                ..g
            });
        }
    }
    line.width = pen - origin.0;
    line
}

/// Lays out point text: `\r` starts a new line, the first baseline sits at the
/// given origin and following lines step down by the leading.
pub fn layout_point(
    face: &Face<'_>,
    text: &str,
    style: &TextStyle,
    origin: (f32, f32),
) -> Vec<Line> {
    let leading = style.leading_pt();
    let mut lines = Vec::new();
    let mut line = Line {
        baseline: origin.1 + style.baseline_shift,
        ..Default::default()
    };
    let mut pen = origin.0;

    for chunk in text.split(['\r', '\n']) {
        if !line.glyphs.is_empty() {
            line.width = pen - origin.0;
            lines.push(std::mem::take(&mut line));
            line.baseline = origin.1 + style.baseline_shift - leading * lines.len() as f32;
            pen = origin.0;
        }
        for g in shape(face, chunk, style) {
            let placed = GlyphPos {
                x: pen,
                y: line.baseline,
                ..g
            };
            pen += g.advance;
            line.glyphs.push(placed);
        }
    }
    line.width = pen - origin.0;
    lines.push(line);
    lines
}
