use crate::error::Error;

use super::super::path::{Step, Value, replace_value, resolve};
use super::super::runs::run_path;
use super::super::syntax::{Kind, skip_value, tokens, utf16_name};

fn font_set_path() -> Vec<Step<'static>> {
    vec![Step::Key("0"), Step::Key("1"), Step::Key("0")]
}

/// Number of fonts in the block's `FontSet`.
pub fn font_count(block: &[u8]) -> Result<usize, Error> {
    let toks = tokens(block)?;
    let open =
        resolve(&toks, &font_set_path()).ok_or_else(|| Error::Txt2("font set not found".into()))?;
    let mut j = open + 1;
    let mut n = 0;
    while !matches!(
        toks.get(j)
            .ok_or_else(|| Error::Txt2("unterminated font set".into()))?
            .kind,
        Kind::ArrayClose
    ) {
        j = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad font entry".into()))?;
        n += 1;
    }
    Ok(n)
}

/// Appends `name` to the `FontSet` unless it is already there, returning the
/// new block and the font's index. Photoshop stores the PostScript name
/// (`NotoSans-Regular`).
pub fn ensure_font(block: &[u8], name: &str) -> Result<(Vec<u8>, usize), Error> {
    let encoded = utf16_name(name);
    let toks = tokens(block)?;
    let open =
        resolve(&toks, &font_set_path()).ok_or_else(|| Error::Txt2("font set not found".into()))?;
    let mut j = open + 1;
    let mut index = 0;
    while !matches!(toks[j].kind, Kind::ArrayClose) {
        let next = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad font entry".into()))?;
        if block[toks[j].start..toks[next - 1].end]
            .windows(encoded.len())
            .any(|w| w == encoded.as_slice())
        {
            return Ok((block.to_vec(), index));
        }
        j = next;
        index += 1;
    }
    let close = toks[j].start;
    let mut entry = Vec::new();
    entry.extend_from_slice(b" << /0 << /99 /CoolTypeFont /0 << /0 (");
    entry.extend_from_slice(&encoded);
    entry.extend_from_slice(b") /2 1 >> >> >>");
    entry.push(b' ');
    let mut out = block.to_vec();
    out.splice(close..close, entry);
    Ok((out, index))
}

/// Points style run `run` at `FontSet` index `index` (`/0` of its style sheet).
pub fn set_run_font_index(block: &[u8], run: usize, index: usize) -> Result<Vec<u8>, Error> {
    let path = run_path(
        run,
        &[
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key("0"),
        ],
    );
    replace_value(block, &path, &Value::Int(index as i64))
}
