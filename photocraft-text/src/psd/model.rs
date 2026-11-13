//! JSON-compatible PSD authoring models.

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
    /// Character flags. Photoshop's `Txt2` model has no measured slot for these,
    /// so they reach `TySh` `EngineData` and the stored pixels only; a
    /// re-layout in Photoshop recomposes from `Txt2` and drops them.
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strikethrough: bool,
    #[serde(default)]
    pub all_caps: bool,
    #[serde(default)]
    pub small_caps: bool,
}

/// Paragraph formatting for the whole layer.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ParagraphSpec {
    /// Photoshop's code: 0 left, 1 right, 2 centre, 3 justify-last-centre,
    /// 4 justify-last-right, 5 justify all, 6 justify-last-left.
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
        }
    }
}

/// A whole layer: the text, its run split, paragraph formatting and renderer
/// settings. This is the JSON the live editor posts.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LayerSpec {
    pub text: String,
    #[serde(default)]
    pub runs: Vec<RunSpec>,
    #[serde(default)]
    pub paragraph: ParagraphSpec,
    /// `EngineDict/AntiAlias` code: 0 none, 1 crisp, 2 strong, 3 smooth, 4 sharp.
    #[serde(default)]
    pub aa: Option<i64>,
    /// Area-text frame `[left top right bottom]`; absent for point text.
    #[serde(default)]
    pub frame: Option<[f32; 4]>,
    /// Legacy live-preview option; ignored by PSD authoring. Attach an atlas
    /// to the caller's `FaceStore` to use it for the stored pixels.
    #[serde(default)]
    pub atlas: Option<bool>,
    #[serde(default)]
    pub fractional: Option<bool>,
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
