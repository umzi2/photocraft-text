use crate::error::Error;

use super::path::{Step, Value, ensure_container, replace_value, resolve};
use super::syntax::{skip_value, tokens};

/// Path to the layer's antialias code (`/1/1/[0]/0/10/[0]`), the same code
/// Photoshop stores in `EngineData`'s `EngineDict/AntiAlias`.
pub fn antialias_path_at(object: usize) -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("0"),
        Step::Key("10"),
        Step::Key("0"),
    ]
}

/// Path to text object 0's antialias code (`/1/1[0]/0/10/[0]`).
pub fn antialias_path() -> Vec<Step<'static>> {
    antialias_path_at(0)
}

/// Path to the layer's antialias dictionary (`/1/1[0]/0/10`).
///
/// Photoshop omits the whole dictionary for `AntiAlias::Smooth` — smooth is
/// the implicit mode when the key is absent (`docs/txt2-map.md` §4,
/// `out/probe2/aa_smooth.psd`).
pub fn antialias_dict_path_at(object: usize) -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("0"),
        Step::Key("10"),
    ]
}

/// Path to text object 0's antialias dictionary (`/1/1[0]/0/10`).
pub fn antialias_dict_path() -> Vec<Step<'static>> {
    antialias_dict_path_at(0)
}

/// Writes the layer's antialias code (`raster::AntiAlias::code`).
///
/// `3` (smooth) is encoded by removing the `/10` dictionary, which is how
/// Photoshop stores it; every other code writes `/10/0`. A template that has
/// no `/10` dict (smooth) gets one back in Photoshop's own shape
/// `/10 << /0 <code> /2 true >>` when a non-smooth code is authored.
pub fn set_antialias(block: &[u8], code: i64) -> Result<Vec<u8>, Error> {
    set_antialias_at(block, 0, code)
}

/// [`set_antialias`] for text object `object`.
pub fn set_antialias_at(block: &[u8], object: usize, code: i64) -> Result<Vec<u8>, Error> {
    let smooth = crate::raster::AntiAlias::Smooth.code();
    if code == smooth {
        return remove_entry(block, &antialias_dict_path_at(object));
    }
    let toks = tokens(block)?;
    if resolve(&toks, &antialias_dict_path_at(object)).is_none() {
        let value = format!("<< /0 {code} /2 true >>");
        return ensure_container(block, &antialias_dict_path_at(object), value.as_bytes());
    }
    replace_value(block, &antialias_path_at(object), &Value::Int(code))
}

/// The antialias code of text object `object`: `/1/1[object]/0/10/0`.
///
/// Photoshop omits the whole `/10` dictionary for `AntiAlias::Smooth`, so an
/// object that exists but has no `/10` reads as smooth (code 3). `None` when
/// the object or its model dictionary is absent.
pub fn object_antialias(block: &[u8], object: usize) -> Option<i64> {
    let toks = tokens(block).ok()?;
    resolve(
        &toks,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(object),
            Step::Key("0"),
        ],
    )?;
    match resolve(&toks, &antialias_dict_path_at(object)) {
        None => Some(crate::raster::AntiAlias::Smooth.code()),
        Some(dict) => {
            let entries = super::syntax::dict_entries(&toks, dict)?;
            let code = entries.iter().find(|(k, _)| k == "0").map(|(_, v)| *v)?;
            toks[code].kind.as_int()
        }
    }
}

/// Removes the dictionary entry whose value ends at `path` (a key of its
/// parent dict), together with the whitespace that follows it.
///
/// The whitespace before the key becomes the separator for whatever follows,
/// so the result keeps Photoshop's single-space style and stays tokenisable.
fn remove_entry(block: &[u8], path: &[Step<'_>]) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let Some(value_index) = resolve(&toks, path) else {
        return Ok(block.to_vec());
    };
    let key_index = value_index
        .checked_sub(1)
        .ok_or_else(|| Error::Txt2("entry has no key".into()))?;
    let next =
        skip_value(&toks, value_index).ok_or_else(|| Error::Txt2("entry has no end".into()))?;
    let start = toks[key_index].start;
    let end = toks.get(next).map_or(block.len(), |t| t.start);
    let mut out = block.to_vec();
    out.drain(start..end);
    Ok(out)
}
