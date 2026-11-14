//! JSON-compatible PSD authoring models.

pub use crate::runs::RunFeature;

/// One style run, as JSON in `runs.json` or the live editor's request body.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct RunSpec {
    /// Length of this run in UTF-16 units. Every run must be positive, and the
    /// lengths must cover the text plus its trailing U+000D terminator.
    pub len: usize,
    /// PostScript font name (`NotoSans-Bold`); `None` keeps the layer font.
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default)]
    pub size: Option<f32>,
    /// Explicit leading in points; `None` with `auto_leading` unset keeps auto.
    #[serde(default)]
    pub leading: Option<f32>,
    /// `AutoLeading`: derive the line advance from the auto-leading fraction.
    #[serde(default)]
    pub auto_leading: Option<bool>,
    #[serde(default)]
    pub tracking: Option<i16>,
    #[serde(default)]
    pub baseline_shift: Option<f32>,
    #[serde(default)]
    pub horizontal_scale: Option<f32>,
    #[serde(default)]
    pub vertical_scale: Option<f32>,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// `Some(true)` metrics, `Some(false)` manual with `kerning`, `None` keeps.
    #[serde(default)]
    pub auto_kerning: Option<bool>,
    /// Manual kerning in 1/1000 em (`Kerning`).
    #[serde(default)]
    pub kerning: Option<i16>,
    /// Photoshop's auto-kern mode (`Txt2` `/6/11`): 0 manual, 1 metrics, 2 optical.
    /// Wins over [`Self::auto_kerning`] when set; `None` keeps the file's value.
    #[serde(default)]
    pub kerning_mode: Option<i64>,
    /// Character flags. Photoshop's `Txt2` model has no measured slot for these,
    /// so they reach `TySh` `EngineData` and the stored pixels only; a
    /// re-layout in Photoshop recomposes from `Txt2` and drops them.
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    /// The `Txt2` underline style code (`/6/16`): `0` none, `1` right,
    /// `2` left (measured from PS-authored files; `YUnderline` is 0 only for
    /// the left style). `None` derives the code from [`Self::underline`].
    #[serde(default)]
    pub underline_type: Option<i64>,
    #[serde(default)]
    pub strikethrough: bool,
    /// The `Txt2` strike style code (`/6/15`): `0` none, `2` box (measured).
    /// `None` derives the code from [`Self::strikethrough`].
    #[serde(default)]
    pub strike_type: Option<i64>,
    #[serde(default)]
    pub all_caps: bool,
    #[serde(default)]
    pub small_caps: bool,
    /// Old-style figures (`Txt2` `/6/30`: 0 off, 2 on). `author` writes the level.
    #[serde(default)]
    pub old_style: bool,
    /// Standard ligatures (`Txt2` `/6/18`); `None` keeps the file's value.
    #[serde(default)]
    pub ligatures: Option<bool>,
    /// Alternate (discretionary) ligatures (`Txt2` `/6/19`); `None` keeps the value.
    #[serde(default)]
    pub alternate_ligatures: Option<bool>,
    /// No-break run (`Txt2` `/6/52`); `None` keeps the value.
    #[serde(default)]
    pub no_break: Option<bool>,
    /// Language code (`Txt2` `/6/38`, Photoshop's id space); `None` keeps the value.
    #[serde(default)]
    pub language: Option<i64>,
    /// Stroke colour; `None` keeps the file's stroke paint. The fill colour is
    /// [`Self::color`] (`Txt2` `/6/53`; the stroke is the `SimplePaint` at `/6/54`).
    #[serde(default)]
    pub stroke: Option<[u8; 3]>,
    /// `FontBaseline`: 0 normal, 1 superscript, 2 subscript; `None` keeps it.
    #[serde(default)]
    pub baseline: Option<i64>,
    /// `BaselineDirection == 2`: vertical roman text rotated along the column.
    #[serde(default)]
    pub rotate_roman: Option<bool>,
    /// OpenType features (`calt`, `onum`, `ss01`, …); written to `EngineData`
    /// where it has a key and to `Txt2` otherwise.
    #[serde(default)]
    pub features: Vec<RunFeature>,
}

/// Paragraph formatting for the whole layer.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ParagraphSpec {
    /// Photoshop's code (measured with the 7-alignment oracle): 0 left,
    /// 1 right, 2 centre, 3 justify-last-left, 4 justify-last-right,
    /// 5 justify-last-centre, 6 justify-all.
    #[serde(default)]
    pub justification: i64,
    #[serde(default)]
    pub every_line_composer: bool,
    #[serde(default)]
    pub first_line_indent: f32,
    #[serde(default)]
    pub start_indent: f32,
    #[serde(default)]
    pub end_indent: f32,
    #[serde(default)]
    pub space_before: f32,
    #[serde(default)]
    pub space_after: f32,
    #[serde(default = "default_auto_leading")]
    pub auto_leading: f32,
    #[serde(default = "default_word_spacing")]
    pub word_spacing: [f32; 3],
    #[serde(default = "default_letter_spacing")]
    pub letter_spacing: [f32; 3],
    #[serde(default = "default_glyph_scaling")]
    pub glyph_scaling: [f32; 3],
    /// `Hanging`: optical margin alignment (hanging punctuation).
    #[serde(default)]
    pub hanging_punctuation: bool,
    /// `AutoHyphenate`.
    #[serde(default)]
    pub hyphenate: bool,
    /// `HyphenatedWordSize`: hyphenate words longer than this many characters.
    #[serde(default = "default_hyphenate_words")]
    pub hyphenate_words_longer_than: i64,
    /// `PreHyphen`: characters required before the hyphen.
    #[serde(default = "default_hyphenate_after")]
    pub hyphenate_after_first: i64,
    /// `PostHyphen`: characters required after the hyphen.
    #[serde(default = "default_hyphenate_before")]
    pub hyphenate_before_last: i64,
    /// `ConsecutiveHyphens`: maximum consecutive hyphenated lines.
    #[serde(default = "default_hyphenate_limit")]
    pub hyphenate_limit: i64,
    /// `Zone`: hyphenation zone in points.
    #[serde(default = "default_hyphenate_zone")]
    pub hyphenate_zone: f32,
    /// Hyphenate capitalised words (no `EngineData` key; `Txt2` `/5/15`).
    #[serde(default = "default_hyphenate_capital")]
    pub hyphenate_capital_words: bool,
    /// `Burasagari`: CJK line-end hanging punctuation (`EngineData` only).
    #[serde(default)]
    pub burasagari: bool,
    /// `KinsokuOrder`: 0 push in, 1 push out first, 2 push out only
    /// (`EngineData` only).
    #[serde(default)]
    pub kinsoku_order: i64,
}

fn default_auto_leading() -> f32 {
    1.2
}

fn default_word_spacing() -> [f32; 3] {
    [0.8, 1.0, 1.33]
}

fn default_letter_spacing() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}

fn default_glyph_scaling() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

fn default_hyphenate_words() -> i64 {
    6
}

fn default_hyphenate_after() -> i64 {
    2
}

fn default_hyphenate_before() -> i64 {
    2
}

fn default_hyphenate_zone() -> f32 {
    36.0
}

fn default_hyphenate_limit() -> i64 {
    8
}

fn default_hyphenate_capital() -> bool {
    true
}

impl Default for ParagraphSpec {
    fn default() -> Self {
        Self {
            justification: 0,
            every_line_composer: false,
            first_line_indent: 0.0,
            start_indent: 0.0,
            end_indent: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            auto_leading: default_auto_leading(),
            word_spacing: default_word_spacing(),
            letter_spacing: default_letter_spacing(),
            glyph_scaling: default_glyph_scaling(),
            hanging_punctuation: false,
            hyphenate: false,
            hyphenate_words_longer_than: default_hyphenate_words(),
            hyphenate_after_first: default_hyphenate_after(),
            hyphenate_before_last: default_hyphenate_before(),
            hyphenate_limit: default_hyphenate_limit(),
            hyphenate_zone: default_hyphenate_zone(),
            hyphenate_capital_words: default_hyphenate_capital(),
            burasagari: false,
            kinsoku_order: 0,
        }
    }
}

/// A whole layer: the text, its run split, paragraph formatting and renderer
/// settings. This is the JSON the live editor posts.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct LayerSpec {
    pub text: String,
    #[serde(default)]
    pub runs: Vec<RunSpec>,
    #[serde(default)]
    pub paragraph: ParagraphSpec,
    /// Per-paragraph styles, one per paragraph run in order. When non-empty the
    /// entry for a paragraph run wins over [`Self::paragraph`]; a shorter list
    /// repeats its last entry, an empty one falls back to `paragraph`.
    #[serde(default)]
    pub paragraphs: Vec<ParagraphSpec>,
    /// `EngineDict/AntiAlias` code: 0 none, 1 crisp, 2 strong, 3 smooth, 4 sharp.
    #[serde(default)]
    pub aa: Option<i64>,
    /// Area-text frame `[left top right bottom]`; absent for point text.
    #[serde(default)]
    pub frame: Option<[f32; 4]>,
    /// Area-type or type-on-a-path shape (the `Txt2` frame's curve); `None`
    /// keeps the rect in [`Self::frame`] as the layer's shape.
    #[serde(default)]
    pub shape: Option<ShapeSpec>,
    /// Text orientation (`TySh` `Ornt`): `Some(1)` vertical, `Some(0)` horizontal,
    /// `None` keeps the file's value. The reader fills it from `TySh`/EngineData.
    #[serde(default)]
    pub orientation: Option<i64>,
    /// The layer transform (`TySh` header, `[xx, xy, yx, yy, tx, ty]`); `None`
    /// keeps the file's value.
    #[serde(default)]
    pub transform: Option<[f64; 6]>,
    /// Legacy live-preview option; ignored by PSD authoring. Attach an atlas
    /// to the caller's `FaceStore` to use it for the stored pixels.
    #[serde(default)]
    pub atlas: Option<bool>,
    #[serde(default)]
    pub fractional: Option<bool>,
    /// Keep one U+000D per hard break the text ends with (the empty final
    /// paragraph a caret can sit on) instead of trimming every trailing break
    /// before authoring. The stored text then ends with the breaks, and the
    /// run lengths must cover them.
    #[serde(default)]
    pub preserve_trailing_break: bool,
}

#[cfg(test)]
mod tests {
    use super::LayerSpec;

    #[test]
    fn json_defaults_match_the_legacy_authoring_model() {
        for json in [
            r#"{"text":"Hi","runs":[{"len":3}]}"#,
            r#"{"text":"Hi","runs":[{"len":3}],"paragraph":{}}"#,
        ] {
            let spec: LayerSpec = serde_json::from_str(json).unwrap();
            assert_eq!(spec.text, "Hi");
            assert_eq!(spec.runs[0].len, 3);
            assert!(spec.runs[0].font.is_none());
            assert!(!spec.runs[0].bold);
            assert_eq!(spec.paragraph.justification, 0);
            assert!(!spec.paragraph.every_line_composer);
            assert_eq!(spec.paragraph.auto_leading, 1.2);
            assert_eq!(spec.paragraph.word_spacing, [0.8, 1.0, 1.33]);
            assert_eq!(spec.paragraph.letter_spacing, [0.0, 0.0, 0.0]);
            assert_eq!(spec.paragraph.glyph_scaling, [1.0, 1.0, 1.0]);
            assert!(spec.aa.is_none() && spec.frame.is_none());
            assert!(spec.atlas.is_none() && spec.fractional.is_none());
        }
        assert!(serde_json::from_str::<LayerSpec>(r#"{"text":"Hi","runs":[{}]}"#).is_err());
    }
}

/// A text layer's non-rectangular shape: type on a path, or area type whose
/// frame is a closed curve. Everything here comes from the `Txt2` frame
/// (`/0 /8 /0 [id] /0 /1` points, `/2` matrix, range and flip).
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ShapeSpec {
    /// The stored frame in text space, byte-for-byte as `Txt2`'
    /// `<< /1 /0 [points] /2 << /0 kind /6 [start end] /11 << /0 flip >> >> >>`
    /// expresses it; `None` means the frame rect in [`super::LayerSpec::frame`]
    /// is authoritative.
    #[serde(default)]
    pub frame: Option<FrameSpec>,
    /// The decoded curve, for callers that need geometry rather than the
    /// stored numbers (the fork's layout places glyphs on it).
    #[serde(default)]
    pub path: Option<PathSpec>,
}

/// A text layer's stored `Txt2` frame, in the exact shape the writer re-emits:
/// 8 numbers per cubic segment (text space), the frame kind, the fractional
/// segment range with the `-1.0` default sentinel, the flip flag and the stored
/// matrix (identity when the points are already in text space).
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct FrameSpec {
    #[serde(default)]
    pub points: Vec<f64>,
    #[serde(default)]
    pub kind: i64,
    #[serde(default)]
    pub range: [f64; 2],
    /// `None` keeps the stored flip value.
    #[serde(default)]
    pub flip: Option<bool>,
    /// Stored `/2 /2` when it is not the identity (read only; the writer emits
    /// identity for text-space points). `None` keeps the stored matrix.
    #[serde(default)]
    pub matrix: Option<[f64; 6]>,
}

/// Type on a path / area frame curve, in text-space points.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct PathSpec {
    /// Anchors and handles; a frame whose segment carries no handles has
    /// `in_ctrl`/`out_ctrl` equal to its anchor.
    #[serde(default)]
    pub knots: Vec<PathKnot>,
    /// Whether the frame is closed (area/box frames are, type-on-path may not).
    #[serde(default)]
    pub closed: bool,
    /// Arc offset of the text start along the path, in points.
    #[serde(default)]
    pub start: f32,
    /// Unwrapped arc end: `None` runs to the path end (open) or one lap
    /// (closed). Equal to `start` means an empty run.
    #[serde(default)]
    pub end: Option<f32>,
    /// `true` reverses the travel direction (`Txt2` `/2 /11 /0`).
    #[serde(default)]
    pub flip: bool,
    /// Frame-space to `TySh` text-space transform (`Txt2` frame `/2 /2`).
    #[serde(default)]
    pub matrix: Option<[f64; 6]>,
    /// The `Txt2` frame kind as Photoshop names it (`Some` when the frame
    /// carries one): lets a caller keep box/area/path apart.
    #[serde(default)]
    pub kind: Option<String>,
}

/// One knot of a [`PathSpec`]: an anchor with its incoming and outgoing
/// control points (equal to the anchor for a corner).
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct PathKnot {
    #[serde(default)]
    pub anchor: [f32; 2],
    #[serde(default)]
    pub in_ctrl: [f32; 2],
    #[serde(default)]
    pub out_ctrl: [f32; 2],
    /// `Some` when the frame marks the knot smooth (linked handles).
    #[serde(default)]
    pub smooth: Option<bool>,
}
