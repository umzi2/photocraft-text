//! Multi-run (per-character) styling.
//!
//! A text layer is a sequence of style runs: font, size, colour, tracking,
//! kerning, baseline shift, horizontal/vertical scale and faux styles can change
//! mid-string. This module owns the run model and the shaping that consumes it;
//! `layout` and `area` stay single-run and are reused per run.
//!
//! Run lengths are UTF-16 code units, as in EngineData's `RunLengthArray`.

use std::collections::BTreeMap;

use crate::area::Justification;
use crate::engine_data::Value;
use crate::font::Face;
use crate::font::gsub::{self, Gsub, Span};
use crate::layout::{GlyphPos, Kerning, TextStyle};
use crate::typography::faux_bold_extra_advance;

/// One style run covering `len` UTF-16 code units of the layer text.
///
/// Optional fields override the base [`TextStyle`]; `None` inherits it.
/// `sheet` keeps the parsed `StyleSheetData` so keys this model does not
/// interpret survive [`to_engine_data`] unchanged.
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
    /// so [`engine_language`] translates before writing.
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

// --------------------------------------------------------------------------- //
// Parsing
// --------------------------------------------------------------------------- //

/// Writes the layer's antialias mode (`EngineDict/AntiAlias`).
pub fn set_antialias(root: &mut Value, aa: crate::raster::AntiAlias) -> bool {
    let Some(engine) = root.get_mut("EngineDict") else {
        return false;
    };
    match engine {
        Value::Dict(entries) => {
            let value = Value::Int(aa.code());
            match entries.iter_mut().find(|(k, _)| k == "AntiAlias") {
                Some((_, slot)) => *slot = value,
                None => entries.push(("AntiAlias".to_string(), value)),
            }
            true
        }
        _ => false,
    }
}

/// Writes `EngineDict/UseFractionalGlyphWidths` (Photoshop's "Fractional Widths").
pub fn set_fractional_glyph_widths(root: &mut Value, on: bool) -> bool {
    let Some(engine) = root.get_mut("EngineDict") else {
        return false;
    };
    match engine {
        Value::Dict(entries) => {
            let value = Value::Bool(on);
            match entries
                .iter_mut()
                .find(|(k, _)| k == "UseFractionalGlyphWidths")
            {
                Some((_, slot)) => *slot = value,
                None => entries.push(("UseFractionalGlyphWidths".to_string(), value)),
            }
            true
        }
        _ => false,
    }
}

/// Writes the paragraph runs (`EngineDict/ParagraphRun`) for `paras`.
pub fn apply_paragraph_runs(root: &mut Value, paras: &[ParagraphRun]) -> bool {
    if paras.is_empty() {
        return false;
    }
    let Some(engine) = root.get_mut("EngineDict") else {
        return false;
    };
    let Some(run) = engine.get_mut("ParagraphRun") else {
        return false;
    };
    if let Some(lengths) = run.get_mut("RunLengthArray").and_then(Value::as_array_mut) {
        lengths.clear();
        lengths.extend(paras.iter().map(|p| Value::Int(p.len as i64)));
    } else {
        return false;
    }
    let Some(items) = run.get_mut("RunArray").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(template) = items.first().cloned() else {
        return false;
    };
    items.clear();
    for p in paras {
        let mut item = template.clone();
        let props = item
            .get_mut("ParagraphSheet")
            .and_then(|s| s.get_mut("Properties"))
            .and_then(Value::as_dict_mut);
        if let Some(props) = props {
            let mut put = |key: &str, value: Value| match props.iter_mut().find(|(k, _)| k == key) {
                Some((_, slot)) => *slot = value,
                None => props.push((key.to_owned(), value)),
            };
            put(
                "Justification",
                Value::Int(justification_code(p.justification)),
            );
            put(
                "FirstLineIndent",
                Value::Float(f64::from(p.first_line_indent)),
            );
            put("StartIndent", Value::Float(f64::from(p.start_indent)));
            put("EndIndent", Value::Float(f64::from(p.end_indent)));
            put("SpaceBefore", Value::Float(f64::from(p.space_before)));
            put("SpaceAfter", Value::Float(f64::from(p.space_after)));
            put("WordSpacing", triple(p.word_spacing));
            put("LetterSpacing", triple(p.letter_spacing));
            put("GlyphSpacing", triple(p.glyph_scaling));
            put("AutoLeading", Value::Float(f64::from(p.auto_leading)));
            put("EveryLineComposer", Value::Bool(p.every_line_composer));
            put("Hanging", Value::Bool(p.hanging));
            put("AutoHyphenate", Value::Bool(p.hyphenate));
            put("HyphenatedWordSize", Value::Int(p.words_longer_than));
            put("PreHyphen", Value::Int(p.pre_hyphen));
            put("PostHyphen", Value::Int(p.post_hyphen));
            put("ConsecutiveHyphens", Value::Int(p.consecutive_hyphens));
            put("Zone", Value::Float(f64::from(p.hyphen_zone)));
        }
        items.push(item);
    }
    true
}

/// Photoshop's justification code, as stored in `Justification`.
fn justification_code(j: Justification) -> i64 {
    match j {
        Justification::Left => 0,
        Justification::Right => 1,
        Justification::Center => 2,
        Justification::JustifyLastCenter => 3,
        Justification::JustifyLastRight => 4,
        Justification::JustifyAll => 5,
        Justification::JustifyLastLeft => 6,
    }
}

fn triple(values: [f32; 3]) -> Value {
    Value::Array(values.iter().map(|v| Value::Float(f64::from(*v))).collect())
}

/// Style runs of a layer, from its EngineData root (`EngineDict/StyleRun`).
/// Font indices are resolved against `ResourceDict/FontSet`. Run lengths come
/// from `RunLengthArray`; the two arrays are zipped, so a mismatch in counts
/// truncates to the shorter one.
pub fn from_engine_data(root: &Value) -> Vec<RunStyle> {
    let Some(style_run) = root.get("EngineDict").and_then(|e| e.get("StyleRun")) else {
        return Vec::new();
    };
    let fonts = font_names(root);
    let lens = run_lengths(style_run.get("RunLengthArray"));
    let items = style_run
        .get("RunArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    items
        .iter()
        .zip(lens)
        .map(|(item, len)| parse_style_run(item, len, &fonts))
        .collect()
}

/// The layer's antialiasing mode (`EngineDict/AntiAlias`), default sharp.
pub fn antialias(root: &Value) -> crate::raster::AntiAlias {
    root.get("EngineDict")
        .and_then(|e| e.get("AntiAlias"))
        .and_then(Value::as_i64)
        .and_then(crate::raster::AntiAlias::from_code)
        .unwrap_or_default()
}

/// Paragraph runs of a layer (`EngineDict/ParagraphRun`).
pub fn paragraphs_from_engine_data(root: &Value) -> Vec<ParagraphRun> {
    let Some(para_run) = root.get("EngineDict").and_then(|e| e.get("ParagraphRun")) else {
        return Vec::new();
    };
    let lens = run_lengths(para_run.get("RunLengthArray"));
    let items = para_run
        .get("RunArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    items
        .iter()
        .zip(lens)
        .map(|(item, len)| parse_paragraph_run(item, len))
        .collect()
}

fn font_names(root: &Value) -> Vec<String> {
    root.get("ResourceDict")
        .and_then(|r| r.get("FontSet"))
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|f| f.get("Name").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn run_lengths(value: Option<&Value>) -> Vec<usize> {
    value
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_i64)
        .map(|n| n.max(0) as usize)
        .collect()
}

/// Appends a font to `EngineData`'s `ResourceDict/FontSet` unless present.
pub fn ensure_engine_font(root: &mut Value, name: &str) -> Option<usize> {
    let fonts = root
        .get_mut("ResourceDict")
        .and_then(|r| r.get_mut("FontSet"))
        .and_then(Value::as_array_mut)?;
    for (i, f) in fonts.iter().enumerate() {
        if f.get("Name").and_then(Value::as_str) == Some(name) {
            return Some(i);
        }
    }
    fonts.push(Value::dict([("Name", Value::Str(name.to_owned()))]));
    Some(fonts.len() - 1)
}

/// Writes `styles` back into a layer's `EngineData` so other readers (and our
/// own renderer) see the same run split Photoshop takes from `Txt2`.
///
/// Only the keys a [`RunStyle`] carries are written; everything else is copied
/// from the first existing run. Returns false when the tree has no style runs.
pub fn apply_style_runs(root: &mut Value, styles: &[RunStyle]) -> bool {
    if styles.is_empty() {
        return false;
    }
    let Some(engine) = root.get_mut("EngineDict") else {
        return false;
    };
    let Some(style_run) = engine.get_mut("StyleRun") else {
        return false;
    };
    if let Some(lengths) = style_run
        .get_mut("RunLengthArray")
        .and_then(Value::as_array_mut)
    {
        lengths.clear();
        lengths.extend(styles.iter().map(|r| Value::Int(r.len as i64)));
    } else {
        return false;
    }
    let Some(items) = style_run.get_mut("RunArray").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(template) = items.first().cloned() else {
        return false;
    };
    items.clear();
    for run in styles {
        let mut item = template.clone();
        if let Some(sheet) = item
            .get_mut("StyleSheet")
            .and_then(|s| s.get_mut("StyleSheetData"))
            .and_then(Value::as_dict_mut)
        {
            let mut put = |key: &str, value: Value| match sheet.iter_mut().find(|(k, _)| k == key) {
                Some((_, slot)) => *slot = value,
                None => sheet.push((key.to_owned(), value)),
            };
            if let Some(i) = run.font_index {
                put("Font", Value::Int(i));
            }
            if let Some(v) = run.size {
                put("FontSize", Value::Float(f64::from(v)));
            }
            if let Some(v) = run.horizontal_scale {
                put("HorizontalScale", Value::Float(f64::from(v)));
            }
            if let Some(v) = run.vertical_scale {
                put("VerticalScale", Value::Float(f64::from(v)));
            }
            if let Some(v) = run.tracking {
                put("Tracking", Value::Int(i64::from(v)));
            }
            if let Some(v) = run.baseline_shift {
                put("BaselineShift", Value::Float(f64::from(v)));
            }
            if let Some(auto) = run.auto_leading_flag() {
                put("AutoLeading", Value::Bool(auto));
            }
            if let Some(v) = run.leading {
                put("Leading", Value::Float(f64::from(v)));
            }
            if let Some(auto) = run.auto_kerning {
                put("AutoKerning", Value::Bool(auto));
            }
            if let Some(k) = run.kerning {
                put("Kerning", Value::Int(i64::from(k)));
            }
            put("FauxBold", Value::Bool(run.faux_bold));
            put("FauxItalic", Value::Bool(run.faux_italic));
            put("Underline", Value::Bool(run.underline));
            put("Strikethrough", Value::Bool(run.strikethrough));
            put(
                "FontCaps",
                Value::Int(if run.all_caps {
                    CAPS_ALL
                } else if run.small_caps {
                    CAPS_SMALL
                } else {
                    0
                }),
            );
            if let Some(on) = run.ligatures {
                put("Ligatures", Value::Bool(on));
            }
            if let Some(on) = run.alternate_ligatures {
                put("DLigatures", Value::Bool(on));
            }
            if let Some(code) = run.language.and_then(engine_language) {
                put("Language", Value::Int(code));
            }
            if let Some(on) = run.no_break {
                put("NoBreak", Value::Bool(on));
            }
            if let Some([r, g, b]) = run.color {
                // Photoshop stores `[ alpha r g b ]`, alpha first.
                let values = Value::Array(vec![
                    Value::Float(1.0),
                    Value::Float(f64::from(r) / 255.0),
                    Value::Float(f64::from(g) / 255.0),
                    Value::Float(f64::from(b) / 255.0),
                ]);
                put(
                    "FillColor",
                    Value::dict([("Type", Value::Int(1)), ("Values", values)]),
                );
            }
        }
        items.push(item);
    }
    true
}

fn parse_style_run(item: &Value, len: usize, fonts: &[String]) -> RunStyle {
    let sheet = item
        .get("StyleSheet")
        .and_then(|s| s.get("StyleSheetData"))
        .cloned();
    let get = |key: &str| sheet.as_ref().and_then(|d| d.get(key));

    let font_index = get("Font").and_then(Value::as_i64);
    let font = font_index
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| fonts.get(i))
        .cloned();
    let caps = get("FontCaps").and_then(Value::as_i64).unwrap_or(0);

    RunStyle {
        len,
        font,
        font_index,
        size: get("FontSize").and_then(Value::as_f32),
        color: get("FillColor").and_then(color_rgb),
        tracking: get("Tracking")
            .and_then(Value::as_i64)
            .and_then(|v| i16::try_from(v).ok()),
        baseline_shift: get("BaselineShift").and_then(Value::as_f32),
        horizontal_scale: get("HorizontalScale").and_then(Value::as_f32),
        vertical_scale: get("VerticalScale").and_then(Value::as_f32),
        auto_kerning: get("AutoKerning").and_then(Value::as_bool),
        kerning: get("Kerning")
            .and_then(Value::as_i64)
            .and_then(|v| i16::try_from(v).ok()),
        leading: get("Leading").and_then(Value::as_f32),
        auto_leading: get("AutoLeading").and_then(Value::as_bool),
        faux_bold: get("FauxBold").and_then(Value::as_bool).unwrap_or(false),
        faux_italic: get("FauxItalic").and_then(Value::as_bool).unwrap_or(false),
        underline: get("Underline").and_then(Value::as_bool).unwrap_or(false),
        strikethrough: get("Strikethrough")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        all_caps: caps == CAPS_ALL,
        small_caps: caps == CAPS_SMALL,
        // Only `Txt2` carries old-style figures; `EngineData` never does.
        old_style: false,
        ligatures: get("Ligatures").and_then(Value::as_bool),
        alternate_ligatures: get("DLigatures").and_then(Value::as_bool),
        language: get("Language")
            .and_then(Value::as_i64)
            .and_then(txt2_language),
        no_break: get("NoBreak").and_then(Value::as_bool),
        sheet,
    }
}

fn parse_paragraph_run(item: &Value, len: usize) -> ParagraphRun {
    let props = item.get("ParagraphSheet").and_then(|s| s.get("Properties"));
    let get = |key: &str| props.and_then(|d| d.get(key));
    let num = |key: &str| get(key).and_then(Value::as_f32).unwrap_or(0.0);
    let triple = |key: &str, fallback: [f32; 3]| {
        get(key)
            .and_then(Value::as_array)
            .map(|a| {
                let mut out = fallback;
                for (slot, v) in out.iter_mut().zip(a.iter().filter_map(Value::as_f32)) {
                    *slot = v;
                }
                out
            })
            .unwrap_or(fallback)
    };
    let int = |key: &str, fallback: i64| get(key).and_then(Value::as_i64).unwrap_or(fallback);
    ParagraphRun {
        len,
        word_spacing: triple("WordSpacing", [0.8, 1.0, 1.33]),
        letter_spacing: triple("LetterSpacing", [0.0, 0.0, 0.0]),
        glyph_scaling: triple("GlyphScaling", [1.0, 1.0, 1.0]),
        justification: Justification::from_psd(
            get("Justification").and_then(Value::as_i64).unwrap_or(0) as i32,
        ),
        every_line_composer: matches!(get("EveryLineComposer"), Some(Value::Bool(true))),
        first_line_indent: num("FirstLineIndent"),
        start_indent: num("StartIndent"),
        end_indent: num("EndIndent"),
        space_before: num("SpaceBefore"),
        space_after: num("SpaceAfter"),
        auto_leading: get("AutoLeading").and_then(Value::as_f32).unwrap_or(1.2),
        hanging: matches!(get("Hanging"), Some(Value::Bool(true))),
        hyphenate: matches!(get("AutoHyphenate"), Some(Value::Bool(true))),
        words_longer_than: int("HyphenatedWordSize", 6),
        pre_hyphen: int("PreHyphen", 2),
        post_hyphen: int("PostHyphen", 2),
        consecutive_hyphens: int("ConsecutiveHyphens", 8),
        hyphen_zone: get("Zone").and_then(Value::as_f32).unwrap_or(36.0),
    }
}

/// Translates a `Txt2` `/6/38` language code into `EngineData`'s `Language`.
///
/// The two models normally share the id, but not always: English (UK) is `14`
/// in both while German is `42` in `Txt2` and `4` in `EngineData`
/// (`docs/txt2-map.md` §2, measured on `out/probe5/pE.psd`). `None` means this
/// id was never measured, so the mirror leaves the existing value alone rather
/// than guessing.
pub fn engine_language(txt2_code: i64) -> Option<i64> {
    Some(match txt2_code {
        0 => 0,
        14 => 14,
        42 => 4,
        _ => return None,
    })
}

/// Inverse of [`engine_language`]: the `Txt2` code for an `EngineData`
/// `Language` id, or `None` when the id was never measured.
pub fn txt2_language(engine_id: i64) -> Option<i64> {
    Some(match engine_id {
        0 => 0,
        14 => 14,
        4 => 42,
        _ => return None,
    })
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

impl ParagraphFormat {
    /// The `ParagraphSheet/Properties` keys this format owns.
    fn entries(&self) -> [(&'static str, Value); 7] {
        [
            ("AutoHyphenate", Value::Bool(self.hyphenate)),
            ("HyphenatedWordSize", Value::Int(self.words_longer_than)),
            ("PreHyphen", Value::Int(self.after_first)),
            ("PostHyphen", Value::Int(self.before_last)),
            ("ConsecutiveHyphens", Value::Int(self.limit)),
            ("Zone", Value::Float(f64::from(self.zone))),
            ("Hanging", Value::Bool(self.hanging_punctuation)),
        ]
    }
}

/// Writes hyphenation and hanging punctuation into every paragraph run's
/// `ParagraphSheet/Properties`.
///
/// `formats` is indexed by paragraph run; a single entry applies to every
/// paragraph and a short list repeats its last value, so callers that format
/// the whole layer can pass one [`ParagraphFormat`]. Returns false when the
/// tree has no paragraph runs.
pub fn apply_paragraph_formats(root: &mut Value, formats: &[ParagraphFormat]) -> bool {
    let Some(first) = formats.first().copied() else {
        return false;
    };
    let Some(engine) = root.get_mut("EngineDict") else {
        return false;
    };
    let Some(run) = engine.get_mut("ParagraphRun") else {
        return false;
    };
    let Some(items) = run.get_mut("RunArray").and_then(Value::as_array_mut) else {
        return false;
    };
    for (i, item) in items.iter_mut().enumerate() {
        let format = formats.get(i).copied().unwrap_or(first);
        let Some(props) = item
            .get_mut("ParagraphSheet")
            .and_then(|s| s.get_mut("Properties"))
            .and_then(Value::as_dict_mut)
        else {
            continue;
        };
        for (key, value) in format.entries() {
            match props.iter_mut().find(|(k, _)| k == key) {
                Some((_, slot)) => *slot = value,
                None => props.push((key.to_owned(), value)),
            }
        }
    }
    true
}

/// Reads the hyphenation and hanging punctuation of every paragraph run.
pub fn paragraph_formats_from_engine_data(root: &Value) -> Vec<ParagraphFormat> {
    let Some(para_run) = root.get("EngineDict").and_then(|e| e.get("ParagraphRun")) else {
        return Vec::new();
    };
    let items = para_run
        .get("RunArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    items
        .iter()
        .map(|item| {
            let props = item.get("ParagraphSheet").and_then(|s| s.get("Properties"));
            let get = |key: &str| props.and_then(|d| d.get(key));
            let int =
                |key: &str, fallback: i64| get(key).and_then(Value::as_i64).unwrap_or(fallback);
            let default = ParagraphFormat::default();
            ParagraphFormat {
                hyphenate: get("AutoHyphenate")
                    .and_then(Value::as_bool)
                    .unwrap_or(default.hyphenate),
                words_longer_than: int("HyphenatedWordSize", default.words_longer_than),
                after_first: int("PreHyphen", default.after_first),
                before_last: int("PostHyphen", default.before_last),
                limit: int("ConsecutiveHyphens", default.limit),
                zone: get("Zone").and_then(Value::as_f32).unwrap_or(default.zone),
                hanging_punctuation: get("Hanging")
                    .and_then(Value::as_bool)
                    .unwrap_or(default.hanging_punctuation),
            }
        })
        .collect()
}

/// `FillColor` is `<< /Type 1 /Values [ alpha r g b ] >>` with components in 0..1.
fn color_rgb(value: &Value) -> Option<[u8; 3]> {
    if value.get("Type").and_then(Value::as_i64) != Some(COLOR_TYPE_RGB) {
        return None;
    }
    let values = value.get("Values").and_then(Value::as_array)?;
    if values.len() < 4 {
        return None;
    }
    let comp = |v: &Value| (v.as_f32().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
    Some([comp(&values[1]), comp(&values[2]), comp(&values[3])])
}

// --------------------------------------------------------------------------- //
// Writing
// --------------------------------------------------------------------------- //

const CAPS_SMALL: i64 = 1;
const CAPS_ALL: i64 = 2;
const COLOR_TYPE_RGB: i64 = 1;

/// Builds the `StyleRun` dictionary for `runs`, in the layout Photoshop writes
/// (`DefaultRunData`, `RunArray`, `RunLengthArray`, `IsJoinable 2`). Insert it
/// as `EngineDict/StyleRun`. Each run starts from its own `sheet` when present,
/// so unmodelled keys are kept; otherwise from the default sheet Photoshop
/// writes for a plain run. Only fields that differ from the parsed value are
/// rewritten, so an unchanged parse writes back identically.
pub fn to_engine_data(runs: &[RunStyle]) -> Value {
    let run_array = runs
        .iter()
        .map(|run| style_run_item(&sheet_for(run)))
        .collect();
    let run_lengths = runs.iter().map(|r| Value::Int(r.len as i64)).collect();
    Value::dict([
        ("DefaultRunData", style_run_item(&[])),
        ("RunArray", Value::Array(run_array)),
        ("RunLengthArray", Value::Array(run_lengths)),
        ("IsJoinable", Value::Int(2)),
    ])
}

fn style_run_item(sheet: &[(String, Value)]) -> Value {
    Value::dict([(
        "StyleSheet",
        Value::dict([("StyleSheetData", Value::Dict(sheet.to_vec()))]),
    )])
}

fn sheet_for(run: &RunStyle) -> Vec<(String, Value)> {
    let mut d = match &run.sheet {
        Some(Value::Dict(entries)) => entries.clone(),
        _ => default_sheet(),
    };
    if let Some(i) = run.font_index {
        put(&mut d, "Font", Value::Int(i));
    }
    if let Some(s) = run.size {
        put_num(&mut d, "FontSize", s);
    }
    if let Some(c) = run.color {
        put_color(&mut d, "FillColor", c);
    }
    if let Some(t) = run.tracking {
        put(&mut d, "Tracking", Value::Int(t as i64));
    }
    if let Some(b) = run.baseline_shift {
        put_num(&mut d, "BaselineShift", b);
    }
    if let Some(h) = run.horizontal_scale {
        put_num(&mut d, "HorizontalScale", h);
    }
    if let Some(v) = run.vertical_scale {
        put_num(&mut d, "VerticalScale", v);
    }
    if let Some(a) = run.auto_kerning {
        put(&mut d, "AutoKerning", Value::Bool(a));
    }
    if let Some(k) = run.kerning {
        put(&mut d, "Kerning", Value::Int(k as i64));
    }
    if let Some(auto) = run.auto_leading_flag() {
        put(&mut d, "AutoLeading", Value::Bool(auto));
    }
    if let Some(l) = run.leading {
        put_num(&mut d, "Leading", l);
    }
    put(&mut d, "FauxBold", Value::Bool(run.faux_bold));
    put(&mut d, "FauxItalic", Value::Bool(run.faux_italic));
    put(&mut d, "Underline", Value::Bool(run.underline));
    put(&mut d, "Strikethrough", Value::Bool(run.strikethrough));
    put_caps(&mut d, run.all_caps, run.small_caps);
    if let Some(on) = run.ligatures {
        put(&mut d, "Ligatures", Value::Bool(on));
    }
    if let Some(on) = run.alternate_ligatures {
        put(&mut d, "DLigatures", Value::Bool(on));
    }
    if let Some(code) = run.language.and_then(engine_language) {
        put(&mut d, "Language", Value::Int(code));
    }
    if let Some(on) = run.no_break {
        put(&mut d, "NoBreak", Value::Bool(on));
    }
    d
}

/// `FontCaps`: 0 none, 1 small caps, 2 all caps (Aspose.PSD `FontCaps` enum).
fn put_caps(d: &mut Vec<(String, Value)>, all: bool, small: bool) {
    let current = get_int(d, "FontCaps").unwrap_or(0);
    let same = (current == CAPS_ALL) == all && (current == CAPS_SMALL) == small;
    if !same {
        let code = if all {
            CAPS_ALL
        } else if small {
            CAPS_SMALL
        } else {
            0
        };
        put(d, "FontCaps", Value::Int(code));
    }
}

fn put_color(d: &mut Vec<(String, Value)>, key: &str, rgb: [u8; 3]) {
    if d.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| color_rgb(v))
        == Some(rgb)
    {
        return;
    }
    let comp = |c: u8| Value::Float(c as f64 / 255.0);
    let color = Value::dict([
        ("Type", Value::Int(COLOR_TYPE_RGB)),
        (
            "Values",
            Value::Array(vec![
                Value::Float(1.0),
                comp(rgb[0]),
                comp(rgb[1]),
                comp(rgb[2]),
            ]),
        ),
    ]);
    put(d, key, color);
}

fn get_int(d: &[(String, Value)], key: &str) -> Option<i64> {
    d.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.as_i64())
}

fn put(d: &mut Vec<(String, Value)>, key: &str, value: Value) {
    match d.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => {
            if slot.1 != value {
                slot.1 = value;
            }
        }
        None => d.push((key.to_string(), value)),
    }
}

/// Writes a float unless the stored value already equals `v` at f32 precision.
fn put_num(d: &mut Vec<(String, Value)>, key: &str, v: f32) {
    if d.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, x)| x.as_f32())
        == Some(v)
    {
        return;
    }
    put(d, key, Value::Float(v as f64));
}

/// Sheet Photoshop writes for a plain run (`StyleSheetSet` default, black NotoSans).
fn default_sheet() -> Vec<(String, Value)> {
    let black = Value::dict([
        ("Type", Value::Int(COLOR_TYPE_RGB)),
        (
            "Values",
            Value::Array(vec![
                Value::Float(1.0),
                Value::Float(0.0),
                Value::Float(0.0),
                Value::Float(0.0),
            ]),
        ),
    ]);
    let entries: Vec<(&str, Value)> = vec![
        ("Font", Value::Int(0)),
        ("FontSize", Value::Float(12.0)),
        ("FauxBold", Value::Bool(false)),
        ("FauxItalic", Value::Bool(false)),
        ("AutoLeading", Value::Bool(true)),
        ("Leading", Value::Float(0.0)),
        ("HorizontalScale", Value::Float(1.0)),
        ("VerticalScale", Value::Float(1.0)),
        ("Tracking", Value::Int(0)),
        ("AutoKerning", Value::Bool(true)),
        ("Kerning", Value::Int(0)),
        ("BaselineShift", Value::Float(0.0)),
        ("FontCaps", Value::Int(0)),
        ("FontBaseline", Value::Int(0)),
        ("Underline", Value::Bool(false)),
        ("Strikethrough", Value::Bool(false)),
        ("Ligatures", Value::Bool(true)),
        ("DLigatures", Value::Bool(false)),
        ("BaselineDirection", Value::Int(1)),
        ("Tsume", Value::Float(0.0)),
        ("StyleRunAlignment", Value::Int(2)),
        ("Language", Value::Int(0)),
        ("NoBreak", Value::Bool(false)),
        ("FillColor", black.clone()),
        ("StrokeColor", black),
        ("YUnderline", Value::Int(1)),
        ("HindiNumbers", Value::Bool(false)),
        ("Kashida", Value::Int(1)),
    ];
    entries
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

// --------------------------------------------------------------------------- //
// Fonts and shaping
// --------------------------------------------------------------------------- //

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
    /// Index into the `runs` slice passed to [`shape_runs_detailed`].
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
    /// Index into the `runs` slice passed to [`shape_runs_attrs`].
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

/// Pen positions for a multi-run string. Same semantics as
/// [`crate::layout::shape`] (advance = pen delta to the next glyph), across
/// runs. Kerning does not cross run boundaries.
///
/// Pipeline: caps mapping per character, then `liga` (and `dlig` when enabled)
/// over adjacent glyphs sharing font, feature flags and small-caps state. A
/// ligature may span runs with identical font and features; it takes the first
/// component's run and attributes. Then advances.
///
/// Panics if the run lengths do not cover the text in UTF-16 units, or a run
/// names a font missing from `faces`: both are caller errors.
pub fn shape_runs_attrs(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
) -> Vec<(GlyphPos, GlyphAttrs)> {
    let text_units = text.encode_utf16().count();
    let run_units: usize = runs.iter().map(|r| r.len).sum();
    assert_eq!(
        run_units, text_units,
        "run lengths cover {run_units} UTF-16 units, text has {text_units}"
    );

    let items = collect_items(faces, text, runs);
    let spans = substitute(faces, &items);
    layout_glyphs(faces, runs, style, &items, &spans)
}

pub fn shape_runs(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
) -> Vec<GlyphPos> {
    shape_runs_attrs(faces, text, runs, style)
        .into_iter()
        .map(|(g, _)| g)
        .collect()
}

/// Like [`shape_runs`], but keeps the run index, colour and vertical scale.
pub fn shape_runs_detailed(
    faces: &FontSet<'_>,
    text: &str,
    runs: &[RunStyle],
    style: &TextStyle,
) -> Vec<ShapedGlyph> {
    shape_runs_attrs(faces, text, runs, style)
        .into_iter()
        .map(|(glyph, a)| ShapedGlyph {
            glyph,
            run: a.run,
            vertical_scale: a.vertical_scale,
            color: runs[a.run].color,
        })
        .collect()
}

/// One glyph before substitution.
struct Item<'a> {
    gid: u16,
    /// Character after caps mapping.
    ch: char,
    run: usize,
    small: bool,
    /// Font name key into the `FontSet`.
    face: &'a str,
    ligatures: bool,
    dligatures: bool,
}

impl Item<'_> {
    fn shares_shaping(&self, other: &Item<'_>) -> bool {
        self.face == other.face
            && self.ligatures == other.ligatures
            && self.dligatures == other.dligatures
            && self.small == other.small
    }
}

fn collect_items<'a>(faces: &'a FontSet<'_>, text: &str, runs: &'a [RunStyle]) -> Vec<Item<'a>> {
    let mut items = Vec::with_capacity(text.len());
    for (run_index, ch) in char_runs(text, runs) {
        if ch == '\r' || ch == '\n' {
            continue;
        }
        let run = &runs[run_index];
        let name = face_name(faces, run);
        let face = faces.get(name).expect("face_name validated the font");
        let (ligatures, dligatures) = ligature_flags(run);
        let mapped: Vec<(char, bool)> = if run.all_caps {
            ch.to_uppercase().map(|u| (u, false)).collect()
        } else {
            vec![(ch, false)]
        };
        for (c, all_cap) in mapped {
            let Some(mut gid) = face.glyph_index(c) else {
                continue;
            };
            let mut small = false;
            if run.small_caps && !all_cap {
                // Photoshop lays small caps out with the face's real `smcp`
                // design, not a scaled capital: the advances measured from a
                // 26 pt capture sheet's repeated-glyph lines are the `*.sc`
                // advances (`m.sc` 736/1000 em -> 19.14 pt vs 19.10 measured,
                // `a.sc` 528 -> 13.73 vs 13.75, ...), while `capital * 0.8`
                // drifts by up to 0.55 pt per glyph. The *bitmaps* also come
                // from the face's small caps design; a `small_caps` atlas
                // capture (see `Atlas::small_caps`) supplies Photoshop's own
                // raster for them. Faces without `smcp` fall back to the
                // synthetic scaled capital.
                if let Some(sub) = substitute_feature(face, c, *b"smcp", gid) {
                    gid = sub;
                } else if c.is_lowercase() {
                    if let Some(upper) = c.to_uppercase().next().and_then(|u| face.glyph_index(u)) {
                        gid = upper;
                        small = true;
                    }
                }
            }
            if run.old_style && c.is_numeric() {
                // Photoshop's old-style figures are proportional: `pnum`
                // (`zero` -> `zero.lf`) then `onum` (`zero.lf` -> `zero.osf`).
                // `onum` alone selects the *tabular* `.tosf` set (547/1000 em
                // for every digit), which lays the figures out up to 3.8 pt
                // per digit away from Photoshop (measured: `1` 10.40 pt =
                // `one.osf` 399/1000 em, `0` 15.25 = `zero.osf` 587/1000).
                let step = substitute_feature(face, c, *b"pnum", gid).unwrap_or(gid);
                if let Some(subst) = substitute_feature(face, c, *b"onum", step) {
                    gid = subst;
                } else {
                    gid = step;
                }
            }
            if std::env::var("PHOTOCRAFT_TRACE").is_ok_and(|v| v == "12") {
                eprintln!(
                    "item: ch={:?} gid={:05X} small={} old={}",
                    c, gid, small, run.old_style
                );
            }
            items.push(Item {
                gid,
                ch: c,
                run: run_index,
                small,
                face: name,
                ligatures,
                dligatures,
            });
        }
    }
    items
}

/// `(Ligatures, DLigatures)` from the run's sheet. Defaults match Photoshop's
/// `default_sheet` ([INFERENCE] for runs without a sheet).
fn ligature_flags(run: &RunStyle) -> (bool, bool) {
    let flag = |key: &str, default: bool| {
        run.sheet
            .as_ref()
            .and_then(|s| s.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    (flag("Ligatures", true), flag("DLigatures", false))
}

/// Font name key of a run, asserting it exists in the set.
fn face_name<'a>(faces: &'a FontSet<'_>, run: &'a RunStyle) -> &'a str {
    match &run.font {
        Some(name) => {
            assert!(
                faces.get(name).is_some(),
                "run font {name:?} is not in the FontSet"
            );
            name
        }
        None => faces.default_name(),
    }
}

/// One-glyph feature substitution (`smcp`, `c2sc`, `onum`): the replacement
/// glyph of the first lookup that changes `gid`, or `None` when the face does
/// not implement the feature for this glyph.
fn substitute_feature(
    face: &crate::font::Face<'_>,
    ch: char,
    tag: [u8; 4],
    gid: u16,
) -> Option<u16> {
    let data = face.table(gsub::TAG_GSUB)?;
    let table = Gsub::parse(data).ok()?;
    let mut span = Span {
        gid,
        start: 0,
        len: 1,
    };
    let mut changed = false;
    for lookup in table.lookups(&script_tags(ch), &[tag]) {
        let out = table.apply(lookup, &[span]);
        if let Some(first) = out.first() {
            if first.gid != span.gid {
                span = *first;
                changed = true;
            }
        }
    }
    changed.then_some(span.gid)
}

/// Script tags tried in order for a group: Latin letters use `latn`, others
/// `DFLT`. The other tag is the fallback.
fn script_tags(ch: char) -> [[u8; 4]; 2] {
    const LATN: [u8; 4] = *b"latn";
    const DFLT: [u8; 4] = *b"DFLT";
    if ch.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&ch) {
        [LATN, DFLT]
    } else {
        [DFLT, LATN]
    }
}

/// Ligature substitution over the whole item list, one shaping group at a
/// time (adjacent items sharing font, flags and small-caps state).
fn substitute(faces: &FontSet<'_>, items: &[Item<'_>]) -> Vec<Span> {
    let mut out = Vec::with_capacity(items.len());
    let mut start = 0;
    while start < items.len() {
        let head = &items[start];
        let len = items[start..]
            .iter()
            .take_while(|it| it.shares_shaping(head))
            .count();
        out.extend(substitute_group(faces, &items[start..start + len], start));
        start += len;
    }
    out
}

fn substitute_group(faces: &FontSet<'_>, group: &[Item<'_>], base: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = group
        .iter()
        .enumerate()
        .map(|(i, it)| Span {
            gid: it.gid,
            start: base + i,
            len: 1,
        })
        .collect();
    let head = &group[0];
    let mut features: Vec<[u8; 4]> = Vec::with_capacity(2);
    if head.ligatures {
        features.push(*b"liga");
    }
    if head.dligatures {
        features.push(*b"dlig");
    }
    if features.is_empty() {
        return spans;
    }
    let Some(table) = faces
        .get(head.face)
        .and_then(|f| f.table(gsub::TAG_GSUB))
        .and_then(|d| Gsub::parse(d).ok())
    else {
        return spans;
    };
    for lookup in table.lookups(&script_tags(head.ch), &features) {
        spans = table.apply(lookup, &spans);
    }
    spans
}

/// Advances, kerning, tracking, faux bold and attribute assembly.
fn layout_glyphs(
    faces: &FontSet<'_>,
    runs: &[RunStyle],
    style: &TextStyle,
    items: &[Item<'_>],
    spans: &[Span],
) -> Vec<(GlyphPos, GlyphAttrs)> {
    let mut out: Vec<(GlyphPos, GlyphAttrs)> = Vec::with_capacity(spans.len());
    let mut pen = 0f32;
    let mut prev: Option<(usize, u16)> = None;
    for span in spans {
        let item = &items[span.start];
        let run = &runs[item.run];
        let face = faces.get(item.face).expect("face_name validated the font");
        let rs = resolve(run, style);
        let upm = face.units_per_em() as f32;
        let size_scale = if item.small { SMALL_CAPS_SCALE } else { 1.0 };
        let glyph_scale = rs.size / upm * size_scale;
        let hscale = rs.horizontal_scale;
        let kern = match prev {
            Some((run_prev, prev_gid)) if run_prev == item.run => match rs.kerning {
                Kerning::Metrics => face.kern_pair(prev_gid, span.gid) as f32 * glyph_scale,
                Kerning::Manual(v) => v as f32 / 1000.0 * rs.size,
                Kerning::Optical => crate::optical::optical_pair(face, prev_gid, span.gid, rs.size),
            },
            _ => 0.0,
        };
        pen += kern * hscale;
        let tracking = rs.tracking as f32 / 1000.0 * rs.size;
        let extra = if run.faux_bold {
            faux_bold_extra_advance(rs.size)
        } else {
            0.0
        };
        // Tracking is a horizontal step, so it follows the horizontal scale
        // (measured on `10_all_features.psd` layer `03 tracking baseline
        // scales`: 22 pt, hscale 1.5, tracking 300 gives a 304 px ink width;
        // unscaled predicts 268).
        let advance =
            face.advance(span.gid) as f32 * glyph_scale * hscale + tracking * hscale + extra;
        let glyph = GlyphPos {
            gid: span.gid,
            ch: item.ch,
            x: pen,
            y: rs.baseline_shift,
            advance,
            nominal_advance: advance,
        };
        let attrs = GlyphAttrs {
            run: item.run,
            color: run.color.unwrap_or([0, 0, 0]),
            // Vertical scale stretches the glyph in y only: it must not compose
            // with the horizontal scale, or a 50% horizontal squeeze would also
            // halve the glyph height.
            vertical_scale: run.vertical_scale.unwrap_or(1.0) * size_scale,
            x_scale: size_scale,
            faux_italic: run.faux_italic,
            leading: rs.leading_pt(),
        };
        out.push((glyph, attrs));
        pen += advance;
        prev = Some((item.run, span.gid));
    }
    // advance = pen delta to the next glyph (includes the next glyph's kern).
    for i in 0..out.len().saturating_sub(1) {
        let next_x = out[i + 1].0.x;
        out[i].0.advance = next_x - out[i].0.x;
    }
    out
}

/// `(char after caps mapping, run index)` for every source character.
fn char_runs(text: &str, runs: &[RunStyle]) -> Vec<(usize, char)> {
    let mut out = Vec::with_capacity(text.len());
    let mut index = 0usize;
    let mut remaining = runs.first().map_or(0, |r| r.len);
    for ch in text.chars() {
        while remaining == 0 && index + 1 < runs.len() {
            index += 1;
            remaining = runs[index].len;
        }
        remaining = remaining.saturating_sub(ch.len_utf16());
        out.push((index, ch));
    }
    out
}

impl RunStyle {
    /// `AutoLeading` for this run: an explicit leading with no flag means auto
    /// is off, which is what Photoshop's own sheet carries.
    pub fn auto_leading_flag(&self) -> Option<bool> {
        self.auto_leading.or(self.leading.map(|_| false))
    }
}

/// Effective per-run [`TextStyle`] (baseline shift in points, positive up).
fn resolve(run: &RunStyle, base: &TextStyle) -> TextStyle {
    TextStyle {
        size: run.size.unwrap_or(base.size),
        tracking: run.tracking.unwrap_or(base.tracking),
        kerning: kerning_for(run, base.kerning),
        horizontal_scale: run.horizontal_scale.unwrap_or(base.horizontal_scale),
        baseline_shift: run.baseline_shift.unwrap_or(base.baseline_shift),
        // Auto leading (explicitly, or implied by the absence of a leading)
        // steps by `size x paragraph fraction`; a manual leading overrides it.
        leading: match run.auto_leading_flag() {
            Some(true) => None,
            _ => run.leading.or(base.leading),
        },
        auto_leading: base.auto_leading,
    }
}

fn kerning_for(run: &RunStyle, base: Kerning) -> Kerning {
    match (run.auto_kerning, run.kerning) {
        (Some(true), _) => Kerning::Metrics,
        (Some(false), manual) => Kerning::Manual(manual.unwrap_or(0)),
        (None, Some(manual)) => Kerning::Manual(manual),
        (None, None) => base,
    }
}
