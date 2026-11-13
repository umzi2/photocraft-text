mod font;
mod paint;
mod properties;

use crate::error::Error;

use super::path::{Step, Value, ensure_value, replace_value, scalar_at};
use super::runs::run_path;

pub use font::{ensure_font, font_count, set_run_font_index};
pub use paint::{run_fill_color, run_stroke_color, set_run_fill_color, set_run_stroke_color};
pub use properties::{
    Capitalization, KerningType, Language, OLD_STYLE_ON, RunProperty, Strikethrough, Underline,
};

/// Sets the font size of style run `run`.
pub fn set_run_font_size(block: &[u8], run: usize, size: f32) -> Result<Vec<u8>, Error> {
    set_run_property(
        block,
        run,
        RunProperty::FontSize,
        &Value::Real(f64::from(size)),
    )
}

/// Writes one character property of style run `run`.
pub fn set_run_property(
    block: &[u8],
    run: usize,
    property: RunProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    if property.is_paint() {
        return Err(Error::Txt2(format!(
            "/6/{} is a paint dictionary: use set_run_fill_color / set_run_stroke_color",
            property.key()
        )));
    }
    let path = run_path(
        run,
        &[
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key(property.key()),
        ],
    );
    replace_value(block, &path, value)
}

/// Reads one scalar character property of style run `run`.
///
/// `None` when the slot, the run or the block is absent; paint slots are read
/// with [`run_fill_color`] / [`run_stroke_color`].
pub fn run_property(block: &[u8], run: usize, property: RunProperty) -> Option<Value> {
    if property.is_paint() {
        return None;
    }
    let path = run_path(
        run,
        &[
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key(property.key()),
        ],
    );
    scalar_at(block, &path)
}

fn set_run_int(
    block: &[u8],
    run: usize,
    property: RunProperty,
    value: i64,
) -> Result<Vec<u8>, Error> {
    set_run_ensured(block, run, property, &Value::Int(value))
}

fn set_run_bool(
    block: &[u8],
    run: usize,
    property: RunProperty,
    on: bool,
) -> Result<Vec<u8>, Error> {
    set_run_ensured(block, run, property, &Value::Bool(on))
}

/// Replaces a slot, adding it when the template's sheet does not carry it.
fn set_run_ensured(
    block: &[u8],
    run: usize,
    property: RunProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let path = run_path(
        run,
        &[
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key(property.key()),
        ],
    );
    ensure_value(block, &path, value)
}

fn run_int(block: &[u8], run: usize, property: RunProperty) -> Option<i64> {
    run_property(block, run, property).and_then(|v| match v {
        Value::Int(i) => Some(i),
        _ => None,
    })
}

fn run_bool(block: &[u8], run: usize, property: RunProperty) -> Option<bool> {
    run_property(block, run, property).and_then(|v| match v {
        Value::Bool(b) => Some(b),
        _ => None,
    })
}

/// Faux bold of run `run` (`/6/2`).
pub fn set_run_faux_bold(block: &[u8], run: usize, on: bool) -> Result<Vec<u8>, Error> {
    set_run_bool(block, run, RunProperty::FauxBold, on)
}

pub fn run_faux_bold(block: &[u8], run: usize) -> Option<bool> {
    run_bool(block, run, RunProperty::FauxBold)
}

/// Faux italic of run `run` (`/6/3`).
pub fn set_run_faux_italic(block: &[u8], run: usize, on: bool) -> Result<Vec<u8>, Error> {
    set_run_bool(block, run, RunProperty::FauxItalic, on)
}

pub fn run_faux_italic(block: &[u8], run: usize) -> Option<bool> {
    run_bool(block, run, RunProperty::FauxItalic)
}

/// Kerning mode of run `run` (`/6/11`).
pub fn set_run_kerning_type(block: &[u8], run: usize, kind: KerningType) -> Result<Vec<u8>, Error> {
    set_run_int(block, run, RunProperty::KerningType, kind.code())
}

/// Same, from a stored code. Rejects codes Photoshop does not define, so a
/// bogus value can never be written.
pub fn set_run_kerning_type_code(block: &[u8], run: usize, code: i64) -> Result<Vec<u8>, Error> {
    let kind = KerningType::from_code(code).ok_or_else(|| {
        Error::Txt2(format!(
            "not a kerning type: {code} (0 manual, 1 metrics, 2 optical)"
        ))
    })?;
    set_run_kerning_type(block, run, kind)
}

pub fn run_kerning_type(block: &[u8], run: usize) -> Option<KerningType> {
    run_int(block, run, RunProperty::KerningType).and_then(KerningType::from_code)
}

/// Case mapping of run `run` (`/6/12`).
pub fn set_run_capitalization(
    block: &[u8],
    run: usize,
    caps: Capitalization,
) -> Result<Vec<u8>, Error> {
    set_run_int(block, run, RunProperty::Capitalization, caps.code())
}

pub fn set_run_capitalization_code(block: &[u8], run: usize, code: i64) -> Result<Vec<u8>, Error> {
    let caps = Capitalization::from_code(code).ok_or_else(|| {
        Error::Txt2(format!(
            "not a capitalization: {code} (0 normal, 1 small, 2 all)"
        ))
    })?;
    set_run_capitalization(block, run, caps)
}

pub fn run_capitalization(block: &[u8], run: usize) -> Option<Capitalization> {
    run_int(block, run, RunProperty::Capitalization).and_then(Capitalization::from_code)
}

/// Underline of run `run` (`/6/16`).
pub fn set_run_underline(block: &[u8], run: usize, kind: Underline) -> Result<Vec<u8>, Error> {
    set_run_int(block, run, RunProperty::Underline, kind.code())
}

pub fn set_run_underline_code(block: &[u8], run: usize, code: i64) -> Result<Vec<u8>, Error> {
    let kind = Underline::from_code(code).ok_or_else(|| {
        Error::Txt2(format!(
            "not an underline kind: {code} (0 off, 1 right, 2 left)"
        ))
    })?;
    set_run_underline(block, run, kind)
}

pub fn run_underline(block: &[u8], run: usize) -> Option<Underline> {
    run_int(block, run, RunProperty::Underline).and_then(Underline::from_code)
}

/// Strikethrough of run `run` (`/6/15`).
pub fn set_run_strikethrough(
    block: &[u8],
    run: usize,
    kind: Strikethrough,
) -> Result<Vec<u8>, Error> {
    set_run_int(block, run, RunProperty::Strikethrough, kind.code())
}

pub fn set_run_strikethrough_code(block: &[u8], run: usize, code: i64) -> Result<Vec<u8>, Error> {
    let kind = Strikethrough::from_code(code).ok_or_else(|| {
        Error::Txt2(format!(
            "not a strikethrough kind: {code} (0 off, 2 strike box)"
        ))
    })?;
    set_run_strikethrough(block, run, kind)
}

pub fn run_strikethrough(block: &[u8], run: usize) -> Option<Strikethrough> {
    run_int(block, run, RunProperty::Strikethrough).and_then(Strikethrough::from_code)
}

/// Standard ligatures of run `run` (`/6/18`).
pub fn set_run_ligatures(block: &[u8], run: usize, on: bool) -> Result<Vec<u8>, Error> {
    set_run_bool(block, run, RunProperty::Ligatures, on)
}

pub fn run_ligatures(block: &[u8], run: usize) -> Option<bool> {
    run_bool(block, run, RunProperty::Ligatures)
}

/// Alternate (discretionary) ligatures of run `run` (`/6/19`).
pub fn set_run_alternate_ligatures(block: &[u8], run: usize, on: bool) -> Result<Vec<u8>, Error> {
    set_run_bool(block, run, RunProperty::AlternateLigatures, on)
}

pub fn run_alternate_ligatures(block: &[u8], run: usize) -> Option<bool> {
    run_bool(block, run, RunProperty::AlternateLigatures)
}

/// Old-style figures of run `run` (`/6/30`); the measured "on" value is
/// [`OLD_STYLE_ON`]. Rejects a negative level.
pub fn set_run_old_style(block: &[u8], run: usize, level: i64) -> Result<Vec<u8>, Error> {
    if level < 0 {
        return Err(Error::Txt2(format!("not an old-style level: {level}")));
    }
    set_run_int(block, run, RunProperty::OldStyle, level)
}

pub fn run_old_style(block: &[u8], run: usize) -> Option<i64> {
    run_int(block, run, RunProperty::OldStyle)
}

/// Language of run `run` (`/6/38`).
pub fn set_run_language(block: &[u8], run: usize, language: Language) -> Result<Vec<u8>, Error> {
    set_run_language_code(block, run, language.code())
}

/// Language by raw Photoshop code. Negative codes are refused; forward codes
/// Photoshop may add later are accepted as-is.
pub fn set_run_language_code(block: &[u8], run: usize, code: i64) -> Result<Vec<u8>, Error> {
    if code < 0 {
        return Err(Error::Txt2(format!("not a language code: {code}")));
    }
    set_run_int(block, run, RunProperty::Language, code)
}

/// The raw language code of run `run`.
pub fn run_language_code(block: &[u8], run: usize) -> Option<i64> {
    run_int(block, run, RunProperty::Language)
}

/// Language of run `run` when it is one of the measured codes.
pub fn run_language(block: &[u8], run: usize) -> Option<Language> {
    run_language_code(block, run).and_then(Language::from_code)
}

/// "No break" of run `run` (`/6/52`): keep it on one line.
pub fn set_run_no_break(block: &[u8], run: usize, on: bool) -> Result<Vec<u8>, Error> {
    set_run_bool(block, run, RunProperty::NoBreak, on)
}

pub fn run_no_break(block: &[u8], run: usize) -> Option<bool> {
    run_bool(block, run, RunProperty::NoBreak)
}
