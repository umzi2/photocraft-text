use crate::engine_data::Value;

use super::super::model::RunStyle;
use super::{CAPS_ALL, CAPS_SMALL, color_rgb, font_names, run_lengths, txt2_language};

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
