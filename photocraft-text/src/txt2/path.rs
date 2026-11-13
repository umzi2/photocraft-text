use crate::error::Error;

use super::syntax::{Kind, Token, dict_entries, skip_value, tokens};

// --------------------------------------------------------------------------- //
// Path editing
// --------------------------------------------------------------------------- //

/// One step of a path through the token tree: a dict key or an array index.
/// The first step addresses the block's top-level names (`/1`, `/98`, …).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

/// A scalar that can be written back into a block.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Real(f64),
    Bool(bool),
    Name(String),
}

impl Value {
    pub(super) fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

fn encode_value(v: &Value) -> Vec<u8> {
    match v {
        Value::Int(i) => i.to_string().into_bytes(),
        Value::Real(r) => format_real(*r).into_bytes(),
        Value::Bool(b) => {
            if *b {
                b"true".to_vec()
            } else {
                b"false".to_vec()
            }
        }
        Value::Name(n) => format!("/{n}").into_bytes(),
    }
}

/// CoolType real format: `%.5f` with trailing zeros trimmed, at least one decimal.
pub(super) fn format_real(r: f64) -> String {
    let t = format!("{r:.5}");
    let t = t.trim_end_matches('0');
    if t.ends_with('.') {
        format!("{t}0")
    } else {
        t.to_string()
    }
}

fn top_level(tokens: &[Token]) -> Option<Vec<(String, usize)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Kind::Name(k) = &tokens[i].kind {
            let value = i + 1;
            out.push((k.clone(), value));
            i = skip_value(tokens, value)?;
        } else {
            i += 1;
        }
    }
    Some(out)
}

/// Bytes of the value at `path`, or `None` when the path does not exist.
pub fn read_path(block: &[u8], path: &[Step<'_>]) -> Result<Option<Vec<u8>>, Error> {
    let toks = tokens(block)?;
    Ok(value_span(&toks, path).map(|(start, end)| block[start..end].to_vec()))
}

/// Token index of the value at `path`, or `None` when the path does not exist.
pub(super) fn resolve(tokens: &[Token], path: &[Step<'_>]) -> Option<usize> {
    let mut cur: Option<usize> = None;
    for step in path {
        cur = Some(match (step, cur) {
            (Step::Key(k), None) => {
                top_level(tokens)?
                    .into_iter()
                    .find(|(n, _)| n.as_str() == *k)?
                    .1
            }
            (Step::Key(k), Some(open)) => {
                dict_entries(tokens, open)?
                    .into_iter()
                    .find(|(n, _)| n.as_str() == *k)?
                    .1
            }
            (Step::Index(n), Some(open)) => {
                if !matches!(tokens.get(open)?.kind, Kind::ArrayOpen) {
                    return None;
                }
                let mut j = open + 1;
                for _ in 0..*n {
                    j = skip_value(tokens, j)?;
                }
                if matches!(tokens.get(j)?.kind, Kind::ArrayClose) {
                    return None;
                }
                j
            }
            (Step::Index(_), None) => return None,
        });
    }
    cur
}

/// Byte range `(start, end)` of the complete value at `path`.
pub(super) fn value_span(tokens: &[Token], path: &[Step<'_>]) -> Option<(usize, usize)> {
    let v = resolve(tokens, path)?;
    let after = skip_value(tokens, v)?;
    Some((tokens[v].start, tokens[after - 1].end))
}

/// Byte range of the value at `path` in `block`.
pub fn value_range(block: &[u8], path: &[Step<'_>]) -> Result<(usize, usize), Error> {
    let toks = tokens(block)?;
    value_span(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))
}

/// Replaces the scalar at `path` with `value`. Containers are refused: the
/// rest of the block depends on their contents, so replacing one wholesale is
/// never what a caller means.
pub fn replace_value(block: &[u8], path: &[Step<'_>], value: &Value) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let v = resolve(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))?;
    if matches!(toks[v].kind, Kind::DictOpen | Kind::ArrayOpen) {
        return Err(Error::Txt2(format!(
            "path is a container, not a scalar: {path:?}"
        )));
    }
    let (start, end) = value_span(&toks, path).ok_or_else(|| Error::Txt2("no span".into()))?;
    let mut out = block.to_vec();
    out.splice(start..end, encode_value(value));
    Ok(out)
}

/// Replaces the scalar at `path` with `value`, or inserts it when the slot is
/// absent.
///
/// Photoshop omits the hyphenation detail keys (`/5/10` … `/5/15`) while
/// hyphenation is off, so authoring them has to add the entries. The dict stays
/// in ascending key order and the inserted text uses the same single-space
/// separators Photoshop writes.
pub fn ensure_value(block: &[u8], path: &[Step<'_>], value: &Value) -> Result<Vec<u8>, Error> {
    let encoded = encode_value(value);
    ensure_encoded(block, path, &encoded, false)
}

/// Same as [`ensure_value`] for a container (a paint dictionary, an array).
pub fn ensure_container(block: &[u8], path: &[Step<'_>], value: &[u8]) -> Result<Vec<u8>, Error> {
    ensure_encoded(block, path, value, true)
}

fn ensure_encoded(
    block: &[u8],
    path: &[Step<'_>],
    encoded: &[u8],
    container: bool,
) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    if let Some(index) = resolve(&toks, path) {
        if container {
            return replace_container(block, path, encoded);
        }
        if matches!(toks[index].kind, Kind::DictOpen | Kind::ArrayOpen) {
            return Err(Error::Txt2(format!(
                "path is a container, not a scalar: {path:?}"
            )));
        }
        let (start, end) = value_span(&toks, path).ok_or_else(|| Error::Txt2("no span".into()))?;
        let mut out = block.to_vec();
        out.splice(start..end, encoded.iter().copied());
        return Ok(out);
    }
    let (last, parent) = path
        .split_last()
        .ok_or_else(|| Error::Txt2("empty path".into()))?;
    let Step::Key(key) = last else {
        return Err(Error::Txt2(format!(
            "cannot insert at array index: {path:?}"
        )));
    };
    let dict = resolve(&toks, parent)
        .ok_or_else(|| Error::Txt2(format!("parent dict not found: {parent:?}")))?;
    if !matches!(toks[dict].kind, Kind::DictOpen) {
        return Err(Error::Txt2(format!("parent is not a dict: {parent:?}")));
    }
    let entries = dict_entries(&toks, dict).ok_or_else(|| Error::Txt2("malformed dict".into()))?;
    let wanted = key.parse::<i64>().ok();
    // Insert before the first entry whose (numeric) key is larger, so the dict
    // keeps Photoshop's ascending order; otherwise before the closing `>>`.
    let mut at = None;
    for (name, value_index) in &entries {
        if let (Some(wanted), Ok(found)) = (wanted, name.parse::<i64>())
            && found > wanted
        {
            at = Some(toks[value_index - 1].start);
            break;
        }
    }
    let at = match at {
        Some(pos) => pos,
        None => {
            let mut depth = 0i32;
            let mut close = None;
            for token in toks.iter().skip(dict) {
                match token.kind {
                    Kind::DictOpen | Kind::ArrayOpen => depth += 1,
                    Kind::DictClose | Kind::ArrayClose => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(token.start);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            close.ok_or_else(|| Error::Txt2("unterminated dict".into()))?
        }
    };
    let mut insert = Vec::with_capacity(encoded.len() + key.len() + 3);
    insert.push(b'/');
    insert.extend_from_slice(key.as_bytes());
    insert.push(b' ');
    insert.extend_from_slice(encoded);
    insert.push(b' ');
    let mut out = block.to_vec();
    out.splice(at..at, insert);
    Ok(out)
}

pub(super) fn scalar_at(block: &[u8], path: &[Step<'_>]) -> Option<Value> {
    let toks = tokens(block).ok()?;
    let idx = resolve(&toks, path)?;
    match &toks[idx].kind {
        Kind::Int(i) => Some(Value::Int(*i)),
        Kind::Real(r) => Some(Value::Real(*r)),
        Kind::Bool(b) => Some(Value::Bool(*b)),
        Kind::Name(n) => Some(Value::Name(n.clone())),
        _ => None,
    }
}

/// Replaces the value at `path` with arbitrary bytes (`[ ]` for an empty array).
pub fn replace_container(block: &[u8], path: &[Step<'_>], value: &[u8]) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let (start, end) =
        value_span(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))?;
    let mut out = block.to_vec();
    out.splice(start..end, value.iter().copied());
    Ok(out)
}
