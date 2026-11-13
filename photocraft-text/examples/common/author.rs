//! Shared PSD authoring: write text, style runs, paragraph formatting, the
//! layer's renderer settings, and the pixels that go with them, into a template
//! PSD. Used by `examples/author_multi` and the live editor's PSD export.
//!
//! The copies must agree or Photoshop shows one thing and other readers
//! another:
//!
//! 1. `Txt2` text + paragraph run table (`txt2::replace_text`),
//! 2. `Txt2` style run table: lengths, per-run properties, font indices, fill
//!    paints, and an emptied layout cache,
//! 3. `Txt2` paragraph properties and the layer's antialias code,
//! 4. `TySh` `EngineData`: the same split, fonts, paragraphs, antialias, text,
//! 5. the layer's prerendered pixels, re-rendered from that EngineData.

use std::error::Error;

use photocraft_psd::PsdFile;
use photocraft_text::raster::AntiAlias;
use photocraft_text::runs::{self, ParagraphRun, RunStyle};
use photocraft_text::txt2::{self, ParagraphProperty, RunProperty, Value as TxtValue};

#[path = "pixels.rs"]
mod pixels;

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
    /// Render through the captured Photoshop atlas (render only, not stored).
    #[serde(default)]
    pub atlas: Option<bool>,
    #[serde(default)]
    pub fractional: Option<bool>,
}

/// Rewrites `file`'s text layer from `spec`.
pub fn author(file: &mut PsdFile, spec: &LayerSpec) -> Result<(), Box<dyn Error>> {
    let text = normalize(&spec.text);
    let units = text.encode_utf16().count();
    if spec.runs.is_empty() {
        return Err("no runs".into());
    }
    let covered: usize = spec.runs.iter().map(|r| r.len).sum();
    if covered != units + 1 {
        return Err(format!(
            "runs cover {covered} units, text + terminator is {}",
            units + 1
        )
        .into());
    }

    // 1. Text and paragraph runs.
    {
        let block = file
            .global_blocks
            .iter_mut()
            .find(|b| &b.key == b"Txt2")
            .ok_or("no Txt2 block")?;
        block.data = txt2::replace_text(&block.data, &text)?;
        block.padding = None;
    }

    // 2. The style run table Photoshop reads; cache emptied so it recomposes.
    let block = file
        .global_blocks
        .iter_mut()
        .find(|b| &b.key == b"Txt2")
        .ok_or("no Txt2 block")?;
    let mut data = block.data.clone();
    data = txt2::set_run_layout(&data, &runs_len(&spec.runs))?;
    let mut font_index = Vec::new();
    for run in &spec.runs {
        font_index.push(match &run.font {
            Some(name) => {
                let (next, index) = txt2::ensure_font(&data, name)?;
                data = next;
                Some(index)
            }
            None => None,
        });
    }
    for (i, run) in spec.runs.iter().enumerate() {
        for (prop, value) in style_of(run) {
            data = txt2::set_run_property(&data, i, prop, &value)?;
        }
        if let Some(index) = font_index[i] {
            data = txt2::set_run_font_index(&data, i, index)?;
        }
        if let Some([r, g, b]) = run.color {
            data = txt2::set_run_fill_color(
                &data,
                i,
                [
                    1.0,
                    f32::from(r) / 255.0,
                    f32::from(g) / 255.0,
                    f32::from(b) / 255.0,
                ],
            )?;
        }
    }

    // 3. Paragraph properties, per paragraph run, and the layer antialias.
    let para = &spec.paragraph;
    let count = txt2::paragraph_count(&data)?;
    for i in 0..count {
        data = txt2::set_paragraph_property(
            &data,
            i,
            ParagraphProperty::Justification,
            &TxtValue::Int(para.justification),
        )?;
        for (prop, value) in [
            (ParagraphProperty::FirstLineIndent, para.first_line_indent),
            (ParagraphProperty::StartIndent, para.start_indent),
            (ParagraphProperty::EndIndent, para.end_indent),
            (ParagraphProperty::SpaceBefore, para.space_before),
            (ParagraphProperty::SpaceAfter, para.space_after),
            (ParagraphProperty::AutoLeading, para.auto_leading),
        ] {
            data = txt2::set_paragraph_property(&data, i, prop, &TxtValue::Real(f64::from(value)))?;
        }
        data = txt2::set_paragraph_property(
            &data,
            i,
            ParagraphProperty::EveryLineComposer,
            &TxtValue::Bool(para.every_line_composer),
        )?;
        for (prop, values) in [
            (ParagraphProperty::WordSpacing, para.word_spacing),
            (ParagraphProperty::LetterSpacing, para.letter_spacing),
            (ParagraphProperty::GlyphScaling, para.glyph_scaling),
        ] {
            data = txt2::set_paragraph_range(&data, i, prop, values)?;
        }
    }
    if let Some(code) = spec.aa {
        data = txt2::set_antialias(&data, code)?;
    }
    if let Some([left, top, right, bottom]) = spec.frame {
        data = txt2::set_frame_geometry(
            &data,
            txt2::FrameGeometry {
                left,
                top,
                right,
                bottom,
            },
        )?;
    }
    data = txt2::clear_cache(&data, 0)?;
    block.data = data;
    block.padding = None;

    // 4. TySh EngineData: fonts, runs, paragraphs, antialias, text.
    let mut styles = run_styles(&spec.runs);
    let paragraphs = paragraph_runs(&text, para);
    let layer = file
        .layers_mut()
        .iter_mut()
        .find(|l| l.blocks.iter().any(|b| &b.key == b"TySh"))
        .ok_or("no text layer")?;
    let tysh = layer
        .blocks
        .iter_mut()
        .find(|b| &b.key == b"TySh")
        .ok_or("no TySh block")?;
    tysh.data = pixels::with_engine_data(&tysh.data, |ed| {
        for (i, run) in spec.runs.iter().enumerate() {
            if let Some(name) = &run.font {
                styles[i].font_index = runs::ensure_engine_font(ed, name).map(|v| v as i64);
            }
        }
        runs::apply_style_runs(ed, &styles);
        runs::apply_paragraph_runs(ed, &paragraphs);
        if let Some(code) = spec.aa.and_then(AntiAlias::from_code) {
            runs::set_antialias(ed, code);
        }
        if let Some(on) = spec.fractional {
            runs::set_fractional_glyph_widths(ed, on);
        }
        if let Some(photocraft_text::engine_data::Value::Dict(entries)) =
            ed.get_mut("EngineDict").and_then(|d| d.get_mut("Editor"))
        {
            let text = photocraft_text::engine_data::Value::Str(format!("{text}\r"));
            match entries.iter_mut().find(|(k, _)| k == "Text") {
                Some((_, value)) => *value = text,
                None => entries.push(("Text".to_string(), text)),
            }
        }
    })?;
    tysh.padding = None;

    // 5. The prerendered pixels, from the same EngineData.
    pixels::refresh_text_layer(file, &text)?;
    Ok(())
}

/// Normalises a PSD paragraph break (U+000D) out of editor input (LF or CRLF).
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\r")
        .replace('\n', "\r")
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

/// One paragraph run per paragraph, each owning its break, carrying the text's
/// character count so the EngineData run table matches the text.
fn paragraph_runs(text: &str, para: &ParagraphSpec) -> Vec<ParagraphRun> {
    text.split('\r')
        .map(|p| ParagraphRun {
            len: p.encode_utf16().count() + 1,
            word_spacing: para.word_spacing,
            letter_spacing: para.letter_spacing,
            glyph_scaling: para.glyph_scaling,
            justification: photocraft_text::area::Justification::from_psd(
                para.justification as i32,
            ),
            every_line_composer: para.every_line_composer,
            first_line_indent: para.first_line_indent,
            start_indent: para.start_indent,
            end_indent: para.end_indent,
            space_before: para.space_before,
            space_after: para.space_after,
            auto_leading: para.auto_leading,
            ..ParagraphRun::default()
        })
        .collect()
}

fn runs_len(spec: &[RunSpec]) -> Vec<i64> {
    spec.iter().map(|r| r.len as i64).collect()
}

fn run_styles(spec: &[RunSpec]) -> Vec<RunStyle> {
    spec.iter()
        .map(|r| RunStyle {
            len: r.len,
            font: r.font.clone(),
            size: r.size,
            leading: r.leading,
            auto_leading: r.auto_leading,
            tracking: r.tracking,
            baseline_shift: r.baseline_shift,
            horizontal_scale: r.horizontal_scale,
            vertical_scale: r.vertical_scale,
            color: r.color,
            auto_kerning: r.auto_kerning,
            kerning: r.kerning,
            faux_bold: r.bold,
            faux_italic: r.italic,
            underline: r.underline,
            strikethrough: r.strikethrough,
            all_caps: r.all_caps,
            small_caps: r.small_caps,
            ..RunStyle::default()
        })
        .collect()
}

fn style_of(run: &RunSpec) -> Vec<(RunProperty, TxtValue)> {
    let mut props = Vec::new();
    if let Some(v) = run.size {
        props.push((RunProperty::FontSize, TxtValue::Real(f64::from(v))));
    }
    if let Some(v) = run.leading {
        props.push((RunProperty::AutoLeading, TxtValue::Bool(false)));
        props.push((RunProperty::Leading, TxtValue::Real(f64::from(v))));
    } else if let Some(auto) = run.auto_leading {
        props.push((RunProperty::AutoLeading, TxtValue::Bool(auto)));
    }
    if let Some(v) = run.tracking {
        props.push((RunProperty::Tracking, TxtValue::Int(i64::from(v))));
    }
    if let Some(v) = run.baseline_shift {
        props.push((RunProperty::BaselineShift, TxtValue::Real(f64::from(v))));
    }
    if let Some(v) = run.horizontal_scale {
        props.push((RunProperty::HorizontalScale, TxtValue::Real(f64::from(v))));
    }
    if let Some(v) = run.vertical_scale {
        props.push((RunProperty::VerticalScale, TxtValue::Real(f64::from(v))));
    }
    props
}
