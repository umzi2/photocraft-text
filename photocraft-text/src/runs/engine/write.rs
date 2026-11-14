use crate::engine_data::Value;

use super::super::model::RunStyle;
use super::{CAPS_ALL, CAPS_SMALL, COLOR_TYPE_RGB, OPENTYPE_KEYS, color_rgb, engine_language};

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
                if auto {
                    // Photoshop's own form: with auto leading on, the absolute
                    // `Leading` field is not the model's leading (Photoshop
                    // recomputes it); write the neutral 0 so a re-read cannot
                    // carry a stale `Leading/size` into the model ratio.
                    put("Leading", Value::Float(0.0));
                }
            }
            // Auto leading owns the field: a `Some` absolute value on an
            // auto-leading run must not overwrite the neutral 0 (or a re-read
            // would take it as the model's leading).
            if let Some(v) = run.leading
                && run.auto_leading_flag() != Some(true)
            {
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
            if let Some(code) = run.baseline {
                put("FontBaseline", Value::Int(code));
            }
            if let Some(roman) = run.rotate_roman {
                // `BaselineDirection` 2 rotates roman text along a vertical
                // column; 1 stands it upright one character per em.
                put("BaselineDirection", Value::Int(if roman { 2 } else { 1 }));
            }
            put_features(&mut put, &run.features);
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
        if auto {
            // Photoshop's own form (see `sheet_for`): auto leading writes the
            // neutral absolute `Leading` 0, never the computed line height.
            put_num(&mut d, "Leading", 0.0);
        }
    }
    if let Some(l) = run.leading
        && run.auto_leading_flag() != Some(true)
    {
        put_num(&mut d, "Leading", l);
    }
    // `OldStyle` is EngineData's key for the `onum` feature; the sheet's own
    // `Features` array is not modelled, so a run whose model says old-style
    // must write the key itself or the figures read back as lining.
    if run.old_style && !run.features.iter().any(|f| f.tag == "onum" && f.value > 0) {
        put(&mut d, "OldStyle", Value::Bool(true));
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
    if let Some(code) = run.baseline {
        put(&mut d, "FontBaseline", Value::Int(code));
    }
    if let Some(roman) = run.rotate_roman {
        put(
            &mut d,
            "BaselineDirection",
            Value::Int(if roman { 2 } else { 1 }),
        );
    }
    for feature in &run.features {
        if let Some((key, _)) = OPENTYPE_KEYS.iter().find(|(_, tag)| *tag == feature.tag) {
            put(&mut d, key, Value::Bool(feature.value > 0));
        }
    }
    d
}

/// Writes the OpenType features that have an `EngineData` key; the others
/// (`ss01`…`ss20`) live only in `Txt2` and reach the pixels through
/// [`super::super::model::RunStyle::features`] directly.
fn put_features<'a>(
    put: &mut dyn FnMut(&'a str, Value),
    features: &[super::super::model::RunFeature],
) {
    for feature in features {
        if let Some((key, _)) = OPENTYPE_KEYS.iter().find(|(_, tag)| *tag == feature.tag) {
            put(key, Value::Bool(feature.value > 0));
        }
    }
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
