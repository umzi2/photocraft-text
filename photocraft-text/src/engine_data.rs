//! Adobe EngineData: the PostScript-flavoured text blob Photoshop stores in
//! `TySh/EngineData`.
//!
//! The format is small (dicts, arrays, numbers, booleans, UTF-16 strings) but
//! every text layer carries it, so it is parsed into [`Value`] and written back
//! byte-identically in the same tab-indented layout Photoshop uses.

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Name(String),
    Array(Vec<Value>),
    Dict(Vec<(String, Value)>),
}

impl Value {
    pub fn dict(entries: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
        Value::Dict(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        match self {
            Value::Dict(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float(f) => Some(*f as f32),
            Value::Int(i) => Some(*i as f32),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::Name(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut Vec<(String, Value)>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

// --------------------------------------------------------------------------- //
// Parsing
// --------------------------------------------------------------------------- //

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<Value> {
        self.skip_ws();
        match self.peek() {
            Some(b'<') => {
                if self.data.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    self.parse_dict()
                } else {
                    Err(Error::EngineData("unexpected '<'"))
                }
            }
            Some(b'[') => {
                self.pos += 1;
                self.parse_array()
            }
            Some(b'/') => {
                self.pos += 1;
                Ok(Value::Name(self.parse_name()))
            }
            Some(b'(') => {
                self.pos += 1;
                self.parse_string()
            }
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'+') | Some(b'-') | Some(b'.') | Some(b'0'..=b'9') => self.parse_number(),
            _ => Err(Error::EngineData("unexpected token")),
        }
    }

    fn parse_dict(&mut self) -> Result<Value> {
        let mut entries = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'>') => {
                    self.pos += 2;
                    break;
                }
                Some(b'/') => {
                    self.pos += 1;
                    let key = self.parse_name();
                    let value = self.parse_value()?;
                    entries.push((key, value));
                }
                Some(_) => return Err(Error::EngineData("expected key in dict")),
                None => return Err(Error::EngineData("unterminated dict")),
            }
        }
        Ok(Value::Dict(entries))
    }

    fn parse_array(&mut self) -> Result<Value> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => items.push(self.parse_value()?),
                None => return Err(Error::EngineData("unterminated array")),
            }
        }
        Ok(Value::Array(items))
    }

    fn parse_name(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b'/' | b'<' | b'>' | b'[' | b']' | b'(' | b')') {
                break;
            }
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.data[start..self.pos]).into_owned()
    }

    fn parse_bool(&mut self) -> Result<Value> {
        let rest = &self.data[self.pos..];
        if rest.starts_with(b"true") {
            self.pos += 4;
            Ok(Value::Bool(true))
        } else if rest.starts_with(b"false") {
            self.pos += 5;
            Ok(Value::Bool(false))
        } else {
            Err(Error::EngineData("bad boolean"))
        }
    }

    fn parse_number(&mut self) -> Result<Value> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.data[start..self.pos]).map_err(|_| Error::EngineData("bad number"))?;
        if text.contains('.') {
            text.parse::<f64>().map(Value::Float)
        } else {
            text.parse::<i64>().map(Value::Int).or_else(|_| text.parse::<f64>().map(Value::Float))
        }
        .map_err(|_| Error::EngineData("bad number"))
    }

    fn parse_string(&mut self) -> Result<Value> {
        let mut bytes: Vec<u8> = Vec::new();
        let mut depth = 1;
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            self.pos += 1;
            match c {
                b'\\' => {
                    let Some(esc) = self.peek() else { break };
                    self.pos += 1;
                    bytes.push(esc);
                }
                b'(' => {
                    depth += 1;
                    bytes.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    bytes.push(c);
                }
                _ => bytes.push(c),
            }
        }
        // Photoshop writes text strings as `þÿ` (UTF-16BE BOM) followed by the
        // UTF-16BE code units.
        let text = if bytes.starts_with(&[0xFE, 0xFF]) {
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        Ok(Value::Str(text))
    }
}

/// Parses a whole EngineData blob (leading junk before `<<` is tolerated).
pub fn parse(data: &[u8]) -> Result<Value> {
    let start = data
        .windows(2)
        .position(|w| w == b"<<")
        .ok_or(Error::EngineData("no dictionary"))?;
    let mut p = Parser { data, pos: start };
    let value = p.parse_value()?;
    Ok(value)
}

// --------------------------------------------------------------------------- //
// Writing
// --------------------------------------------------------------------------- //

/// Serialises to Photoshop's layout: tabs for indentation, one entry per line,
/// floats with at least one decimal digit, strings as UTF-16BE with a BOM.
pub fn write(value: &Value) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"\n\n");
    write_value(&mut out, value, 0);
    out
}

fn indent(out: &mut Vec<u8>, depth: usize) {
    for _ in 0..depth {
        out.push(b'\t');
    }
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
}

fn fmt_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{v:.1}");
    }
    // Shortest round-trip form, as the ASL writer emits it: `.8`, not `0.8`.
    let s = format!("{v}");
    if let Some(rest) = s.strip_prefix("0.") {
        format!(".{rest}")
    } else if let Some(rest) = s.strip_prefix("-0.") {
        format!("-.{rest}")
    } else {
        s
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    out.push(b'(');
    out.extend_from_slice(&[0xFE, 0xFF]);
    for unit in s.encode_utf16() {
        for byte in unit.to_be_bytes() {
            match byte {
                b'(' | b')' | b'\\' => {
                    out.push(b'\\');
                    out.push(byte);
                }
                _ => out.push(byte),
            }
        }
    }
    out.push(b')');
}

fn write_value(out: &mut Vec<u8>, value: &Value, depth: usize) {
    match value {
        Value::Bool(b) => push_str(out, if *b { "true" } else { "false" }),
        Value::Int(i) => push_str(out, &i.to_string()),
        Value::Float(f) => push_str(out, &fmt_number(*f)),
        Value::Name(n) => {
            out.push(b'/');
            push_str(out, n);
        }
        Value::Str(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                push_str(out, "[ ]");
                return;
            }
            let simple = items
                .iter()
                .all(|i| matches!(i, Value::Int(_) | Value::Float(_) | Value::Bool(_)));
            if simple {
                out.push(b'[');
                for item in items {
                    out.push(b' ');
                    write_value(out, item, depth);
                }
                push_str(out, " ]");
                return;
            }
            // Items line up with the key that introduced the array, and the
            // closing bracket sits at the enclosing indentation level.
            push_str(out, "[\n");
            for item in items {
                indent(out, depth);
                write_value(out, item, depth);
                out.push(b'\n');
            }
            indent(out, depth);
            out.push(b']');
        }
        Value::Dict(entries) => {
            if entries.is_empty() {
                push_str(out, "<<\n");
                indent(out, depth);
                push_str(out, ">>");
                return;
            }
            push_str(out, "<<\n");
            for (key, v) in entries {
                indent(out, depth + 1);
                out.push(b'/');
                push_str(out, key);
                if matches!(v, Value::Dict(_)) {
                    out.push(b'\n');
                    indent(out, depth + 1);
                } else {
                    out.push(b' ');
                }
                write_value(out, v, depth + 1);
                out.push(b'\n');
            }
            indent(out, depth);
            push_str(out, ">>");
        }
    }
}
