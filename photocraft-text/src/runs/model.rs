use std::collections::BTreeMap;

use crate::area::Justification;
use crate::engine_data::Value;
use crate::font::Face;
use crate::layout::GlyphPos;

/// One style run covering `len` UTF-16 code units of the layer text.
///
/// Optional fields override the base [`crate::layout::TextStyle`]; `None` inherits it.
/// `sheet` keeps the parsed `StyleSheetData` so keys this model does not
/// interpret survive [`super::to_engine_data`] unchanged.
#[derive(Clone, Debug, Default)]
pub struct RunStyle {
    pub len: usize,
    /// Font name, resolved through `ResourceDict/FontSet`. `None` = default face.
    pub font: Option<String>,
    /// Index into `ResourceDict/FontSet` as stored in EngineData (`Font`).
    pub font_index: Option<i64>,
    pub size: Option<f32>,
    pub color: Option<[u8; 3]>,
    pub tracking: Option<i16>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    /// `AutoKerning`: `Some(true)` = metrics, `Some(false)` = manual `kerning`.
    pub auto_kerning: Option<bool>,
    /// Manual kerning in 1/1000 em (`Kerning`).
    pub kerning: Option<i16>,
    /// Explicit leading in points (`Leading`), used when `auto_leading` is off.
    pub leading: Option<f32>,
    /// `AutoLeading`: `true` derives leading from the auto-leading fraction.
    pub auto_leading: Option<bool>,
    pub faux_bold: bool,
    pub faux_italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub all_caps: bool,
    pub small_caps: bool,
    /// Old-style figures. `EngineData` has no key for this feature (Photoshop
    /// writes it only to `Txt2` `/6/30`, measured "on" value 2); the renderer
    /// starts with `false` and a caller that can read `Txt2` sets it.
    pub old_style: bool,
    /// `Ligatures`; `None` keeps the sheet's value.
    pub ligatures: Option<bool>,
    /// `DLigatures` (Photoshop's "alternate ligatures"); `None` keeps it.
    pub alternate_ligatures: Option<bool>,
    /// `Language`, as the `Txt2` `/6/38` code (0 default, 14 English UK,
    /// 42 German). `EngineData` uses a different id space for some languages,
    /// so [`super::engine_language`] translates before writing.
    pub language: Option<i64>,
    /// `NoBreak`: keep the run on one line.
    pub no_break: Option<bool>,
    pub sheet: Option<Value>,
}

/// One paragraph run: `len` UTF-16 code units sharing paragraph formatting.
/// Indents and spacing are points; `auto_leading` is a multiplier of size.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ParagraphRun {
    /// UTF-16 code units covered by the run, paragraph separators included.
    pub len: usize,
    /// `[min desired max]` word spacing, fractions of the space width.
    pub word_spacing: [f32; 3],
    /// `[min desired max]` letter spacing, fractions of the em.
    pub letter_spacing: [f32; 3],
    /// `[min desired max]` glyph scaling.
    pub glyph_scaling: [f32; 3],
    pub justification: Justification,
    pub first_line_indent: f32,
    pub start_indent: f32,
    pub end_indent: f32,
    pub space_before: f32,
    pub space_after: f32,
    pub auto_leading: f32,
    /// `EveryLineComposer`: Photoshop's "Adobe Every-line Composer" instead of
    /// the default single-line (greedy) composer.
    pub every_line_composer: bool,
    /// `Hanging`: optical margin alignment (hanging punctuation).
    pub hanging: bool,
    /// `AutoHyphenate`.
    pub hyphenate: bool,
    /// `HyphenatedWordSize`: hyphenate words longer than this.
    pub words_longer_than: i64,
    /// `PreHyphen`: characters required before the hyphen.
    pub pre_hyphen: i64,
    /// `PostHyphen`: characters required after the hyphen.
    pub post_hyphen: i64,
    /// `ConsecutiveHyphens`: maximum consecutive hyphenated lines.
    pub consecutive_hyphens: i64,
    /// `Zone`: hyphenation zone in points.
    pub hyphen_zone: f32,
}

/// Paragraph formatting that `EngineData` carries beyond [`ParagraphRun`]:
/// hyphenation and hanging punctuation.
///
/// Photoshop keeps these in the same `ParagraphSheet/Properties` dict as the
/// indents and spacing; the key names are read from Photoshop's own
/// `EngineData` (`out/probe2/base.psd`: `AutoHyphenate false`,
/// `HyphenatedWordSize 6`, `PreHyphen 2`, `PostHyphen 2`,
/// `ConsecutiveHyphens 8`, `Zone 36.0`, `Hanging false`). `Zone` is in points,
/// the same number `Txt2` keeps at `/5/14` (`out/probe5/pE.psd`: DOM 18 →
/// `/5/14 18.0`, `Zone 18.0`). The models' *defaults* differ in one place:
/// `ConsecutiveHyphens` defaults to `8` here, `Txt2` `/5/13` defaults to `0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParagraphFormat {
    /// `AutoHyphenate`.
    pub hyphenate: bool,
    /// `HyphenatedWordSize`: hyphenate words longer than this.
    pub words_longer_than: i64,
    /// `PreHyphen`: characters required before the hyphen.
    pub after_first: i64,
    /// `PostHyphen`: characters required after the hyphen.
    pub before_last: i64,
    /// `ConsecutiveHyphens`: maximum consecutive hyphens.
    pub limit: i64,
    /// `Zone`: hyphenation zone in points (same value as `Txt2` `/5/14`).
    pub zone: f32,
    /// `Hanging`: hanging punctuation.
    pub hanging_punctuation: bool,
}

impl Default for ParagraphFormat {
    /// Photoshop's defaults, as its own `EngineData` records them.
    fn default() -> Self {
        Self {
            hyphenate: false,
            words_longer_than: 6,
            after_first: 2,
            before_last: 2,
            limit: 8,
            zone: 36.0,
            hanging_punctuation: false,
        }
    }
}

/// Faces a layer's runs can reference, keyed by font name. The default face is
/// used by runs without a `font`.
pub struct FontSet<'a> {
    default: String,
    faces: BTreeMap<String, Face<'a>>,
}

impl<'a> FontSet<'a> {
    pub fn new(name: impl Into<String>, face: Face<'a>) -> Self {
        let name = name.into();
        let mut faces = BTreeMap::new();
        faces.insert(name.clone(), face);
        Self {
            default: name,
            faces,
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, face: Face<'a>) {
        self.faces.insert(name.into(), face);
    }

    pub fn get(&self, name: &str) -> Option<&Face<'a>> {
        self.faces.get(name)
    }

    pub fn default_name(&self) -> &str {
        &self.default
    }

    pub fn default_face(&self) -> &Face<'a> {
        &self.faces[&self.default]
    }
}

/// A shaped glyph together with the run it came from.
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub glyph: GlyphPos,
    /// Index into the `runs` slice passed to [`super::shape_runs_detailed`].
    pub run: usize,
    /// Vertical scale of the run (1.0 = 100%). Glyph height is not part of
    /// [`GlyphPos`], so the rasteriser applies it.
    pub vertical_scale: f32,
    pub color: Option<[u8; 3]>,
}

/// Small caps: lowercase letters set (synthetically) at this fraction of the
/// run size. Measured on `10_all_features.psd` layer `15 small caps old style
/// 0123456789`: with old-style figures the raster is 417 px wide at 26 pt, and
/// the lower-case letters account for exactly 0.8 of their capital advances
/// (0.7 predicts 393 px, 0.8 predicts 417 px; the raster matches at 417).
pub const SMALL_CAPS_SCALE: f32 = 0.8;

/// Per-glyph attributes that never change pen positions. Glyph height and
/// skew are applied by the rasteriser.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphAttrs {
    /// Index into the `runs` slice passed to [`super::shape_runs_attrs`].
    pub run: usize,
    /// Fill colour of the run; black when the run sets none.
    pub color: [u8; 3],
    /// Glyph height factor: run `VerticalScale`, times [`SMALL_CAPS_SCALE`] for
    /// small-caps glyphs. Pen x is unaffected.
    pub vertical_scale: f32,
    /// Glyph width factor: [`SMALL_CAPS_SCALE`] for small-caps glyphs, `1.0`
    /// otherwise. Small caps shrink the whole glyph, not just its height.
    pub x_scale: f32,
    /// Faux italic: the rasteriser skews the glyph. Pen x is unaffected.
    pub faux_italic: bool,
    /// This run's line advance in points (`Leading`, or 1.2 x size).
    pub leading: f32,
}

impl RunStyle {
    /// `AutoLeading` for this run: an explicit leading with no flag means auto
    /// is off, which is what Photoshop's own sheet carries.
    pub fn auto_leading_flag(&self) -> Option<bool> {
        self.auto_leading.or(self.leading.map(|_| false))
    }
}
