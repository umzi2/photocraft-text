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
    push_escaped(&mut out, s);
    out.extend_from_slice(&TERMINATOR);
    out
}

/// Pushes UTF-16BE bytes with CoolType's string escapes.  A literal `(`, `)`
/// or `\` inside a parenthesised string must be backslash-escaped; without
/// this a single parenthesis in the layer text truncates the token and
/// Photoshop refuses to open the file (reproduced with `out/demo/03`).  The
/// escapes are byte-wise, so the low byte of the UTF-16 code unit is escaped.
fn push_escaped(out: &mut Vec<u8>, s: &str) {
    for unit in s.encode_utf16() {
        for byte in unit.to_be_bytes() {
            if matches!(byte, b'(' | b')' | b'\\') {
                out.push(b'\\');
            }
            out.push(byte);
        }
    }
}

/// BOM-prefixed UTF-16BE with no terminator, as used for names (fonts, sheets).
fn utf16_name(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2 + 2);
    out.extend_from_slice(&BOM);
    push_escaped(&mut out, s);
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
    // The body can contain backslash escapes, so scan byte-wise: the only
    // `0x00 0x0d` pair in a text string is the terminator (a U+0D00 unit is
    // stored as `0d 00`, and a literal U+000D is not allowed in the text).
    let mut j = start + 2;
    while j + 3 <= block.len() {
        if block[j..j + 2] == TERMINATOR && block[j + 2] == b')' {
            return Some((start, j + 2));
        }
        j += 1;
    }
    None
}

/// The layer text as Photoshop sees it.
pub fn read_text(block: &[u8]) -> Option<String> {
    let (start, end) = text_span(block)?;
    let body = &block[start + 2..end - 2];
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
    let bare = |t: &str| {
        t.replace("\r\n", "\r")
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    };
    let text = bare(new_text);
    if bare(&old) == text {
        return Ok(block.to_vec());
    }
    let paragraphs: Vec<&str> = text.split('\r').collect();
    let units = text.encode_utf16().count() as i64 + 1;

    if paragraphs.len() == 1 && old.encode_utf16().count() == text.encode_utf16().count() {
        // Same length, same paragraph structure: splice in place. Photoshop
        // notices the stale layout by itself for this case.
        let (start, end) =
            text_span(block).ok_or_else(|| Error::Txt2("text run not found".into()))?;
        let mut out = block.to_vec();
        out.splice(start..end, utf16(&text));
        return Ok(out);
    }

    let (start, end) = text_span(block).ok_or_else(|| Error::Txt2("text run not found".into()))?;
    let mut out = block.to_vec();
    out.splice(start..end, utf16(&text));

    let trace = std::env::var("PHOTOCRAFT_TRACE_TXT2").is_ok();
    if trace {
        eprintln!(
            "replace_text: spliced, /F = {}",
            find(&out, b"/99 /F").is_some()
        );
    }
    // One style run for the whole text; the terminator counts as a unit.
    out = collapse_table(&out, &table_path(true))?;
    if trace {
        eprintln!("style collapse, /F = {}", find(&out, b"/99 /F").is_some());
    }
    out = set_table_length(&out, &table_path(true), 0, units)?;
    if trace {
        eprintln!("style length, /F = {}", find(&out, b"/99 /F").is_some());
    }

    // One paragraph run per paragraph, each owning its U+000D break.
    out = collapse_table(&out, &table_path(false))?;
    if trace {
        eprintln!("para collapse, /F = {}", find(&out, b"/99 /F").is_some());
    }
    out = replicate_table(&out, &table_path(false), paragraphs.len())?;
    if trace {
        eprintln!("para replicate, /F = {}", find(&out, b"/99 /F").is_some());
    }
    for (i, para) in paragraphs.iter().enumerate() {
        let len = para.encode_utf16().count() as i64 + 1;
        out = set_table_length(&out, &table_path(false), i, len)?;
    }
    clear_cache(&out, 0)
}

/// Length of every paragraph run in UTF-16 units, in text order.
pub fn paragraph_run_lengths(block: &[u8]) -> Result<Vec<i64>, Error> {
    let toks = tokens(block)?;
    let open = resolve(&toks, &table_path(false))
        .ok_or_else(|| Error::Txt2("paragraph table not found".into()))?;
    let mut j = open + 1;
    let mut out = Vec::new();
    while !matches!(
        toks.get(j)
            .ok_or_else(|| Error::Txt2("unterminated paragraph table".into()))?
            .kind,
        Kind::ArrayClose
    ) {
        let next = skip_value(&toks, j).ok_or_else(|| Error::Txt2("bad paragraph entry".into()))?;
        let entry = toks[j].start..toks[next - 1].end;
        let entry = &block[entry];
        // `/1` inside the entry, before the closing dictionary run.
        let p = entry
            .windows(3)
            .rposition(|w| w == b"/1 ")
            .ok_or_else(|| Error::Txt2("paragraph run without length".into()))?;
        let rest = &entry[p + 3..];
        let digits: Vec<u8> = rest
            .iter()
            .copied()
            .take_while(|b| b.is_ascii_digit() || *b == b'-')
            .collect();
        let len = std::str::from_utf8(&digits)
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| Error::Txt2("bad paragraph length".into()))?;
        out.push(len);
        j = next;
    }
    Ok(out)
}

/// Writes `/1` (the run length) of entry `i` in a run table.
fn set_table_length(
    block: &[u8],
    table: &[Step<'_>],
    i: usize,
    len: i64,
) -> Result<Vec<u8>, Error> {
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
    let open =
        resolve(&toks, table).ok_or_else(|| Error::Txt2(format!("table not found: {table:?}")))?;
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
    let open =
        resolve(&toks, path).ok_or_else(|| Error::Txt2(format!("table not found: {path:?}")))?;
    let first = skip_value(&toks, open + 1)
        .map(|_| open + 1)
        .filter(|&f| f < toks.len());
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
        assert_eq!(
            read_text(&sample("Hello World")).as_deref(),
            Some("Hello World")
        );
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
        out.push(Token {
            kind,
            start,
            end: i,
            sep_start,
        });
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

impl Value {
    fn as_bool(&self) -> Option<bool> {
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
/// Photoshop and diffing two documents (`examples/txt2_diff`), or by authoring
/// the slot and letting Photoshop re-lay-out and re-save the layer
/// (`out/verify/*.psd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParagraphProperty {
    /// `/0`: `0` left, `1` right, `2` centre, `6` justify all
    /// (`out/probe2/just_right.psd`, `just_center.psd`, `out/verify/just_center.psd`).
    Justification,
    /// `/1`, points: first-line indent. Measured by authoring only: the DOM
    /// setters silently fail, `out/exp/area_indent.psd` proves the slot
    /// (pen x = layer 20 + start 18 + first 24).
    FirstLineIndent,
    /// `/2`, points: left (start) indent.
    StartIndent,
    /// `/3`, points: right (end) indent.
    EndIndent,
    /// `/4`, points: space before the paragraph. **Unconfirmed**: not measured
    /// differentially yet, kept for the existing writer's callers.
    SpaceBefore,
    /// `/5`, points: space after the paragraph. **Unconfirmed**, like `/4`.
    SpaceAfter,
    /// `/7`, fraction of the size: the auto-leading ratio (`1.2` default,
    /// `1.5` measured in `out/exp/auto15.psd`).
    AutoLeading,
    /// `/9`, boolean: hyphenate (`AutoHyphenate` in `EngineData`).
    Hyphenate,
    /// `/10`: hyphenate words longer than this many characters.
    HyphenateWordSize,
    /// `/11`: minimum characters before the hyphen.
    HyphenateAfterFirst,
    /// `/12`: minimum characters after the hyphen.
    HyphenateBeforeLast,
    /// `/13`: maximum consecutive hyphens.
    HyphenLimit,
    /// `/14`: hyphenation zone in points. A default paragraph sheet may
    /// materialise `36.0` (Photoshop's own default), but a measured value is
    /// stored 1:1: DOM 18 pt -> `/5/14 = 18.0` (`out/probe5/pE.psd`).
    HyphenationZone,
    /// `/15`: hyphenate capitalised words. No `EngineData` key is known for
    /// this one, so it reaches `Txt2` only.
    HyphenateCapitalWords,
    /// `/17`, `[min desired max]` word spacing as fractions of the space width.
    WordSpacing,
    /// `/18`, `[min desired max]`: letter spacing, fractions of the em.
    LetterSpacing,
    /// `/19`, `[min desired max]`: glyph scaling (`GlyphSpacing` in
    /// `EngineData`; `out/probe2/glyph_scaling.psd` measured `[.9 1.0 1.1]`).
    GlyphScaling,
    /// `/21`, boolean: hanging punctuation (`Hanging` in `EngineData`;
    /// `out/verify/hang.psd`).
    HangingPunctuation,
    /// `/29`, boolean: every-line composer (`out/probe2/composer_every.psd`
    /// could not measure it — the ExtendScript enum is invalid on CC 2019 — so
    /// the key comes from Photoshop's own paragraph dict).
    EveryLineComposer,
}

impl ParagraphProperty {
    /// The dict key Photoshop stores this property under.
    pub fn key(self) -> &'static str {
        match self {
            ParagraphProperty::Justification => "0",
            ParagraphProperty::FirstLineIndent => "1",
            ParagraphProperty::StartIndent => "2",
            ParagraphProperty::EndIndent => "3",
            ParagraphProperty::SpaceBefore => "4",
            ParagraphProperty::SpaceAfter => "5",
            ParagraphProperty::AutoLeading => "7",
            ParagraphProperty::Hyphenate => "9",
            ParagraphProperty::HyphenateWordSize => "10",
            ParagraphProperty::HyphenateAfterFirst => "11",
            ParagraphProperty::HyphenateBeforeLast => "12",
            ParagraphProperty::HyphenLimit => "13",
            ParagraphProperty::HyphenationZone => "14",
            ParagraphProperty::HyphenateCapitalWords => "15",
            ParagraphProperty::WordSpacing => "17",
            ParagraphProperty::LetterSpacing => "18",
            ParagraphProperty::GlyphScaling => "19",
            ParagraphProperty::HangingPunctuation => "21",
            ParagraphProperty::EveryLineComposer => "29",
        }
    }

    /// The property stored under `key`, or `None` for a key this module does
    /// not know. Unknown slots are refused rather than written.
    pub fn from_key(key: &str) -> Option<Self> {
        [
            Self::Justification,
            Self::FirstLineIndent,
            Self::StartIndent,
            Self::EndIndent,
            Self::SpaceBefore,
            Self::SpaceAfter,
            Self::AutoLeading,
            Self::Hyphenate,
            Self::HyphenateWordSize,
            Self::HyphenateAfterFirst,
            Self::HyphenateBeforeLast,
            Self::HyphenLimit,
            Self::HyphenationZone,
            Self::HyphenateCapitalWords,
            Self::WordSpacing,
            Self::LetterSpacing,
            Self::GlyphScaling,
            Self::HangingPunctuation,
            Self::EveryLineComposer,
        ]
        .into_iter()
        .find(|p| p.key() == key)
    }
}

/// Photoshop's paragraph justification code, as stored in `Txt2` `/5/0` and in
/// `EngineData` `Justification`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justification {
    /// `0`.
    Left,
    /// `1`.
    Right,
    /// `2`.
    Center,
    /// `3`.
    JustifyLastCenter,
    /// `4`.
    JustifyLastRight,
    /// `5`: justify every line, the last one included.
    JustifyAll,
    /// `6`: justify all but the last line (left).
    JustifyLastLeft,
}

impl Justification {
    pub fn code(self) -> i64 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Center => 2,
            Self::JustifyLastCenter => 3,
            Self::JustifyLastRight => 4,
            Self::JustifyAll => 5,
            Self::JustifyLastLeft => 6,
        }
    }

    /// Decodes a stored code; `None` for a value Photoshop does not define.
    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Left,
            1 => Self::Right,
            2 => Self::Center,
            3 => Self::JustifyLastCenter,
            4 => Self::JustifyLastRight,
            5 => Self::JustifyAll,
            6 => Self::JustifyLastLeft,
            _ => return None,
        })
    }
}

/// Hyphenation settings of one paragraph (`Txt2` `/5/9` … `/5/15`).
///
/// Every field is the value **as stored in `Txt2`**. One conversion is worth
/// knowing: Photoshop's DOM `hyphenateWordsLongerThan` is stored as that value
/// **+ 1** (`out/probe5/pE.psd`, `hyph_params`: DOM 6 → `/5/10 = 7`), while
/// `hyphenationZone` is stored in points unchanged (`DOM 18` → `/5/14 = 18.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hyphenation {
    /// `/9`: hyphenate at all.
    pub hyphenate: bool,
    /// `/10`: only words longer than this many characters — Photoshop's DOM
    /// value plus one.
    pub words_longer_than: i64,
    /// `/11`: characters required before the hyphen.
    pub after_first: i64,
    /// `/12`: characters required after the hyphen.
    pub before_last: i64,
    /// `/13`: maximum consecutive hyphens.
    pub limit: i64,
    /// `/14`, points: hyphenation zone.
    pub zone_pt: f32,
    /// `/15`: hyphenate capitalised words.
    pub capital_words: bool,
}

impl Default for Hyphenation {
    /// The values Photoshop's shared default paragraph sheet `/1/3` carries, so
    /// this is what it materialises when hyphenation is switched on without
    /// touching the parameters (`out/probe5/pE.psd`, `hyphenation`:
    /// `6 / 2 / 2 / 0 / 36.0 / true`). Note that `EngineData`'s default for
    /// `ConsecutiveHyphens` is `8` where `Txt2` stores `0` — the two models'
    /// default sheets disagree there, as `docs/txt2-map.md` records.
    fn default() -> Self {
        Self {
            hyphenate: false,
            words_longer_than: 6,
            after_first: 2,
            before_last: 2,
            limit: 0,
            zone_pt: 36.0,
            capital_words: true,
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

/// Path to the layer's antialias dictionary (`/1/1[0]/0/10`).
///
/// Photoshop omits the whole dictionary for `AntiAlias::Smooth` — smooth is
/// the implicit mode when the key is absent (`docs/txt2-map.md` §4,
/// `out/probe2/aa_smooth.psd`).
pub fn antialias_dict_path() -> Vec<Step<'static>> {
    vec![
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(0),
        Step::Key("0"),
        Step::Key("10"),
    ]
}

/// Writes the layer's antialias code (`raster::AntiAlias::code`).
///
/// `3` (smooth) is encoded by removing the `/10` dictionary, which is how
/// Photoshop stores it; every other code writes `/10/0`. A template that has
/// no `/10` dict (smooth) gets one back in Photoshop's own shape
/// `/10 << /0 <code> /2 true >>` when a non-smooth code is authored.
pub fn set_antialias(block: &[u8], code: i64) -> Result<Vec<u8>, Error> {
    let smooth = crate::raster::AntiAlias::Smooth.code();
    if code == smooth {
        return remove_entry(block, &antialias_dict_path());
    }
    let toks = tokens(block)?;
    if resolve(&toks, &antialias_dict_path()).is_none() {
        let value = format!("<< /0 {code} /2 true >>");
        return ensure_container(block, &antialias_dict_path(), value.as_bytes());
    }
    replace_value(block, &antialias_path(), &Value::Int(code))
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
///
/// Fails when the template does not carry the slot; use
/// [`set_paragraph_property_or_insert`] to add it.
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

/// Writes one scalar paragraph property, inserting it when the template omits
/// the slot (Photoshop leaves `/5/10` … `/5/15` out while hyphenation is off).
pub fn set_paragraph_property_or_insert(
    block: &[u8],
    i: usize,
    property: ParagraphProperty,
    value: &Value,
) -> Result<Vec<u8>, Error> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    ensure_value(block, &path, value)
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
    ensure_container(block, &path, value.as_bytes())
}

/// Reads one scalar paragraph property of paragraph run `i`.
///
/// `None` when the slot, the paragraph or the block is absent; the
/// `[min desired max]` triples are read with [`paragraph_range`].
pub fn paragraph_property(block: &[u8], i: usize, property: ParagraphProperty) -> Option<Value> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    scalar_at(block, &path)
}

/// Reads a `[min desired max]` triple, or `None` when the slot is absent.
pub fn paragraph_range(block: &[u8], i: usize, property: ParagraphProperty) -> Option<[f32; 3]> {
    let mut path = paragraph_props_path(i);
    path.push(Step::Key(property.key()));
    let (start, end) = value_span(&tokens(block).ok()?, &path)?;
    let text = std::str::from_utf8(&block[start..end]).ok()?;
    let body = text.strip_prefix('[')?.split(']').next()?;
    let nums: Vec<f32> = body
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    match nums.as_slice() {
        [a, b, c] => Some([*a, *b, *c]),
        _ => None,
    }
}

/// Justification of paragraph run `i` (`/5/0`).
pub fn set_paragraph_justification(
    block: &[u8],
    i: usize,
    justification: Justification,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::Justification,
        &Value::Int(justification.code()),
    )
}

/// Same, from a stored code. Rejects codes Photoshop does not define.
pub fn set_paragraph_justification_code(
    block: &[u8],
    i: usize,
    code: i64,
) -> Result<Vec<u8>, Error> {
    let j = Justification::from_code(code)
        .ok_or_else(|| Error::Txt2(format!("not a justification: {code} (0..6)")))?;
    set_paragraph_justification(block, i, j)
}

pub fn paragraph_justification(block: &[u8], i: usize) -> Option<Justification> {
    match paragraph_property(block, i, ParagraphProperty::Justification)? {
        Value::Int(code) => Justification::from_code(code),
        _ => None,
    }
}

/// First-line indent of paragraph run `i`, points (`/5/1`).
pub fn set_paragraph_first_line_indent(
    block: &[u8],
    i: usize,
    points: f32,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::FirstLineIndent,
        &Value::Real(f64::from(points)),
    )
}

pub fn paragraph_first_line_indent(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::FirstLineIndent)
}

/// Left (start) indent of paragraph run `i`, points (`/5/2`).
pub fn set_paragraph_start_indent(block: &[u8], i: usize, points: f32) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::StartIndent,
        &Value::Real(f64::from(points)),
    )
}

pub fn paragraph_start_indent(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::StartIndent)
}

/// Right (end) indent of paragraph run `i`, points (`/5/3`).
pub fn set_paragraph_end_indent(block: &[u8], i: usize, points: f32) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::EndIndent,
        &Value::Real(f64::from(points)),
    )
}

pub fn paragraph_end_indent(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::EndIndent)
}

/// Auto-leading fraction of paragraph run `i` (`/5/7`, `1.2` = 120%).
pub fn set_paragraph_auto_leading(block: &[u8], i: usize, fraction: f32) -> Result<Vec<u8>, Error> {
    if !(fraction.is_finite() && fraction > 0.0) {
        return Err(Error::Txt2(format!(
            "not an auto-leading fraction: {fraction}"
        )));
    }
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::AutoLeading,
        &Value::Real(f64::from(fraction)),
    )
}

pub fn paragraph_auto_leading(block: &[u8], i: usize) -> Option<f32> {
    paragraph_number(block, i, ParagraphProperty::AutoLeading)
}

/// Hanging punctuation of paragraph run `i` (`/5/21`).
pub fn set_paragraph_hanging_punctuation(
    block: &[u8],
    i: usize,
    on: bool,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::HangingPunctuation,
        &Value::Bool(on),
    )
}

pub fn paragraph_hanging_punctuation(block: &[u8], i: usize) -> Option<bool> {
    match paragraph_property(block, i, ParagraphProperty::HangingPunctuation)? {
        Value::Bool(b) => Some(b),
        _ => None,
    }
}

/// Every-line composer of paragraph run `i` (`/5/29`).
pub fn set_paragraph_every_line_composer(
    block: &[u8],
    i: usize,
    on: bool,
) -> Result<Vec<u8>, Error> {
    set_paragraph_property(
        block,
        i,
        ParagraphProperty::EveryLineComposer,
        &Value::Bool(on),
    )
}

pub fn paragraph_every_line_composer(block: &[u8], i: usize) -> Option<bool> {
    match paragraph_property(block, i, ParagraphProperty::EveryLineComposer)? {
        Value::Bool(b) => Some(b),
        _ => None,
    }
}

/// Writes the hyphenation slots of paragraph run `i` (`/5/9` … `/5/15`).
///
/// Photoshop only stores the detail keys while hyphenation is on — `/10` …
/// `/15` are absent in a plain run sheet — so they are inserted only when
/// `hyphenate` is true; switching hyphenation off writes `/9 false` and leaves
/// any existing detail keys alone. The zone is stored in points, the same
/// number `EngineData` `Zone` keeps (`out/probe5/pE.psd`: DOM 18 → `/5/14
/// 18.0`, `Zone 18.0`).
pub fn set_paragraph_hyphenation(
    block: &[u8],
    i: usize,
    hyphenation: Hyphenation,
) -> Result<Vec<u8>, Error> {
    if !(hyphenation.zone_pt.is_finite() && hyphenation.zone_pt >= 0.0) {
        return Err(Error::Txt2(format!(
            "not a hyphenation zone: {}",
            hyphenation.zone_pt
        )));
    }
    for (prop, value) in [
        (
            ParagraphProperty::HyphenateWordSize,
            hyphenation.words_longer_than,
        ),
        (
            ParagraphProperty::HyphenateAfterFirst,
            hyphenation.after_first,
        ),
        (
            ParagraphProperty::HyphenateBeforeLast,
            hyphenation.before_last,
        ),
        (ParagraphProperty::HyphenLimit, hyphenation.limit),
    ] {
        if value < 0 {
            return Err(Error::Txt2(format!(
                "negative hyphenation value {value} for {prop:?}"
            )));
        }
    }
    let mut out = block.to_vec();
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::Hyphenate,
        &Value::Bool(hyphenation.hyphenate),
    )?;
    if !hyphenation.hyphenate {
        return Ok(out);
    }
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateWordSize,
        &Value::Int(hyphenation.words_longer_than),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateAfterFirst,
        &Value::Int(hyphenation.after_first),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateBeforeLast,
        &Value::Int(hyphenation.before_last),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenLimit,
        &Value::Int(hyphenation.limit),
    )?;
    out = set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenationZone,
        &Value::Real(f64::from(hyphenation.zone_pt)),
    )?;
    set_paragraph_property_or_insert(
        &out,
        i,
        ParagraphProperty::HyphenateCapitalWords,
        &Value::Bool(hyphenation.capital_words),
    )
}

/// Hyphenation slots of paragraph run `i`.
///
/// A run sheet only carries `/10` … `/15` while hyphenation is on; absent
/// slots read back as the values Photoshop's shared default sheet `/1/3`
/// holds and materialises when hyphenation is switched on
/// (`out/probe5/pE.psd`). `None` when the paragraph run itself does not exist.
pub fn paragraph_hyphenation(block: &[u8], i: usize) -> Option<Hyphenation> {
    if paragraph_count(block).ok()? <= i {
        return None;
    }
    let default = Hyphenation::default();
    let int = |prop| match paragraph_property(block, i, prop) {
        Some(Value::Int(v)) => Some(v),
        _ => None,
    };
    let boolean = |prop, fallback| {
        paragraph_property(block, i, prop)
            .and_then(|v| v.as_bool())
            .unwrap_or(fallback)
    };
    Some(Hyphenation {
        hyphenate: boolean(ParagraphProperty::Hyphenate, default.hyphenate),
        words_longer_than: int(ParagraphProperty::HyphenateWordSize)
            .unwrap_or(default.words_longer_than),
        after_first: int(ParagraphProperty::HyphenateAfterFirst).unwrap_or(default.after_first),
        before_last: int(ParagraphProperty::HyphenateBeforeLast).unwrap_or(default.before_last),
        limit: int(ParagraphProperty::HyphenLimit).unwrap_or(default.limit),
        zone_pt: paragraph_number(block, i, ParagraphProperty::HyphenationZone)
            .unwrap_or(default.zone_pt),
        capital_words: boolean(
            ParagraphProperty::HyphenateCapitalWords,
            default.capital_words,
        ),
    })
}

fn paragraph_number(block: &[u8], i: usize, property: ParagraphProperty) -> Option<f32> {
    match paragraph_property(block, i, property)? {
        Value::Int(v) => Some(v as f32),
        Value::Real(v) => Some(v as f32),
        _ => None,
    }
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
    let open =
        resolve(toks, &run_array()).ok_or_else(|| Error::Txt2("run table not found".into()))?;
    if !matches!(toks[open].kind, Kind::ArrayOpen) {
        return Err(Error::Txt2("run table is not an array".into()));
    }
    let mut j = open + 1;
    let mut n = 0;
    while !matches!(
        toks.get(j)
            .ok_or_else(|| Error::Txt2("unterminated run table".into()))?
            .kind,
        Kind::ArrayClose
    ) {
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
        return Err(Error::Txt2(format!(
            "{} lengths for {} runs",
            lengths.len(),
            n
        )));
    }
    // The layer string's raw body is BOM + units (CR terminator included), so
    // its unit count is (len - 2) / 2. read_text stops at the first CR and would
    // undercount multi-paragraph text.
    let toks = tokens(block)?;
    let text = resolve(
        &toks,
        &[
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(0),
            Step::Key("0"),
            Step::Key("0"),
        ],
    )
    .ok_or_else(|| Error::Txt2("layer text not found".into()))?;
    let units = match &toks[text].kind {
        Kind::Str(body) => {
            // `body` is the raw token: BOM, UTF-16BE units (possibly with
            // backslash escapes) and the 00 0d terminator.  Unescape before
            // dividing, otherwise one parenthesis inflates the count.
            let mut raw = Vec::with_capacity(body.len());
            let mut i = 0;
            while i < body.len() {
                if body[i] == b'\\' && i + 1 < body.len() {
                    i += 1;
                }
                raw.push(body[i]);
                i += 1;
            }
            (raw.len() as i64 - 2) / 2
        }
        _ => return Err(Error::Txt2("layer text is not a string".into())),
    };
    let total: i64 = lengths.iter().sum();
    if total != units {
        return Err(Error::Txt2(format!(
            "run lengths sum to {total}, text needs {units}"
        )));
    }
    let mut out = block.to_vec();
    for (i, len) in lengths.iter().enumerate() {
        out = replace_value(&out, &run_path(i, &[Step::Key("1")]), &Value::Int(*len))?;
    }
    Ok(out)
}

/// Sets the font size of style run `run`.
pub fn set_run_font_size(block: &[u8], run: usize, size: f32) -> Result<Vec<u8>, Error> {
    set_run_property(
        block,
        run,
        RunProperty::FontSize,
        &Value::Real(f64::from(size)),
    )
}

/// Character properties Photoshop stores inside a `Txt2` style run, with the
/// key it uses in the run's `/6` dictionary.
///
/// Every slot listed here is either measured by diffing two documents that
/// differ in exactly one parameter (`out/probe2/*.psd`) or by authoring the
/// slot and letting Photoshop re-lay-out and re-save the layer
/// (`out/verify/*.psd`). Slots with no evidence are deliberately absent:
/// manual kerning amount `/10` (Photoshop reads it as 16.16 fixed point, see
/// `out/verify/mk100_res.psd`) and the faux-style stroke width keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunProperty {
    /// `/6 /0`: index into the block's font table (`ensure_font`).
    FontIndex,
    /// `/6 /1`, points (`out/mr`, `plain_latin`).
    FontSize,
    /// `/6 /2`, boolean: faux bold (`out/probe2/fauxbold.psd`, `false -> true`).
    FauxBold,
    /// `/6 /3`, boolean: faux italic (`out/probe2/fauxitalic.psd`).
    FauxItalic,
    /// `/6 /4`, boolean: derive the line advance from the auto-leading fraction
    /// (`out/probe2/lead40.psd`, `true -> false`).
    AutoLeading,
    /// `/6 /5`, points (auto leading stores `size * 1.2`;
    /// `out/probe2/lead40.psd`, `57.6 -> 40.0`).
    Leading,
    /// `/6 /6`, fraction (1.0 = 100%; `out/probe2/hscale150.psd`).
    HorizontalScale,
    /// `/6 /7`, fraction (1.0 = 100%).
    VerticalScale,
    /// `/6 /8`, 1/1000 em (`out/probe2/tracking100.psd`, `0 -> 100`).
    Tracking,
    /// `/6 /9`, points (positive raises).
    BaselineShift,
    /// `/6 /11`: `0` manual, `1` metrics, `2` optical
    /// (`out/probe2/kerning_manual.psd` `1 -> 0`, `kerning_optical.psd` `1 -> 2`).
    KerningType,
    /// `/6 /12`: `0` normal, `1` small caps, `2` all caps
    /// (`out/verify/caps.psd` authored `2`; Photoshop re-rendered ALL CAPS in
    /// its PDF and kept `2` after re-save).
    Capitalization,
    /// `/6 /15`: `0` off, `2` strike-box (`out/verify/strike.psd` authored `2`,
    /// kept after re-save).
    Strikethrough,
    /// `/6 /16`: `0` off, `1` right, `2` left (`out/verify/underline.psd`,
    /// both sides kept after re-save).
    Underline,
    /// `/6 /18`, boolean: standard ligatures. `out/verify/ligoff.psd` authored
    /// `false`; Photoshop re-saved `Ligatures false`.
    Ligatures,
    /// `/6 /19`, boolean: alternate (discretionary) ligatures.
    AlternateLigatures,
    /// `/6 /30`: old-style figures; Photoshop's own value for "on" in the
    /// measured files is `2` (`out/verify/oldstyle.psd`, kept after re-save).
    OldStyle,
    /// `/6 /38`: language code (`out/verify/lang.psd` authored `14`; Photoshop
    /// re-saved `Language 14`).
    Language,
    /// `/6 /52`, boolean: no break (keep the run on one line).
    NoBreak,
    /// `/6 /53`: fill paint (`SimplePaint`, see [`set_run_fill_color`]).
    FillPaint,
    /// `/6 /54`: stroke paint (`SimplePaint`).
    StrokePaint,
}

impl RunProperty {
    /// The character-sheet key Photoshop stores this property under.
    pub fn key(self) -> &'static str {
        match self {
            RunProperty::FontIndex => "0",
            RunProperty::FontSize => "1",
            RunProperty::FauxBold => "2",
            RunProperty::FauxItalic => "3",
            RunProperty::AutoLeading => "4",
            RunProperty::Leading => "5",
            RunProperty::HorizontalScale => "6",
            RunProperty::VerticalScale => "7",
            RunProperty::Tracking => "8",
            RunProperty::BaselineShift => "9",
            RunProperty::KerningType => "11",
            RunProperty::Capitalization => "12",
            RunProperty::Strikethrough => "15",
            RunProperty::Underline => "16",
            RunProperty::Ligatures => "18",
            RunProperty::AlternateLigatures => "19",
            RunProperty::OldStyle => "30",
            RunProperty::Language => "38",
            RunProperty::NoBreak => "52",
            RunProperty::FillPaint => "53",
            RunProperty::StrokePaint => "54",
        }
    }

    /// The property stored under `key`, or `None` for a slot this module does
    /// not know.
    pub fn from_key(key: &str) -> Option<Self> {
        [
            Self::FontIndex,
            Self::FontSize,
            Self::FauxBold,
            Self::FauxItalic,
            Self::AutoLeading,
            Self::Leading,
            Self::HorizontalScale,
            Self::VerticalScale,
            Self::Tracking,
            Self::BaselineShift,
            Self::KerningType,
            Self::Capitalization,
            Self::Strikethrough,
            Self::Underline,
            Self::Ligatures,
            Self::AlternateLigatures,
            Self::OldStyle,
            Self::Language,
            Self::NoBreak,
            Self::FillPaint,
            Self::StrokePaint,
        ]
        .into_iter()
        .find(|p| p.key() == key)
    }

    /// Whether the slot holds a paint dictionary rather than a scalar.
    pub fn is_paint(self) -> bool {
        matches!(self, Self::FillPaint | Self::StrokePaint)
    }
}

/// How Photoshop kerns a run (`Txt2` `/6/11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KerningType {
    /// `0`: manual values, stored per pair or as the run's `Kerning` amount.
    Manual,
    /// `1`: pair kerning from the font's `kern`/`GPOS` tables.
    Metrics,
    /// `2`: optical, Adobe's area-based spacing (`optical.rs` approximates it).
    Optical,
}

impl KerningType {
    pub fn code(self) -> i64 {
        match self {
            Self::Manual => 0,
            Self::Metrics => 1,
            Self::Optical => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Manual,
            1 => Self::Metrics,
            2 => Self::Optical,
            _ => return None,
        })
    }

    /// How `EngineData` represents this mode: it only carries the
    /// metrics/manual switch (`AutoKerning`), so optical stays on the metrics
    /// side — Photoshop's own `out/probe2/kerning_optical.psd` keeps
    /// `AutoKerning true`.
    pub fn engine_auto_kerning(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

/// Character case mapping of a run (`Txt2` `/6/12`, `EngineData` `FontCaps`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capitalization {
    /// `0`.
    Normal,
    /// `1`: small caps.
    SmallCaps,
    /// `2`: all caps (`out/verify/caps_res.psd` renders ALL CAPS).
    AllCaps,
}

impl Capitalization {
    pub fn code(self) -> i64 {
        match self {
            Self::Normal => 0,
            Self::SmallCaps => 1,
            Self::AllCaps => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Normal,
            1 => Self::SmallCaps,
            2 => Self::AllCaps,
            _ => return None,
        })
    }

    /// `EngineData` `FontCaps` uses the same three codes.
    pub fn engine_font_caps(self) -> i64 {
        self.code()
    }
}

/// Underline position of a run (`Txt2` `/6/16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    /// `0`: no underline.
    Off,
    /// `1`: underline on the right of vertical text.
    Right,
    /// `2`: underline on the left (ordinary horizontal text).
    Left,
}

impl Underline {
    pub fn code(self) -> i64 {
        match self {
            Self::Off => 0,
            Self::Right => 1,
            Self::Left => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Off,
            1 => Self::Right,
            2 => Self::Left,
            _ => return None,
        })
    }

    /// `EngineData` only has a boolean `Underline`.
    pub fn engine_underline(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Strikethrough style of a run (`Txt2` `/6/15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strikethrough {
    /// `0`: none.
    Off,
    /// `2`: a strike box (`out/verify/strike_res.psd` keeps `2`).
    StrikeBox,
}

impl Strikethrough {
    pub fn code(self) -> i64 {
        match self {
            Self::Off => 0,
            Self::StrikeBox => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Off,
            2 => Self::StrikeBox,
            _ => return None,
        })
    }

    /// `EngineData` only has a boolean `Strikethrough`.
    pub fn engine_strikethrough(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The `/6/30` value Photoshop keeps for old-style figures switched on, as
/// measured in `out/verify/oldstyle.psd`. There is no `EngineData` key for this
/// flag, so it reaches `Txt2` only.
pub const OLD_STYLE_ON: i64 = 2;

/// Language codes measured in a Photoshop re-save. The block stores an integer
/// index into Photoshop's language table; unknown values are still legal, so
/// [`set_run_language_code`] accepts a raw code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// `0`: Photoshop's default.
    Default,
    /// `14`: English (UK), measured in `out/verify/lang_res.psd`.
    EnglishUk,
    /// `42`: German, measured in the round-2 probe.
    German,
}

impl Language {
    pub fn code(self) -> i64 {
        match self {
            Self::Default => 0,
            Self::EnglishUk => 14,
            Self::German => 42,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Default,
            14 => Self::EnglishUk,
            42 => Self::German,
            _ => return None,
        })
    }
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

fn scalar_at(block: &[u8], path: &[Step<'_>]) -> Option<Value> {
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

/// Writes a run's fill colour as Photoshop's `SimplePaint` dictionary:
/// `/53 << /99 /SimplePaint /0 << /0 1 /1 [ a r g b ] >> >>` (alpha first).
pub fn set_run_fill_color(block: &[u8], run: usize, rgba: [f32; 4]) -> Result<Vec<u8>, Error> {
    set_run_paint(block, run, RunProperty::FillPaint, rgba)
}

/// Reads a run's fill colour, if it carries one.
pub fn run_fill_color(block: &[u8], run: usize) -> Option<[f32; 4]> {
    run_paint(block, run, RunProperty::FillPaint)
}

/// Writes a run's stroke colour in the same `SimplePaint` shape at `/54`
/// (`out/probe2/base.psd`: `/54 << /99 /SimplePaint /0 << /0 1 /1 [ 1.0 0.0
/// 0.0 0.0 ] >> >>`).
pub fn set_run_stroke_color(block: &[u8], run: usize, rgba: [f32; 4]) -> Result<Vec<u8>, Error> {
    set_run_paint(block, run, RunProperty::StrokePaint, rgba)
}

/// Reads a run's stroke colour, if it carries one.
pub fn run_stroke_color(block: &[u8], run: usize) -> Option<[f32; 4]> {
    run_paint(block, run, RunProperty::StrokePaint)
}

fn paint_path(run: usize, property: RunProperty) -> Vec<Step<'static>> {
    run_path(
        run,
        &[
            Step::Key("0"),
            Step::Key("0"),
            Step::Key("6"),
            Step::Key(property.key()),
        ],
    )
}

fn set_run_paint(
    block: &[u8],
    run: usize,
    property: RunProperty,
    rgba: [f32; 4],
) -> Result<Vec<u8>, Error> {
    let value = format!(
        "<< /99 /SimplePaint /0 << /0 1 /1 [ {} {} {} {} ] >> >>",
        format_real(f64::from(rgba[0])),
        format_real(f64::from(rgba[1])),
        format_real(f64::from(rgba[2])),
        format_real(f64::from(rgba[3])),
    );
    ensure_container(block, &paint_path(run, property), value.as_bytes())
}

fn run_paint(block: &[u8], run: usize, property: RunProperty) -> Option<[f32; 4]> {
    let (s, e) = value_span(&tokens(block).ok()?, &paint_path(run, property))?;
    let text = std::str::from_utf8(&block[s..e]).ok()?;
    let at = text.find("/1 [")? + 4;
    let body = text[at..].split(']').next()?;
    let nums: Vec<f32> = body
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
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
    let open =
        resolve(&toks, &run_array()).ok_or_else(|| Error::Txt2("run table not found".into()))?;
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
    let (start, end) =
        value_span(&toks, path).ok_or_else(|| Error::Txt2(format!("path not found: {path:?}")))?;
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
    if matches!(toks[arr].kind, Kind::ArrayOpen) {
        Some(arr)
    } else {
        None
    }
}

/// Entries of the `/99 /F` dict (the area-text frame), if the block has one.
fn frame_entries(toks: &[Token]) -> Option<Vec<(String, usize)>> {
    for (i, t) in toks.iter().enumerate() {
        if !matches!(t.kind, Kind::DictOpen) {
            continue;
        }
        let Some(entries) = dict_entries(toks, i) else {
            continue;
        };
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
    Some(FrameGeometry {
        left: v[0],
        top: v[1],
        right: v[2],
        bottom: v[3],
    })
}

/// Writes the frame rectangle in place, keeping the stored order and every
/// other byte of the block.
pub fn set_frame_geometry(block: &[u8], g: FrameGeometry) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let arr = frame_array(&toks).ok_or_else(|| Error::Txt2("frame geometry not found".into()))?;
    let pos =
        frame_values(&toks, arr).ok_or_else(|| Error::Txt2("frame array malformed".into()))?;
    let vals = [g.left, g.top, g.right, g.bottom];
    let mut out = block.to_vec();
    // Reverse order keeps the original byte offsets of earlier elements valid.
    for (idx, &p) in pos.iter().enumerate().rev() {
        let t = &toks[p];
        out.splice(
            t.start..t.end,
            format_real(f64::from(vals[idx])).into_bytes(),
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Composed-layout cache invalidation
// ---------------------------------------------------------------------------

/// One byte span of a block and the bytes that replace it.
struct CacheEdit {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
}

/// Whether the dict opened at `open` is an area-text frame (`/99 /F`) carrying
/// a `/1` rectangle.
fn is_frame_dict(toks: &[Token], open: usize) -> bool {
    let Some(entries) = dict_entries(toks, open) else {
        return false;
    };
    let name = entries
        .iter()
        .find(|(k, _)| k == "99")
        .is_some_and(|(_, v)| matches!(&toks[*v].kind, Kind::Name(n) if n == "F"));
    name && entries
        .iter()
        .any(|(k, v)| k == "1" && matches!(toks[*v].kind, Kind::ArrayOpen))
}

/// Token index of the `/1` rectangle of the area frame inside the subtree
/// opened at `open` (the object's composed layout), if it holds one. Point-text
/// caches have no frame rectangle.
fn frame_in_subtree(toks: &[Token], open: usize) -> Option<usize> {
    let end = skip_value(toks, open)?;
    let dict = (open + 1..end)
        .find(|&i| matches!(toks[i].kind, Kind::DictOpen) && is_frame_dict(toks, i))?;
    let entries = dict_entries(toks, dict)?;
    let arr = entries.iter().find(|(k, _)| k == "1")?.1;
    matches!(toks[arr].kind, Kind::ArrayOpen).then_some(arr)
}

/// The path from a block's top level to text object `object`'s composed
/// layout: `/1` (text engine) → `/1` (objects) → `[object]` → `/1` (cache).
fn cache_path(object: usize) -> [Step<'static>; 4] {
    [
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
    ]
}

/// Number of text objects in `/1 /1` of a block.
fn object_count(toks: &[Token]) -> Option<usize> {
    let arr = resolve(toks, &[Step::Key("1"), Step::Key("1")])?;
    if !matches!(toks[arr].kind, Kind::ArrayOpen) {
        return None;
    }
    let end = skip_value(toks, arr)?;
    let mut count = 0;
    let mut i = arr + 1;
    while i < end && !matches!(toks[i].kind, Kind::ArrayClose) {
        i = skip_value(toks, i)?;
        count += 1;
    }
    Some(count)
}

/// The edit that makes Photoshop recompose object `object`'s layout from the
/// editing model, or `None` when the object has no composed layout to touch.
///
/// * Area text: the cached frame rectangle's top edge moves one point. The
///   model's frame (the `TySh` shape) wins on the next open and Photoshop
///   rewrites the cache with the new line boxes.
/// * Point text: the composed layout (`/2`) becomes `[ ]`, the "not laid out"
///   state Photoshop recomposes from.
fn cache_edit(toks: &[Token], object: usize) -> Option<CacheEdit> {
    let cache = resolve(toks, &cache_path(object))?;
    if !matches!(toks[cache].kind, Kind::DictOpen) {
        return None;
    }
    if let Some(arr) = frame_in_subtree(toks, cache) {
        let pos = frame_values(toks, arr)?;
        let top = number(&toks[pos[1]].kind)?;
        return Some(CacheEdit {
            start: toks[pos[1]].start,
            end: toks[pos[1]].end,
            bytes: format_real(top + 1.0).into_bytes(),
        });
    }
    let path = [
        Step::Key("1"),
        Step::Key("1"),
        Step::Index(object),
        Step::Key("1"),
        Step::Key("2"),
    ];
    let (start, end) = value_span(toks, &path)?;
    Some(CacheEdit {
        start,
        end,
        bytes: b"[ ]".to_vec(),
    })
}

/// Edits of [`invalidate_composed_layouts`], applied back to front so the byte
/// offsets of the spans still to replace stay valid.
fn apply_edits(block: &[u8], mut edits: Vec<CacheEdit>) -> Vec<u8> {
    if edits.is_empty() {
        return block.to_vec();
    }
    edits.sort_by_key(|e| e.start);
    let mut out = block.to_vec();
    for e in edits.into_iter().rev() {
        out.splice(e.start..e.end, e.bytes);
    }
    out
}

/// Makes Photoshop recompose object `object`'s text layout from its editing
/// model on the next open.
///
/// Photoshop draws the composed layout cached in `Txt2` (`/1/1[i]/1`) rather
/// than the model, so a save that changed the text or its properties without
/// rewriting the cache shows the old layout, and the first text edit asks
/// whether to accept the new one. Touching the cache the way Photoshop's own
/// relayout check notices makes it recompose while opening instead:
///
/// * area text (`/99 /F` rectangle in the cache): move the cached frame's top
///   edge one point — the model's frame wins and the cache is rewritten with the
///   model's line boxes. Measured on Photoshop 20.0.2 (`out/verify/
///   existing_depth_framebump.psd` opened and saved again gave exactly the cache
///   of the same layer recomposed from its model, frame included);
/// * point text: empty the composed layout (`/2`), the "not laid out" state
///   Photoshop recomposes from (`docs/layout-update.md`).
///
/// `Ok(block)` unchanged when `object` has no composed layout (a native block
/// or an object past the array).
pub fn invalidate_composed_layout(block: &[u8], object: usize) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let edits = cache_edit(&toks, object).into_iter().collect();
    Ok(apply_edits(block, edits))
}

/// [`invalidate_composed_layout`] for every text object of a document block,
/// so Photoshop recomposes each layer it draws from a cache that may no longer
/// match the model (the block a save writes after text, style or paragraph
/// edits, including the ones `EngineData` cannot describe).
pub fn invalidate_composed_layouts(block: &[u8]) -> Result<Vec<u8>, Error> {
    let toks = tokens(block)?;
    let count = object_count(&toks).unwrap_or(0);
    let edits = (0..count).filter_map(|i| cache_edit(&toks, i)).collect();
    Ok(apply_edits(block, edits))
}

/// Types of the tokens of the complete value at `path` (container included), for comparing
/// two blocks' models without depending on their whitespace or number formatting.
fn value_kinds(toks: &[Token], path: &[Step<'_>]) -> Option<Vec<Kind>> {
    let v = resolve(toks, path)?;
    let end = skip_value(toks, v)?;
    Some(toks[v..end].iter().map(|t| t.kind.clone()).collect())
}

/// [`invalidate_composed_layouts`], skipped when no pre-existing text object's
/// editing model (the `/0` text, style runs and paragraph runs) differs from
/// `previous`'s object at the same slot.
///
/// A writer cannot tell which layers were edited, but the block it replaces
/// can: if every object both blocks hold has the same model, the file is being
/// saved unchanged and its caches, glyph pen positions (`/21 /1`) and other
/// extras are returned byte for byte.  As soon as one object's model changed,
/// every object's cache is invalidated: an old file may carry a stale cache
/// for a layer this save did not touch (an earlier save rewrote its model and
/// kept the cache), and nothing marks those apart, so refreshing all of them is
/// what Photoshop's "Update All Text Layers" does.  Objects `previous` does not
/// hold (new layers) have no older cache and do not count as a change.
pub fn invalidate_changed_composed_layouts(
    current: &[u8],
    previous: &[u8],
) -> Result<Vec<u8>, Error> {
    let toks = tokens(current)?;
    let count = object_count(&toks).unwrap_or(0);
    let Ok(prev) = tokens(previous) else {
        return invalidate_composed_layouts(current);
    };
    let path = |i: usize| {
        [
            Step::Key("1"),
            Step::Key("1"),
            Step::Index(i),
            Step::Key("0"),
        ]
    };
    let changed = (0..count).any(|i| {
        let p = path(i);
        match (value_kinds(&prev, &p), value_kinds(&toks, &p)) {
            // In both blocks: compare the models.
            (Some(before), Some(now)) => before != now,
            // New in this save: no older cache to invalidate.
            (None, _) => false,
            // The replaced block held an object this one lost.
            (Some(_), None) => true,
        }
    });
    if changed {
        invalidate_composed_layouts(current)
    } else {
        Ok(current.to_vec())
    }
}

// Run-table rebuild
// ---------------------------------------------------------------------------
