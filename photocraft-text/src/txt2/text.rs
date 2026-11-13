use crate::error::Error;

use super::cache::clear_cache;
use super::path::{Step, Value, replace_container, replace_value, resolve};
use super::syntax::{BOM, Kind, TERMINATOR, skip_value, tokens, utf16};

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
/// [`set_run_layout`](super::set_run_layout) and [`set_run_property`](super::set_run_property).
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
pub(super) fn table_path(style: bool) -> Vec<Step<'static>> {
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
