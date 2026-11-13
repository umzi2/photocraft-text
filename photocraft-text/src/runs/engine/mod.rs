use crate::engine_data::Value;

mod paragraph;
mod parse;
mod write;

pub use paragraph::{
    apply_paragraph_formats, apply_paragraph_runs, paragraph_formats_from_engine_data,
    paragraphs_from_engine_data,
};
pub use parse::from_engine_data;
pub use write::{apply_style_runs, to_engine_data};

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

/// The layer's antialiasing mode (`EngineDict/AntiAlias`), default sharp.
pub fn antialias(root: &Value) -> crate::raster::AntiAlias {
    root.get("EngineDict")
        .and_then(|e| e.get("AntiAlias"))
        .and_then(Value::as_i64)
        .and_then(crate::raster::AntiAlias::from_code)
        .unwrap_or_default()
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

const CAPS_SMALL: i64 = 1;
const CAPS_ALL: i64 = 2;
const COLOR_TYPE_RGB: i64 = 1;
