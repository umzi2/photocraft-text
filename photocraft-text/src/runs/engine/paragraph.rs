use crate::area::Justification;
use crate::engine_data::Value;

use super::super::model::{ParagraphFormat, ParagraphRun};

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

/// Paragraph runs of a layer (`EngineDict/ParagraphRun`), resolved through the
/// `ParagraphSheetSet` chain (a run's `Properties` → its named style sheet →
/// `TheNormalParagraphSheet`).
pub fn paragraphs_from_engine_data(root: &Value) -> Vec<ParagraphRun> {
    super::parse::resolved_paragraphs(root)
}

/// One paragraph run from its own `Properties` and its inherited ones.
pub(super) fn parse_paragraph_run(
    props: Option<&Value>,
    len: usize,
    base: &[Option<&Value>],
) -> ParagraphRun {
    let get = |key: &str| {
        props
            .and_then(|d| d.get(key))
            .or_else(|| base.iter().flatten().find_map(|b| b.get(key)))
    };
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
        // Photoshop's key is `GlyphSpacing` (the writer above writes it); older
        // EngineData from other writers spells it `GlyphScaling`, so accept both.
        glyph_scaling: {
            let spacing = triple("GlyphSpacing", [f32::NAN; 3]);
            if spacing.iter().all(|v| v.is_finite()) {
                spacing
            } else {
                triple("GlyphScaling", [1.0, 1.0, 1.0])
            }
        },
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
