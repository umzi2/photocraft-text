use crate::engine_data::Value;

use super::super::model::{RunFeature, RunStyle};
use super::{
    CAPS_ALL, CAPS_SMALL, OPENTYPE_KEYS, color_rgb, font_names, run_lengths, txt2_language,
};

/// Style runs of a layer, from its EngineData root (`EngineDict/StyleRun`),
/// resolved the way Photoshop reads them.
///
/// A run's own `StyleSheetData` holds only what it overrides; the rest comes
/// from its sheets (#3795):
/// - a paragraph run: its `Properties` → the `ParagraphSheetSet` entry its
///   `Parent` or `Name` names → `TheNormalParagraphSheet`;
/// - a character run: its `StyleSheetData` → the `StyleSheetSet` entry its
///   `Parent` or `Name` names → the default character style of its paragraph
///   (the paragraph run's `DefaultStyleSheet`, else its paragraph style's) →
///   `TheNormalStyleSheet`.
///
/// Missing or out-of-range indices fall through to the next level, then to the
/// defaults. Pair kerning (`Kerning` on a run is the value of the space *after*
/// each character) is expanded to one run per kerning change, as Photoshop's
/// reader does.
pub fn from_engine_data(root: &Value) -> Vec<RunStyle> {
    let text = root
        .get("EngineDict")
        .and_then(|e| e.get("Editor"))
        .and_then(|e| e.get("Text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    from_engine_data_with_text(root, text)
}

/// [`from_engine_data`] over `text`, the layer's text as Photoshop's editor
/// holds it (`\r` breaks, trailing break included). Pass it when the caller
/// knows the text exactly; the pair-kerning split needs the character offsets.
pub fn from_engine_data_with_text(root: &Value, text: &str) -> Vec<RunStyle> {
    let Some(engine) = root.get("EngineDict") else {
        return Vec::new();
    };
    let Some(style_run) = engine.get("StyleRun") else {
        return Vec::new();
    };
    let fonts = font_names(root);
    let res = root.get("ResourceDict");
    let style_set: &[Value] = res
        .and_then(|r| r.get("StyleSheetSet"))
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let para_set: &[Value] = res
        .and_then(|r| r.get("ParagraphSheetSet"))
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let index = |key: &str| {
        res.and_then(|r| r.get(key))
            .and_then(Value::as_i64)
            .and_then(|i| usize::try_from(i).ok())
    };
    let normal_style = index("TheNormalStyleSheet")
        .and_then(|i| style_set.get(i))
        .or_else(|| style_set.first());
    let normal_para = index("TheNormalParagraphSheet")
        .and_then(|i| para_set.get(i))
        .or_else(|| para_set.first());

    let lens = run_lengths(style_run.get("RunLengthArray"));
    let items = style_run
        .get("RunArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);

    // Paragraph runs first: a character run's defaults depend on its paragraph.
    // Their `Properties` are inherited from the paragraph sheets; their
    // `DefaultStyleSheet` gives the paragraph's default character style.
    let prun = engine.get("ParagraphRun");
    let plens = prun
        .map(|r| run_lengths(r.get("RunLengthArray")))
        .unwrap_or_default();
    let psheets = paragraph_sheets(prun);
    let para_bases = paragraph_bases(psheets, para_set, normal_para, style_set);
    // The paragraph run each character run starts in (UTF-16 offsets).
    let para_ends: Vec<f64> = plens
        .iter()
        .scan(0.0f64, |end, n| {
            *end += *n as f64;
            Some(*end)
        })
        .collect();
    let para_of = |at: f64| -> Option<usize> {
        let i = para_ends.partition_point(|&end| end <= at);
        if i < para_ends.len() {
            Some(i)
        } else {
            plens.len().checked_sub(1)
        }
    };

    let mut at = 0f64;
    let mut incoming: Vec<(usize, bool, f32)> = Vec::with_capacity(lens.len());
    let mut runs: Vec<RunStyle> = Vec::with_capacity(lens.len());
    for (item, len) in items.iter().zip(&lens) {
        let own = item.get("StyleSheet");
        let own_data = own.and_then(|s| s.get("StyleSheetData"));
        let own_sheet = own
            .and_then(|sheet| named(style_set, sheet))
            .and_then(|i| style_set.get(i));
        let para_default = para_of(at)
            .and_then(|p| para_bases.get(p))
            .and_then(|(_, c)| *c);
        let bases = [
            own_sheet.and_then(|s| s.get("StyleSheetData")),
            para_default.and_then(|s| s.get("StyleSheetData")),
            normal_style.and_then(|s| s.get("StyleSheetData")),
        ];
        let data = own_data.unwrap_or(&Value::Bool(false));
        let auto = lookup(data, &bases, "AutoKerning").and_then(Value::as_bool) != Some(false);
        let kern = lookup(data, &bases, "Kerning")
            .and_then(Value::as_f32)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        incoming.push((*len, auto, kern));
        runs.push(parse_style_run(data, *len, &fonts, &bases));
        at += *len as f64;
    }
    apply_pair_kerning(text, runs, &incoming)
}

/// The `ParagraphSheet` dictionaries of every paragraph run.
fn paragraph_sheets(run: Option<&Value>) -> Vec<&Value> {
    run.and_then(|r| r.get("RunArray"))
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| item.get("ParagraphSheet"))
        .collect()
}

/// The inherited `Properties` layers of each paragraph run and its
/// `DefaultStyleSheet` (the paragraph's default character style).
fn paragraph_bases<'a>(
    psheets: Vec<&'a Value>,
    para_set: &'a [Value],
    normal_para: Option<&'a Value>,
    style_set: &'a [Value],
) -> Vec<([Option<&'a Value>; 2], Option<&'a Value>)> {
    psheets
        .iter()
        .map(|sheet| {
            let style = named(para_set, sheet).and_then(|i| para_set.get(i));
            let base = [
                style.and_then(|s| s.get("Properties")),
                normal_para.and_then(|s| s.get("Properties")),
            ];
            let char_default = [Some(*sheet), style].into_iter().flatten().find_map(|s| {
                s.get("DefaultStyleSheet")
                    .and_then(Value::as_i64)
                    .and_then(|i| usize::try_from(i).ok())
                    .and_then(|i| style_set.get(i))
            });
            (base, char_default)
        })
        .collect()
}

/// [`from_engine_data`]'s paragraph side: every paragraph run with its
/// properties resolved through the `ParagraphSheetSet` chain.
pub(super) fn resolved_paragraphs(root: &Value) -> Vec<super::super::model::ParagraphRun> {
    let Some(engine) = root.get("EngineDict") else {
        return Vec::new();
    };
    let Some(prun) = engine.get("ParagraphRun") else {
        return Vec::new();
    };
    let res = root.get("ResourceDict");
    let style_set: &[Value] = res
        .and_then(|r| r.get("StyleSheetSet"))
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let para_set: &[Value] = res
        .and_then(|r| r.get("ParagraphSheetSet"))
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let index = |key: &str| {
        res.and_then(|r| r.get(key))
            .and_then(Value::as_i64)
            .and_then(|i| usize::try_from(i).ok())
    };
    let normal_para = index("TheNormalParagraphSheet")
        .and_then(|i| para_set.get(i))
        .or_else(|| para_set.first());
    let psheets = paragraph_sheets(Some(prun));
    let bases = paragraph_bases(psheets, para_set, normal_para, style_set);
    let lens = run_lengths(prun.get("RunLengthArray"));
    let items = prun
        .get("RunArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    items
        .iter()
        .zip(&lens)
        .zip(&bases)
        .map(|((item, len), (base, _))| {
            let sheet = item.get("ParagraphSheet");
            super::paragraph::parse_paragraph_run(
                sheet.and_then(|s| s.get("Properties")),
                *len,
                base,
            )
        })
        .collect()
}

/// The `StyleSheetSet`/`ParagraphSheetSet` entry a run's sheet names: its
/// `Parent` index when it is in range, else the entry with its `Name`.
fn named(set: &[Value], sheet: &Value) -> Option<usize> {
    if let Some(i) = sheet
        .get("Parent")
        .and_then(Value::as_i64)
        .and_then(|i| usize::try_from(i).ok())
        .filter(|i| *i < set.len())
    {
        return Some(i);
    }
    let name = sheet.get("Name").and_then(Value::as_str)?;
    set.iter()
        .position(|s| s.get("Name").and_then(Value::as_str) == Some(name))
}

/// The first of `run`'s or `bases`' values for `key`.
fn lookup<'a>(run: &'a Value, bases: &[Option<&'a Value>], key: &str) -> Option<&'a Value> {
    run.get(key)
        .or_else(|| bases.iter().flatten().find_map(|b| b.get(key)))
}

fn parse_style_run(
    item: &Value,
    len: usize,
    fonts: &[String],
    bases: &[Option<&Value>],
) -> RunStyle {
    let data = item;
    let get = |key: &str| lookup(data, bases, key);
    let sheet = Some(data.clone())
        .filter(|d| matches!(d, Value::Dict(_)))
        .or_else(|| bases.iter().flatten().next().map(|b| (*b).clone()));

    let font_index = get("Font").and_then(Value::as_i64);
    let font = font_index
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| fonts.get(i))
        .cloned();
    let caps = get("FontCaps").and_then(Value::as_i64).unwrap_or(0);
    let mut features = Vec::new();
    for (key, tag) in OPENTYPE_KEYS {
        match get(key).and_then(Value::as_bool) {
            Some(true) => features.push(RunFeature {
                tag: tag.to_owned(),
                value: 1,
            }),
            // `calt` defaults *on* in shapers, so Photoshop writes an explicit
            // off for a run that clears it; keep that as a zero amount.
            Some(false) if tag == "calt" => features.push(RunFeature {
                tag: tag.to_owned(),
                value: 0,
            }),
            _ => {}
        }
    }

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
        // `Kerning: 0` is Photoshop's "no pair kerning" default (it writes it
        // on every run, manual mode included), so zero is absent, not a manual
        // amount; the manual amount lives in `Txt2` `/6/10`.
        kerning: get("Kerning")
            .and_then(Value::as_i64)
            .and_then(|v| i16::try_from(v).ok())
            .filter(|v| *v != 0),
        // Auto leading owns the line height: Photoshop writes a *computed*
        // absolute `Leading` beside `AutoLeading: true` (its own files carry
        // the line height there), so an auto run must not carry that number
        // into the model as an explicit leading.
        leading: if get("AutoLeading").and_then(Value::as_bool) == Some(true) {
            None
        } else {
            get("Leading").and_then(Value::as_f32)
        },
        auto_leading: get("AutoLeading").and_then(Value::as_bool),
        underline_type: None,
        strike_type: None,
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
        baseline: get("FontBaseline").and_then(Value::as_i64),
        // `BaselineDirection` is a per-run property: Photoshop composes a run
        // that omits it upright even though every PSD's document normal style
        // sheet stores `2` (`Untitled-15`'s vertical layers: their runs omit
        // it, Photoshop's own forced recompose writes `1`, and its stored
        // raster is the upright composition — against the source layer our
        // upright render scores IoU 0.815, the turned one 0.40). Inheriting
        // the sheet value therefore turned every vertical run that omitted it.
        rotate_roman: data
            .get("BaselineDirection")
            .and_then(Value::as_i64)
            .map(|v| v == 2),
        features,
        sheet,
    }
}

/// Expands per-character pair kerning into runs, as Photoshop's reader does:
/// a run's `Kerning` is the space *after* its characters, so the value lands on
/// the previous character; manual kerning also clears the mode of the pair
/// before it.
fn apply_pair_kerning(
    text: &str,
    runs: Vec<RunStyle>,
    incoming: &[(usize, bool, f32)],
) -> Vec<RunStyle> {
    if text.is_empty() || runs.is_empty() || incoming.is_empty() {
        return runs;
    }
    // Character boundaries as (UTF-8 byte offset, UTF-16 unit offset): a style run's
    // position is addressed in bytes, but `RunStyle.len` (like the engine's
    // `RunLengthArray`) counts UTF-16 units. Mixing the two put a byte count into the
    // run lengths, which `read_layer` then had to refit and which split runs at the
    // wrong character for any text with non-ASCII (e.g. `Größe`).
    let mut starts: Vec<(usize, usize)> = Vec::with_capacity(text.len() + 1);
    let mut units = 0usize;
    for (b, ch) in text.char_indices() {
        starts.push((b, units));
        units += ch.len_utf16();
    }
    starts.push((text.len(), units));
    let n = starts.len().saturating_sub(1);
    let mut kern = vec![0.0f32; n];
    let mut manual = vec![false; n];
    let (mut si, mut seg_end) = (0usize, incoming.first().map_or(0, |s| s.0));
    for (j, &(_, u)) in starts.iter().enumerate() {
        while u >= seg_end && si < incoming.len() {
            si += 1;
            seg_end = seg_end.saturating_add(incoming.get(si).map_or(0, |s| s.0));
        }
        let Some(&(_, auto, k)) = incoming.get(si) else {
            break;
        };
        if k != 0.0
            && let Some(p) = j.checked_sub(1)
            && let (Some(kp), Some(mp)) = (kern.get_mut(p), manual.get_mut(p))
        {
            *kp = k;
            *mp = true;
        }
        if !auto && let Some(mp) = j.checked_sub(2).and_then(|p| manual.get_mut(p)) {
            *mp = true;
        }
    }
    let (mut ri, mut run_end) = (0usize, runs.first().map_or(0, |r| r.len));
    let mut out: Vec<RunStyle> = Vec::new();
    for c in 0..n {
        let (_, at) = starts[c];
        let len = starts[c + 1].1 - at;
        while at >= run_end && ri + 1 < runs.len() {
            ri += 1;
            run_end = run_end.saturating_add(runs.get(ri).map_or(0, |r| r.len));
        }
        let Some(r) = runs.get(ri) else {
            break;
        };
        let k = kern.get(c).copied().unwrap_or(0.0);
        let kerning = if manual.get(c).copied().unwrap_or(false) {
            Some(false)
        } else {
            Some(true)
        };
        let manual_value = (k.round() as i16).to_owned();
        match out.last_mut() {
            Some(last)
                if last.kerning == Some(manual_value)
                    && last.auto_kerning == kerning
                    && same_but_kerning(last, r) =>
            {
                last.len += len;
            }
            _ => {
                let mut style = r.clone();
                style.len = len;
                style.kerning = Some(manual_value);
                style.auto_kerning = kerning;
                out.push(style);
            }
        }
    }
    out
}

fn same_but_kerning(a: &RunStyle, b: &RunStyle) -> bool {
    a.font == b.font
        && a.font_index == b.font_index
        && a.size == b.size
        && a.color == b.color
        && a.tracking == b.tracking
        && a.baseline_shift == b.baseline_shift
        && a.horizontal_scale == b.horizontal_scale
        && a.vertical_scale == b.vertical_scale
        && a.leading == b.leading
        && a.auto_leading == b.auto_leading
        && a.faux_bold == b.faux_bold
        && a.faux_italic == b.faux_italic
        && a.underline == b.underline
        && a.strikethrough == b.strikethrough
        && a.all_caps == b.all_caps
        && a.small_caps == b.small_caps
        && a.old_style == b.old_style
        && a.ligatures == b.ligatures
        && a.alternate_ligatures == b.alternate_ligatures
        && a.language == b.language
        && a.no_break == b.no_break
        && a.baseline == b.baseline
        && a.rotate_roman == b.rotate_roman
        && a.features == b.features
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: Vec<(&str, Value)>) -> Value {
        Value::dict(entries)
    }

    fn engine(style_runs: Vec<Value>, paragraphs: Vec<Value>, res: Vec<(&str, Value)>) -> Value {
        dict(vec![
            (
                "EngineDict",
                dict(vec![
                    (
                        "StyleRun",
                        dict(vec![
                            ("RunLengthArray", Value::Array(vec![Value::Int(2)])),
                            ("RunArray", Value::Array(style_runs)),
                        ]),
                    ),
                    (
                        "ParagraphRun",
                        dict(vec![
                            ("RunLengthArray", Value::Array(vec![Value::Int(2)])),
                            ("RunArray", Value::Array(paragraphs)),
                        ]),
                    ),
                ]),
            ),
            ("ResourceDict", dict(res)),
        ])
    }

    /// #3795: a character run inherits what its `StyleSheetData` leaves out from
    /// the `StyleSheetSet` entry its `Parent` names, then from the paragraph's
    /// default character style.
    #[test]
    fn character_runs_inherit_their_style_sheet() {
        let body = dict(vec![
            ("Name", Value::Str("Body".into())),
            (
                "StyleSheetData",
                dict(vec![
                    ("FontSize", Value::Float(10.0)),
                    ("FauxBold", Value::Bool(true)),
                ]),
            ),
        ]);
        let run = dict(vec![(
            "StyleSheet",
            dict(vec![
                ("Parent", Value::Int(1)),
                (
                    "StyleSheetData",
                    dict(vec![("FontSize", Value::Float(20.0))]),
                ),
            ]),
        )]);
        let paragraph = dict(vec![(
            "ParagraphSheet",
            dict(vec![("Properties", dict(vec![]))]),
        )]);
        let root = engine(
            vec![run],
            vec![paragraph],
            vec![
                ("StyleSheetSet", Value::Array(vec![dict(vec![]), body])),
                ("ParagraphSheetSet", Value::Array(vec![dict(vec![])])),
                ("TheNormalStyleSheet", Value::Int(0)),
                ("TheNormalParagraphSheet", Value::Int(0)),
            ],
        );
        let runs = from_engine_data(&root);
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].size, Some(20.0), "the run's own value wins");
        assert!(runs[0].faux_bold, "inherited from the Body sheet");
    }

    /// A paragraph run inherits its `Properties` from the `ParagraphSheetSet`
    /// entry it names, then from `TheNormalParagraphSheet`.
    #[test]
    fn paragraph_runs_inherit_their_sheet() {
        let heading = dict(vec![
            ("Name", Value::Str("Heading".into())),
            (
                "Properties",
                dict(vec![
                    ("Justification", Value::Int(2)),
                    ("FirstLineIndent", Value::Float(12.0)),
                ]),
            ),
        ]);
        let paragraph = dict(vec![(
            "ParagraphSheet",
            dict(vec![
                ("Parent", Value::Int(1)),
                (
                    "Properties",
                    dict(vec![("FirstLineIndent", Value::Float(4.0))]),
                ),
            ]),
        )]);
        let root = engine(
            vec![dict(vec![])],
            vec![paragraph],
            vec![
                ("StyleSheetSet", Value::Array(vec![dict(vec![])])),
                (
                    "ParagraphSheetSet",
                    Value::Array(vec![dict(vec![]), heading]),
                ),
                ("TheNormalStyleSheet", Value::Int(0)),
                ("TheNormalParagraphSheet", Value::Int(0)),
            ],
        );
        let paras = super::super::paragraph::paragraphs_from_engine_data(&root);
        assert_eq!(paras.len(), 1, "{paras:?}");
        assert_eq!(paras[0].first_line_indent, 4.0);
        assert_eq!(
            paras[0].justification,
            crate::area::Justification::Center,
            "justification inherited from the Heading sheet"
        );
    }

    /// A run's `Kerning` is the space after its characters: the pair split
    /// moves it onto the previous character and marks the mode manual/off.
    #[test]
    fn pair_kerning_splits_runs() {
        let run = dict(vec![(
            "StyleSheet",
            dict(vec![(
                "StyleSheetData",
                dict(vec![
                    ("FontSize", Value::Float(12.0)),
                    ("Kerning", Value::Int(100)),
                ]),
            )]),
        )]);
        let root = engine(
            vec![run],
            vec![dict(vec![])],
            vec![("StyleSheetSet", Value::Array(vec![dict(vec![])]))],
        );
        let runs = from_engine_data_with_text(&root, "AB");
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 2);
        assert_eq!(runs[0].kerning, Some(100), "{runs:?}");
        assert_eq!(runs[0].auto_kerning, Some(false));
        assert_eq!(runs[1].kerning, Some(0));
        assert_eq!(runs[1].auto_kerning, Some(true));
    }
}
