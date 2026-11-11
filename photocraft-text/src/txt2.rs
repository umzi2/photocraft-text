//! The legacy `Txt2` text block.
//!
//! Photoshop takes a text layer's contents from this block, not from
//! `TySh/EngineData`: patching `TySh` alone leaves the layer showing its old
//! text, while patching `Txt2` alone changes it. The block is CoolType's
//! PostScript-token format — dictionaries `<< /k v >>`, arrays `[ … ]`, UTF-16BE
//! strings with a `\xfe\xff` BOM terminated by U+000D — and it stores the text
//! together with run lengths, per-glyph advances and the text bounds.
//!
//! Only a same-length replacement is supported for now: the advance table and
//! the bounds are laid out per character, and Photoshop rejects the file
//! ("not compatible with this version of Photoshop") when they disagree with the
//! string. Regenerating them needs our own layout to produce those numbers.

use crate::error::Error;

const BOM: [u8; 2] = [0xfe, 0xff];
const TERMINATOR: [u8; 2] = [0x00, 0x0d];

/// Decoded UTF-16BE string as it appears inside `Txt2`.
pub fn utf16(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2 + 4);
    out.extend_from_slice(&BOM);
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out.extend_from_slice(&TERMINATOR);
    out
}

/// BOM-prefixed UTF-16BE with no terminator, as used for names (fonts, sheets).
fn utf16_name(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2 + 2);
    out.extend_from_slice(&BOM);
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}

/// Every UTF-16BE BOM-prefixed string in `block`, in order. Font names, the
/// layer text and per-run style strings all show up this way.
pub fn strings(block: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= block.len() {
        if block[i..i + 2] == BOM {
            let mut units = Vec::new();
            let mut j = i + 2;
            while j + 2 <= block.len() {
                let unit = u16::from_be_bytes([block[j], block[j + 1]]);
                if unit == 0x000d {
                    break;
                }
                units.push(unit);
                j += 2;
            }
            if !units.is_empty() {
                if let Ok(s) = String::from_utf16(&units) {
                    out.push(s);
                }
            }
        }
        i += 1;
    }
    out
}

/// Anchor that introduces the layer-text run: `/1 [ << /0 << /0 (`.
const TEXT_ANCHOR: &[u8] = b"/1 [ << /0 << /0 (";

/// Byte range of the layer text's UTF-16 string (BOM and terminator included).
/// Internal U+000D units are paragraph breaks; the string's terminator is the
/// U+000D unit that sits directly before the closing `)`.
pub fn text_span(block: &[u8]) -> Option<(usize, usize)> {
    let start = find(block, TEXT_ANCHOR)? + TEXT_ANCHOR.len();
    if block.get(start..start + 2)? != BOM {
        return None;
    }
    let mut j = start + 2;
    while j + 3 <= block.len() {
        if block[j..j + 2] == TERMINATOR && block[j + 2] == b')' {
            return Some((start, j + 2));
        }
        j += 2;
    }
    None
}

/// The layer text as Photoshop sees it.
pub fn read_text(block: &[u8]) -> Option<String> {
    let (start, end) = text_span(block)?;
    let units: Vec<u16> = block[start + 2..end - 2]
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

/// Replaces the layer text with `new_text` of any length.
///
/// The run tables are rebuilt from the text: one style run covering everything
/// (keeping the first run's character sheet) and one paragraph run per `\r`
/// break, with the composed-layout cache emptied so Photoshop lays the text out
/// again instead of trusting stored glyphs. Runs are split afterwards with
/// [`set_run_layout`] and [`set_run_property`].
pub fn replace_text(block: &[u8], new_text: &str) -> Result<Vec<u8>, Error> {
    let old = read_text(block).ok_or_else(|| Error::Txt2("no text string".into()))?;
    let bare = |t: &str| t.replace("\r\n", "\r").trim_end_matches(['\r', '\n']).to_owned();
    let text = bare(new_text);
    if bare(&old) == text {
        return Ok(block.to_vec());
    }
    let paragraphs: Vec<&str> = text.split('\r').collect();
    let units = text.encode_utf16().count() as i64 + 1;

    if paragraphs.len() == 1
        && old.encode_utf16().count() == text.encode_utf16().count()
    {
        // Same length, same paragraph structure: splice in place. Photoshop
        // notices the stale layout by itself for this case.
        let (start, end) = text_span(block).ok_or_else(|| Error::Txt2("text run not found".into()))?;
        let mut out = block.to_vec();
        out.splice(start..end, utf16(&text));
        return Ok(out);
    }

    let (start, end) = text_span(block).ok_or_else(|| Error::Txt2("text run not found".into()))?;
    let mut out = block.to_vec();
    out.splice(start..end, utf16(&text));

    let trace = std::env::var("PHOTOCRAFT_TRACE_TXT2").is_ok();
    if trace { eprintln!("replace_text: spliced, /F = {}", find(&out, b"/99 /F").is_some()); }
    // One style run for the whole text; the terminator counts as a unit.
    out = collapse_table(&out, &table_path(true))?;
    if trace { eprintln!("style collapse, /F = {}", find(&out, b"/99 /F").is_some()); }
    out = set_table_length(&out, &table_path(true), 0, units)?;
    if trace { eprintln!("style length, /F = {}", find(&out, b"/99 /F").is_some()); }

    // One paragraph run per paragraph, each owning its U+000D break.
    out = collapse_table(&out, &table_path(false))?;
    if trace { eprintln!("para collapse, /F = {}", find(&out, b"/99 /F").is_some()); }
    out = replicate_table(&out, &table_path(false), paragraphs.len())?;
    if trace { eprintln!("para replicate, /F = {}", find(&out, b"/99 /F").is_some()); }
    for (i, para) in paragraphs.iter().enumerate() {
        let len = para.encode_utf16().count() as i64 + 1;
        out = set_table_length(&out, &table_path(false), i, len)?;
    }
    clear_cache(&out, 0)
}

/// Length of every paragraph run in UTF-16 units, in text order.
pub fn paragraph_run_lengths(block: &[u8]) -> Result<Vec<i64>, Error> {
    let toks = tokens(block)?;
    let open = resolve(&toks, &table_path(false)).ok_or_else(|| Error::Txt2("paragraph table not found".into()))?;
    let mut j = open + 1;
    let mut out = Vec::new();
    while !matches!(toks.get(j).ok_or_else(|| Error::Txt2("unterminated paragraph table".into()))?.kind, Kind::ArrayClose) {
        let next = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad paragraph entry".into()))?;
        let entry = toks[j].start..toks[next - 1].end;
        let entry = &block[entry];
        // `/1` inside the entry, before the closing dictionary run.
        let p = entry.windows(3).rposition(|w| w == b"/1 ").ok_or_else(|| Error::Txt2("paragraph run without length".into()))?;
        let rest = &entry[p + 3..];
        let digits: Vec<u8> = rest.iter().copied().take_while(|b| b.is_ascii_digit() || *b == b'-').collect();
        let len = std::str::from_utf8(&digits).ok().and_then(|t| t.parse().ok()).ok_or_else(|| Error::Txt2("bad paragraph length".into()))?;
        out.push(len);
        j = next;
    }
    Ok(out)
}

/// Writes `/1` (the run length) of entry `i` in a run table.
fn set_table_length(block: &[u8], table: &[Step<'_>], i: usize, len: i64) -> Result<Vec<u8>, Error> {
    let mut p = table.to_vec();
    p.push(Step::Index(i));
    p.push(Step::Key("1"));
    replace_value(block, &p, &Value::Int(len))
}

/// Grows a run table that holds one entry to `count` copies of that entry,
/// keeping every other one on its original length.
fn replicate_table(block: &[u8], table: &[Step<'_>], count: usize) -> Result<Vec<u8>, Error> {
    if count <= 1 {
        return Ok(block.to_vec());
    }
    let toks = tokens(block)?;
    let open = resolve(&toks, table).ok_or_else(|| Error::Txt2(format!("table not found: {table:?}")))?;
    let first = open + 1;
    let end = skip_value(&toks, first).ok_or_else(|| Error::Txt2("bad table entry".into()))?;
    let entry = block[toks[first].start..toks[end - 1].end].to_vec();
    let mut value = Vec::with_capacity(entry.len() * count + count + 2);
    value.extend_from_slice(b"[");
    for _ in 0..count {
        value.push(b' ');
        value.extend_from_slice(&entry);
    }
    value.extend_from_slice(b" ]");
    replace_container(block, table, &value)
}

/// Path to the style run table (`style`) or the paragraph run table.
fn table_path(style: bool) -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key(if style { "6" } else { "5" }),
        Step::Key("0"),
    ]
}

/// Replaces an array at `path` with its first element alone.
fn collapse_table(block: &[u8], path: &[Step<'_>]) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let open = resolve(&toks, path).ok_or_else(|| Error::Txt2(format!("table not found: {path:?}")))?;
    let first = skip_value(&toks, open + 1).map(|_| open + 1).filter(|&f| f < toks.len());
    let first = first.ok_or_else(|| Error::Txt2(format!("empty table: {path:?}")))?;
    let end = skip_value(&toks, first).ok_or_else(|| Error::Txt2("bad first element".into()))?;
    let (s, e) = (toks[first].start, toks[end - 1].end);
    let mut value = Vec::with_capacity(e - s + 4);
    value.extend_from_slice(b"[ ");
    value.extend_from_slice(&block[s..e]);
    value.extend_from_slice(b" ]");
    replace_container(block, path, &value)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal block with the same shape around the text run as a real `Txt2`.
    fn sample(text: &str) -> Vec<u8> {
        let mut block = Vec::new();
        block.extend_from_slice(TEXT_ANCHOR);
        block.extend_from_slice(&utf16(text));
        block.extend_from_slice(b") /5 << /0 [ << /0 << /0 << /0 (\xfe\xff) /5 1 >> >> /1 ");
        block
    }

    #[test]
    fn reads_the_anchored_text() {
        assert_eq!(read_text(&sample("Hello World")).as_deref(), Some("Hello World"));
        assert_eq!(read_text(b"no text here"), None);
    }

    #[test]
    fn same_length_replacement() {
        let block = sample("Hello World");
        let replaced = replace_text(&block, "World Hello").unwrap();
        assert_eq!(read_text(&replaced).as_deref(), Some("World Hello"));
        assert_eq!(replaced.len(), block.len());
    }

    #[test]
    fn length_change_needs_the_full_block() {
        // The synthetic block has no run tables, so a length change cannot
        // rebuild them; a real block does (see tests/txt2_write.rs).
        assert!(replace_text(&sample("Hello World"), "Hi").is_err());
    }

    #[test]
    fn other_bom_strings_are_ignored() {
        let mut block = sample("Hello World");
        let mut tail = utf16("AVeryLongFontNameThatIsLongerThanTheText");
        tail.extend_from_slice(b" tail");
        block.extend_from_slice(&tail);
        assert_eq!(read_text(&block).as_deref(), Some("Hello World"));
    }
}

// --------------------------------------------------------------------------- //
// Token table
// --------------------------------------------------------------------------- //

/// One token of the CoolType stream, with its byte span in the block.
///
/// Editing a value means replacing exactly `start..end`, so the rest of the
/// block — including Photoshop's own spacing — stays untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    /// Bytes that separate this token from the previous one.
    pub sep_start: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    DictOpen,
    DictClose,
    ArrayOpen,
    ArrayClose,
    Name(String),
    Int(i64),
    Real(f64),
    /// Raw string body, including the BOM and terminator, escaped as stored.
    Str(Vec<u8>),
    Bool(bool),
    Nil,
}

impl Kind {
    /// Returns the value as a name (without the leading slash).
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Kind::Name(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Kind::Int(i) => Some(*i),
            _ => None,
        }
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x00)
}

/// Splits a `Txt2` block into tokens. Fails on anything the grammar does not
/// cover: an unterminated string, dict or array, or a stray byte.
pub fn tokens(block: &[u8]) -> Result<Vec<Token>, Error> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < block.len() {
        let sep_start = i;
        while i < block.len() && is_space(block[i]) {
            i += 1;
        }
        if i >= block.len() {
            break;
        }
        let start = i;
        let kind = match block[i] {
            b'<' if block.get(i + 1) == Some(&b'<') => {
                i += 2;
                Kind::DictOpen
            }
            b'>' if block.get(i + 1) == Some(&b'>') => {
                i += 2;
                Kind::DictClose
            }
            b'[' => {
                i += 1;
                Kind::ArrayOpen
            }
            b']' => {
                i += 1;
                Kind::ArrayClose
            }
            b'/' => {
                i += 1;
                let from = i;
                while i < block.len() && !is_space(block[i]) && !b"<>[]()".contains(&block[i]) {
                    i += 1;
                }
                let name = String::from_utf8_lossy(&block[from..i]).into_owned();
                if name == "nil" {
                    Kind::Nil
                } else {
                    Kind::Name(name)
                }
            }
            b'(' => {
                i += 1;
                let from = i;
                let mut depth = 1;
                while i < block.len() {
                    match block[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if i >= block.len() {
                    return Err(Error::Txt2("unterminated string".into()));
                }
                let body = block[from..i].to_vec();
                i += 1;
                Kind::Str(body)
            }
            _ => {
                let from = i;
                while i < block.len() && !is_space(block[i]) && !b"<>[]()".contains(&block[i]) {
                    i += 1;
                }
                let text = std::str::from_utf8(&block[from..i])
                    .map_err(|_| Error::Txt2("non-UTF-8 token".into()))?;
                match text {
                    "true" => Kind::Bool(true),
                    "false" => Kind::Bool(false),
                    _ => {
                        if text.contains('.') {
                            Kind::Real(
                                text.parse()
                                    .map_err(|_| Error::Txt2(format!("bad real {text}")))?,
                            )
                        } else {
                            Kind::Int(
                                text.parse()
                                    .map_err(|_| Error::Txt2(format!("bad int {text}")))?,
                            )
                        }
                    }
                }
            }
        };
        out.push(Token { kind, start, end: i, sep_start });
    }
    Ok(out)
}

/// Rebuilds the block from its tokens; identical to the input when
/// [`tokens`] accepted it.
pub fn untokenize(block: &[u8], tokens: &[Token]) -> Vec<u8> {
    let mut out = Vec::with_capacity(block.len());
    for (n, t) in tokens.iter().enumerate() {
        if n == 0 {
            out.extend_from_slice(&block[..t.start]);
        } else {
            out.extend_from_slice(&block[t.sep_start..t.start]);
        }
        out.extend_from_slice(&block[t.start..t.end]);
    }
    if let Some(last) = tokens.last() {
        out.extend_from_slice(&block[last.end..]);
    }
    out
}

/// Position of the token that follows `index` after skipping one complete
/// value (a scalar, or a balanced dict/array).
pub fn skip_value(tokens: &[Token], index: usize) -> Option<usize> {
    match tokens.get(index)?.kind {
        Kind::DictOpen | Kind::ArrayOpen => {
            let mut depth = 0i32;
            for (i, t) in tokens.iter().enumerate().skip(index) {
                match t.kind {
                    Kind::DictOpen | Kind::ArrayOpen => depth += 1,
                    Kind::DictClose | Kind::ArrayClose => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => Some(index + 1),
    }
}

/// Value token positions in the dict that starts at `open`, in order.
pub fn dict_entries(tokens: &[Token], open: usize) -> Option<Vec<(String, usize)>> {
    if !matches!(tokens.get(open)?.kind, Kind::DictOpen) {
        return None;
    }
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < tokens.len() {
        match &tokens[i].kind {
            Kind::DictClose => return Some(out),
            Kind::Name(key) => {
                let key = key.clone();
                let value = i + 1;
                out.push((key, value));
                i = skip_value(tokens, value)?;
            }
            _ => return None,
        }
    }
    None
}

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

fn encode_value(v: &Value) -> Vec<u8> {
    match v {
        Value::Int(i) => i.to_string().into_bytes(),
        Value::Real(r) => format_real(*r).into_bytes(),
        Value::Bool(b) => if *b { b"true".to_vec() } else { b"false".to_vec() },
        Value::Name(n) => format!("/{n}").into_bytes(),
    }
}

/// CoolType real format: `%.5f` with trailing zeros trimmed, at least one decimal.
fn format_real(r: f64) -> String {
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
fn resolve(tokens: &[Token], path: &[Step<'_>]) -> Option<usize> {
    let mut cur: Option<usize> = None;
    for step in path {
        cur = Some(match (step, cur) {
            (Step::Key(k), None) => {
                top_level(tokens)?.into_iter().find(|(n, _)| n.as_str() == *k)?.1
            }
            (Step::Key(k), Some(open)) => {
                dict_entries(tokens, open)?.into_iter().find(|(n, _)| n.as_str() == *k)?.1
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
fn value_span(tokens: &[Token], path: &[Step<'_>]) -> Option<(usize, usize)> {
    let v = resolve(tokens, path)?;
    let after = skip_value(tokens, v)?;
    Some((tokens[v].start, tokens[after - 1].end))
}

/// Byte range of the value at `path` in `block`.
pub fn value_range(block: &[u8], path: &[Step<'_>]) -> Result<(usize, usize), Error> {
    let toks = tokens(block)?;
    value_span(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))
}

/// Paragraph-level properties Photoshop keeps per paragraph run in `Txt2`.
///
/// Keys are the ones Photoshop writes in the paragraph entry's property dict
/// (`/1/1/[0]/0/5/0/<run>/0/0/5`); each was measured by varying one control in
/// Photoshop and diffing two documents (`examples/txt2_diff`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParagraphProperty {
    /// `/0`: `0` left, `1` right, `2` centre, `6` justify all.
    Justification,
    /// `/1`, points: first-line indent.
    FirstLineIndent,
    /// `/2`, points: left (start) indent.
    StartIndent,
    /// `/3`, points: right (end) indent.
    EndIndent,
    /// `/4`, points: space before the paragraph.
    SpaceBefore,
    /// `/5`, points: space after the paragraph.
    SpaceAfter,
    /// `/7`, fraction of the size: the auto-leading ratio.
    AutoLeading,
    /// `/29`, boolean: every-line composer.
    EveryLineComposer,
    /// `/17`, `[min desired max]` word spacing as fractions of the space width.
    WordSpacing,
    /// `/18`, `[min desired max]`: letter spacing, fractions of the em.
    LetterSpacing,
    /// `/19`, `[min desired max]`: glyph scaling.
    GlyphScaling,
}

impl ParagraphProperty {
    fn key(self) -> &'static str {
        match self {
            ParagraphProperty::Justification => "0",
            ParagraphProperty::FirstLineIndent => "1",
            ParagraphProperty::StartIndent => "2",
            ParagraphProperty::EndIndent => "3",
            ParagraphProperty::SpaceBefore => "4",
            ParagraphProperty::SpaceAfter => "5",
            ParagraphProperty::AutoLeading => "7",
            ParagraphProperty::WordSpacing => "17",
            ParagraphProperty::LetterSpacing => "18",
            ParagraphProperty::GlyphScaling => "19",
            ParagraphProperty::EveryLineComposer => "29",
        }
    }
}

/// Path to the layer's antialias code (`/1/1/[0]/0/10/[0]`), the same code
/// Photoshop stores in `EngineData`'s `EngineDict/AntiAlias`.
pub fn antialias_path() -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("10"),
        Step::Key("0"),
    ]
}

/// Writes the layer's antialias code (`raster::AntiAlias::code`).
pub fn set_antialias(block: &[u8], code: i64) -> Result<Vec<u8>, Error> {
    replace_value(block, &antialias_path(), &Value::Int(code))
}

/// Path to paragraph run `i`'s property dictionary.
pub fn paragraph_props_path(i: usize) -> Vec<Step<'static>> {
    let mut p = table_path(false);
    p.push(Step::Index(i));
    p.push(Step::Key("0"));
    p.push(Step::Key("0"));
    p.push(Step::Key("5"));
    p
}

/// Number of paragraph runs in the layer text.
pub fn paragraph_count(block: &[u8]) -> Result<usize, Error> {
    Ok(paragraph_run_lengths(block)?.len())
}

/// Writes one scalar paragraph property of paragraph run `i`.
pub fn set_paragraph_property(
    block: &[u8],
    i: usize,
    property: ParagraphProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    replace_value(block, &path, value)
}

/// Writes a `[min desired max]` triple (word/letter spacing, glyph scaling).
pub fn set_paragraph_range(
    block: &[u8],
    i: usize,
    property: ParagraphProperty,
    values: [f32; 3],
) -> Result<Vec<u8>, Error> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    let value = format!(
        "[ {} {} {} ]",
        format_real(f64::from(values[0])),
        format_real(f64::from(values[1])),
        format_real(f64::from(values[2]))
    );
    replace_container(block, &path, value.as_bytes())
}

/// Replaces the scalar at `path` with `value`. Containers are refused: the
/// rest of the block depends on their contents, so replacing one wholesale is
/// never what a caller means.
pub fn replace_value(block: &[u8], path: &[Step<'_>], value: &Value) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let v = resolve(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))?;
    if matches!(toks[v].kind, Kind::DictOpen | Kind::ArrayOpen) {
        return Err(Error::Txt2(format!("path is a container, not a scalar: {path:?}")));
    }
    let (start, end) = value_span(&toks, path).ok_or_else(|| Error::Txt2("no span".into()))?;
    let mut out = block.to_vec();
    out.splice(start..end, encode_value(value));
    Ok(out)
}

// Run table. The layer text's style runs are `1 / 1[0] / 0 / 0 / 6 / 0[i]`:
// one dict per run. Each run dict holds its length in UTF-16 units at `/1`
// and its font size at `/0 /0 /6 /1`. Per-glyph advance and bounds tables
// (`1.0.9`, `1.1.0.1`) are not rebuilt by this module.

fn run_array() -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("6"),
        Step::Key("0"),
    ]
}

/// Path to style run `run` (plus `tail`), for callers outside this module.
pub fn run_path_for(run: usize) -> Vec<Step<'static>> {
    run_path(run, &[])
}

fn run_path<'a>(run: usize, tail: &[Step<'a>]) -> Vec<Step<'a>> {
    let mut p = run_array();
    p.push(Step::Index(run));
    p.extend_from_slice(tail);
    p
}

fn run_count_in(toks: &[Token]) -> Result<usize, Error> {
    let open = resolve(toks, &run_array()).ok_or_else(|| Error::Txt2("run table not found".into()))?;
    if !matches!(toks[open].kind, Kind::ArrayOpen) {
        return Err(Error::Txt2("run table is not an array".into()));
    }
    let mut j = open + 1;
    let mut n = 0;
    while !matches!(toks.get(j).ok_or_else(|| Error::Txt2("unterminated run table".into()))?.kind, Kind::ArrayClose) {
        j = skip_value(toks, j).ok_or_else(|| Error::Txt2("bad run element".into()))?;
        n += 1;
    }
    Ok(n)
}

fn run_lengths_in(toks: &[Token]) -> Result<Vec<i64>, Error> {
    let n = run_count_in(toks)?;
    (0..n)
        .map(|i| {
            let v = resolve(toks, &run_path(i, &[Step::Key("1")]))
                .ok_or_else(|| Error::Txt2(format!("run {i} has no length")))?;
            toks[v]
                .kind
                .as_int()
                .ok_or_else(|| Error::Txt2(format!("run {i} length is not an integer")))
        })
        .collect()
}

/// Number of style runs in the layer text.
pub fn run_count(block: &[u8]) -> Result<usize, Error> {
    run_count_in(&tokens(block)?)
}

/// Length of every style run in UTF-16 units, in text order.
pub fn run_lengths(block: &[u8]) -> Result<Vec<i64>, Error> {
    run_lengths_in(&tokens(block)?)
}

/// Writes new run lengths. The count must match the run table and the sum must
/// cover the whole text. [INFERENCE]: run lengths include the U+000D terminator
/// (plain_latin: text of 29 units, run length 30).
pub fn set_run_lengths(block: &[u8], lengths: &[i64]) -> Result<Vec<u8>, Error> {
    let n = run_count(block)?;
    if lengths.len() != n {
        return Err(Error::Txt2(format!("{} lengths for {} runs", lengths.len(), n)));
    }
    // The layer string's raw body is BOM + units (CR terminator included), so
    // its unit count is (len - 2) / 2. read_text stops at the first CR and would
    // undercount multi-paragraph text.
    let toks = tokens(block)?;
    let text = resolve(
        &toks,
        &[Step::Key("1"), Step::Key("1"), Step::Index(0), Step::Key("0"), Step::Key("0")],
    )
    .ok_or_else(|| Error::Txt2("layer text not found".into()))?;
    let units = match &toks[text].kind {
        Kind::Str(body) => (body.len() as i64 - 2) / 2,
        _ => return Err(Error::Txt2("layer text is not a string".into())),
    };
    let total: i64 = lengths.iter().sum();
    if total != units {
        return Err(Error::Txt2(format!("run lengths sum to {total}, text needs {units}")));
    }
    let mut out = block.to_vec();
    for (i, len) in lengths.iter().enumerate() {
        out = replace_value(&out, &run_path(i, &[Step::Key("1")]), &Value::Int(*len))?;
    }
    Ok(out)
}

/// Sets the font size of style run `run`.
pub fn set_run_font_size(block: &[u8], run: usize, size: f32) -> Result<Vec<u8>, Error> {
    set_run_property(block, run, RunProperty::FontSize, &Value::Real(f64::from(size)))
}

/// Character properties Photoshop stores inside a `Txt2` style run, with the
/// key it uses in the run's `/6` dictionary (verified by diffing corpus files
/// that differ in exactly one parameter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunProperty {
    /// `/6 /1`, points.
    FontSize,
    /// `/6 /4`, boolean: derive the line advance from the auto-leading fraction.
    AutoLeading,
    /// `/6 /5`, points (auto leading stores `size * 1.2`).
    Leading,
    /// `/6 /6`, fraction (1.0 = 100%).
    HorizontalScale,
    /// `/6 /7`, fraction (1.0 = 100%).
    VerticalScale,
    /// `/6 /8`, 1/1000 em.
    Tracking,
    /// `/6 /9`, points (positive raises).
    BaselineShift,
}

impl RunProperty {
    fn key(self) -> &'static str {
        match self {
            RunProperty::FontSize => "1",
            RunProperty::AutoLeading => "4",
            RunProperty::Leading => "5",
            RunProperty::HorizontalScale => "6",
            RunProperty::VerticalScale => "7",
            RunProperty::Tracking => "8",
            RunProperty::BaselineShift => "9",
        }
    }
}

/// Writes one character property of style run `run`.
pub fn set_run_property(
    block: &[u8],
    run: usize,
    property: RunProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let path = run_path(run, &[Step::Key("0"), Step::Key("0"), Step::Key("6"), Step::Key(property.key())]);
    replace_value(block, &path, value)
}

fn font_set_path() -> Vec<Step<'static>> {
    vec![Step::Key("0"), Step::Key("1"), Step::Key("0")]
}

/// Number of fonts in the block's `FontSet`.
pub fn font_count(block: &[u8]) -> Result<usize, Error> {
    let toks = tokens(block)?;
    let open = resolve(&toks, &font_set_path()).ok_or_else(|| Error::Txt2("font set not found".into()))?;
    let mut j = open + 1;
    let mut n = 0;
    while !matches!(toks.get(j).ok_or_else(|| Error::Txt2("unterminated font set".into()))?.kind, Kind::ArrayClose) {
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
    let open = resolve(&toks, &font_set_path()).ok_or_else(|| Error::Txt2("font set not found".into()))?;
    let mut j = open + 1;
    let mut index = 0;
    while !matches!(toks[j].kind, Kind::ArrayClose) {
        let next = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad font entry".into()))?;
        if block[toks[j].start..toks[next - 1].end].windows(encoded.len()).any(|w| w == encoded.as_slice()) {
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
    let path = run_path(run, &[Step::Key("0"), Step::Key("0"), Step::Key("6"), Step::Key("0")]);
    replace_value(block, &path, &Value::Int(index as i64))
}

/// Writes a run's fill colour as Photoshop's `SimplePaint` dictionary:
/// `/53 << /99 /SimplePaint /0 << /0 1 /1 [ a r g b ] >> >>` (alpha first).
pub fn set_run_fill_color(block: &[u8], run: usize, rgba: [f32; 4]) -> Result<Vec<u8>, Error> {
    let path = run_path(run, &[Step::Key("0"), Step::Key("0"), Step::Key("6"), Step::Key("53")]);
    let value = format!(
        "<< /99 /SimplePaint /0 << /0 1 /1 [ {} {} {} {} ] >> >>",
        format_real(f64::from(rgba[0])),
        format_real(f64::from(rgba[1])),
        format_real(f64::from(rgba[2])),
        format_real(f64::from(rgba[3])),
    );
    replace_container(block, &path, value.as_bytes())
}

/// Reads a run's fill colour, if it carries one.
pub fn run_fill_color(block: &[u8], run: usize) -> Option<[f32; 4]> {
    let path = run_path(run, &[Step::Key("0"), Step::Key("0"), Step::Key("6"), Step::Key("53")]);
    let (s, e) = value_span(&tokens(block).ok()?, &path)?;
    let text = std::str::from_utf8(&block[s..e]).ok()?;
    let at = text.find("/1 [")? + 4;
    let body = text[at..].split(']').next()?;
    let nums: Vec<f32> = body.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    match nums.as_slice() {
        [a, r, g, b] => Some([*a, *r, *g, *b]),
        _ => None,
    }
}

/// Splits the single style run of a freshly written block into runs of the
/// given lengths, in text order. The last length takes the U+000D terminator.
pub fn set_run_layout(block: &[u8], lengths: &[i64]) -> Result<Vec<u8>, Error> {
    let n = run_count(block)?;
    if n != 1 {
        return Err(Error::Txt2(format!("expected one run, found {n}")));
    }
    let mut out = block.to_vec();
    for _ in 1..lengths.len() {
        out = duplicate_run(&out, 0)?;
    }
    let total = run_lengths(&out)?.iter().sum::<i64>();
    let mut wanted = lengths.to_vec();
    let sum: i64 = wanted.iter().sum();
    if sum + 1 == total {
        // The layer string carries a terminating U+000D that the last run owns.
        let last = wanted.len() - 1;
        wanted[last] += 1;
    }
    set_run_lengths(&out, &wanted)
}

/// Appends a copy of style run `from` to the run table. The copy takes one unit
/// from the last run, so the lengths still cover the text exactly. Fails when
/// the last run has only one unit. [INFERENCE]: per-glyph tables are not updated.
pub fn duplicate_run(block: &[u8], from: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let n = run_count_in(&toks)?;
    if from >= n {
        return Err(Error::Txt2(format!("run {from} out of {n}")));
    }
    let mut lengths = run_lengths_in(&toks)?;
    // The new run takes one unit from the longest run, so repeated splits
    // always have a donor.
    let donor = (0..n)
        .max_by_key(|&i| lengths[i])
        .filter(|&i| lengths[i] >= 2)
        .ok_or_else(|| Error::Txt2("no run has a spare unit to split off".into()))?;
    let (rs, re) = value_span(&toks, &run_path(from, &[]))
        .ok_or_else(|| Error::Txt2("source run not found".into()))?;
    let open = resolve(&toks, &run_array()).ok_or_else(|| Error::Txt2("run table not found".into()))?;
    let mut j = open + 1;
    while !matches!(toks[j].kind, Kind::ArrayClose) {
        j = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad run element".into()))?;
    }
    let close = toks[j].start;
    let clone = block[rs..re].to_vec();
    let mut out = block.to_vec();
    let mut insert = Vec::with_capacity(clone.len() + 2);
    insert.push(b' ');
    insert.extend_from_slice(&clone);
    insert.push(b' ');
    out.splice(close..close, insert);
    lengths[donor] -= 1;
    lengths.push(1);
    set_run_lengths(&out, &lengths)
}

/// Empties the composed-layout cache of text object `object` (the `/2` of its
/// view), so the next application recomposes from the text and the run tables.
///
/// The key stays: Photoshop 2025 crashes on a block in which no object has a
/// `/2`, while an empty array is accepted and simply means "not laid out".
pub fn clear_cache(block: &[u8], object: usize) -> Result<Vec<u8>, Error> {
    // Area text keeps its frame rectangle inside the layout container: the
    // `/99 /F` dict holds the box in `/1` and the composed glyphs in `/6`.
    // Emptying the whole container deletes the box, and emptying only the
    // glyph cache makes Photoshop hang on open, so area text keeps its layout
    // container; a changed `/1` rectangle is what forces a recomposition.
    let toks = tokens(block)?;
    if frame_entries(&toks).is_some() {
        return Ok(block.to_vec());
    }
    let steps = vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
        Step::Key("2"),
    ];
    replace_container(block, &steps, b"[ ]")
}

/// Replaces the value at `path` with arbitrary bytes (`[ ]` for an empty array).
pub fn replace_container(block: &[u8], path: &[Step<'_>], value: &[u8]) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let (start, end) = value_span(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))?;
    let mut out = block.to_vec();
    out.splice(start..end, value.iter().copied());
    Ok(out)
}

/// Area-text frame rectangle, in the order stored in the `/1` array.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameGeometry {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Token index of the `[` opening the `/1` array of the dict whose `/99` is
/// the name `F`. Found by walking every dict; no byte offsets are assumed.
fn frame_array(toks: &[Token]) -> Option<usize> {
    let entries = frame_entries(toks)?;
    let arr = entries.iter().find(|(k, _)| k.as_str() == "1")?.1;
    if matches!(toks[arr].kind, Kind::ArrayOpen) { Some(arr) } else { None }
}

/// Entries of the `/99 /F` dict (the area-text frame), if the block has one.
fn frame_entries(toks: &[Token]) -> Option<Vec<(String, usize)>> {
    for (i, t) in toks.iter().enumerate() {
        if !matches!(t.kind, Kind::DictOpen) {
            continue;
        }
        let Some(entries) = dict_entries(toks, i) else { continue };
        let is_frame = entries.iter().any(|(k, v)| {
            k.as_str() == "99" && matches!(&toks[*v].kind, Kind::Name(n) if n.as_str() == "F")
        });
        // Point text carries a `/99 /F` node too, but only area text stores the
        // box rectangle in its `/1` array.
        let has_rect = entries
            .iter()
            .any(|(k, v)| k.as_str() == "1" && matches!(toks[*v].kind, Kind::ArrayOpen));
        if is_frame && has_rect {
            return Some(entries);
        }
    }
    None
}

/// Token indices of the four elements of the array opened at `arr`.
fn frame_values(toks: &[Token], arr: usize) -> Option<[usize; 4]> {
    let mut j = arr + 1;
    let mut pos = [0usize; 4];
    for slot in pos.iter_mut() {
        *slot = j;
        j = skip_value(toks, j)?;
    }
    if !matches!(toks.get(j)?.kind, Kind::ArrayClose) {
        return None;
    }
    Some(pos)
}

fn number(kind: &Kind) -> Option<f64> {
    match kind {
        Kind::Real(r) => Some(*r),
        Kind::Int(i) => Some(*i as f64),
        _ => None,
    }
}

/// Frame rectangle of area text. `None` for point text (no `/99 /F` dict).
pub fn frame_geometry(block: &[u8]) -> Option<FrameGeometry> {
    let toks = tokens(block).ok()?;
    let arr = frame_array(&toks)?;
    let pos = frame_values(&toks, arr)?;
    let mut v = [0f32; 4];
    for (slot, &i) in v.iter_mut().zip(pos.iter()) {
        *slot = number(&toks[i].kind)? as f32;
    }
    Some(FrameGeometry { left: v[0], top: v[1], right: v[2], bottom: v[3] })
}

/// Writes the frame rectangle in place, keeping the stored order and every
/// other byte of the block.
pub fn set_frame_geometry(block: &[u8], g: FrameGeometry) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let arr = frame_array(&toks).ok_or_else(|| Error::Txt2("frame geometry not found".into()))?;
    let pos = frame_values(&toks, arr).ok_or_else(|| Error::Txt2("frame array malformed".into()))?;
    let vals = [g.left, g.top, g.right, g.bottom];
    let mut out = block.to_vec();
    // Reverse order keeps the original byte offsets of earlier elements valid.
    for (idx, &p) in pos.iter().enumerate().rev() {
        let t = &toks[p];
        out.splice(t.start..t.end, format_real(f64::from(vals[idx])).into_bytes());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Run-table rebuild
// ---------------------------------------------------------------------------
