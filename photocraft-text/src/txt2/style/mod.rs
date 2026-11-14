mod font;
mod paint;
mod properties;

use crate::error::Error;

use super::path::{Step, Value, ensure_value, replace_value, scalar_at};
use super::runs::run_path;

pub use font::{ensure_font, font_count, set_run_font_index};
pub use paint::{
    run_fill_color, run_stroke_color, set_run_fill_color, set_run_fill_color_at,
    set_run_stroke_color, set_run_stroke_color_at,
};
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
    set_run_property_at(block, 0, run, property, value)
}

/// [`set_run_property`] for text object `object` (Photoshop keeps one object
/// per type layer; a document's block has one per layer).
///
/// Every typed character setter writes object 0 through this function; call it
/// directly to edit another object's run.
pub fn set_run_property_at(
    block: &[u8],
    object: usize,
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
    let mut path = super::runs::run_path_for_at(object, run);
    path.extend_from_slice(&[
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key(property.key()),
    ]);
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
    set_run_ensured_at(block, 0, run, property, value)
}

/// [`set_run_ensured`] for text object `object`, adding the slot when the
/// template's sheet does not carry it.
pub fn set_run_ensured_at(
    block: &[u8],
    object: usize,
    run: usize,
    property: RunProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let mut path = super::runs::run_path_for_at(object, run);
    path.extend_from_slice(&[
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key(property.key()),
    ]);
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

/// Reads one scalar character property of style run `run` of text object
/// `object` (Photoshop keeps one text object per type layer).
///
/// `None` when the slot, the run or the block is absent. This is the
/// object-index form of [`run_property`]; see [`super::runs::run_path_for_at`].
pub fn run_property_at(
    block: &[u8],
    object: usize,
    run: usize,
    property: RunProperty,
) -> Option<Value> {
    if property.is_paint() {
        return None;
    }
    let mut path = super::runs::run_path_for_at(object, run);
    path.extend_from_slice(&[
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key(property.key()),
    ]);
    scalar_at(block, &path)
}

/// Reads a style run's manual kerning (`/6/10`) of text object `object`, in
/// 1/1000 em.
///
/// Photoshop stores the slot as 16.16 fixed point (`/10 = kern / 65536`; a
/// manual kern of `100` is stored `0.00153`, measured in
/// `out/verify/mk100_res.psd`), which is why [`RunProperty`] has no entry for
/// it: that enum only covers scalars written back verbatim. `None` when the run
/// or the slot is absent.
pub fn run_manual_kerning_at(block: &[u8], object: usize, run: usize) -> Option<i32> {
    let mut path = super::runs::run_path_for_at(object, run);
    path.extend_from_slice(&[
        Step::Key("0"),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key("10"),
    ]);
    match scalar_at(block, &path)? {
        Value::Real(r) => fixed_kerning(r),
        Value::Int(i) => fixed_kerning(i as f64),
        _ => None,
    }
}

/// 16.16 fixed point as Photoshop stores manual kerning, in 1/1000 em.
fn fixed_kerning(value: f64) -> Option<i32> {
    let units = (value * 65_536.0).round();
    if !units.is_finite() {
        return None;
    }
    Some(units.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32)
}

/// Manual kerning of style run `run` of text object 0, in 1/1000 em.
pub fn run_manual_kerning(block: &[u8], run: usize) -> Option<i32> {
    run_manual_kerning_at(block, 0, run)
}

/// Reads the fill colour of style run `run` of text object `object` as
/// `[alpha, red, green, blue]` in the stored `SimplePaint` order.
pub fn run_fill_color_at(block: &[u8], object: usize, run: usize) -> Option<[f32; 4]> {
    paint::run_paint_at(block, object, run, RunProperty::FillPaint)
}

/// Reads the stroke colour of style run `run` of text object `object` as
/// `[alpha, red, green, blue]`.
pub fn run_stroke_color_at(block: &[u8], object: usize, run: usize) -> Option<[f32; 4]> {
    paint::run_paint_at(block, object, run, RunProperty::StrokePaint)
}

/// The PostScript name of `FontSet` entry `index` — the name Photoshop stores
/// in the font table (`NotoSans-Regular`); `None` when the entry is missing or
/// malformed.
pub fn font_name(block: &[u8], index: usize) -> Option<String> {
    let toks = super::syntax::tokens(block).ok()?;
    let set = super::path::resolve(&toks, &[Step::Key("0"), Step::Key("1"), Step::Key("0")])?;
    let mut value = set + 1;
    for _ in 0..index {
        value = super::syntax::skip_value(&toks, value)?;
    }
    if matches!(toks.get(value)?.kind, super::syntax::Kind::ArrayClose) {
        return None;
    }
    let entry = super::syntax::dict_entries(&toks, value)?;
    let font = dict_value(&entry, "0")?;
    let font = super::syntax::dict_entries(&toks, font)?;
    let inner = dict_value(&font, "0")?;
    let inner = super::syntax::dict_entries(&toks, inner)?;
    let name = dict_value(&inner, "0")?;
    match &toks[name].kind {
        super::syntax::Kind::Str(body) => decode_name(body),
        _ => None,
    }
}

/// The value token of key `key` in a dict's entry list.
fn dict_value(entries: &[(String, usize)], key: &str) -> Option<usize> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

/// Decodes a `Txt2` name string (BOM + escaped UTF-16BE, no terminator).
fn decode_name(body: &[u8]) -> Option<String> {
    let body = body.strip_prefix(&[0xfe, 0xff])?;
    let mut raw = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if body[i] == b'\\' && i + 1 < body.len() {
            i += 1;
        }
        raw.push(body[i]);
        i += 1;
    }
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    String::from_utf16(&units).ok()
}

/// The layer's antialias code from `Txt2` text object 0 (`/1 /1 [0] /0 /10 /0`).
///
/// Photoshop omits the whole `/10` dictionary for smooth (code `3`), so an
/// object without `/10` is smooth; `None` when the block has no object 0 or the
/// dictionary carries something that is not a code.
pub fn antialias_code(block: &[u8]) -> Option<i64> {
    let toks = super::syntax::tokens(block).ok()?;
    let object = [Step::Key("1"), Step::Key("1"), Step::Index(0)];
    super::path::resolve(&toks, &object)?;
    let mut dict = object.to_vec();
    dict.extend_from_slice(&[Step::Key("0"), Step::Key("10")]);
    if super::path::resolve(&toks, &dict).is_none() {
        return Some(crate::raster::AntiAlias::Smooth.code());
    }
    let mut code = dict;
    code.push(Step::Key("0"));
    match super::path::scalar_at(block, &code)? {
        Value::Int(i) => Some(i),
        Value::Real(r) if r.is_finite() => Some(r as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::fixed_kerning;

    #[test]
    fn manual_kerning_is_16_16_fixed_point() {
        // `/10 0.00153` is 100/65536: a manual kern of 100 (1/1000 em), the
        // value measured in `out/verify/mk100_res.psd`.
        assert_eq!(fixed_kerning(0.001_525_878_906_25), Some(100));
        assert_eq!(fixed_kerning(0.0), Some(0));
        assert_eq!(fixed_kerning(-0.003_051_757_812_5), Some(-200));
    }
}
