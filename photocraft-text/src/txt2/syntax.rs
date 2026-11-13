use crate::error::Error;

pub(super) const BOM: [u8; 2] = [0xfe, 0xff];
pub(super) const TERMINATOR: [u8; 2] = [0x00, 0x0d];

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
pub(super) fn utf16_name(s: &str) -> Vec<u8> {
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
