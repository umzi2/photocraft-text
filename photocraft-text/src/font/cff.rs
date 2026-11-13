//! CFF (Compact Font Format) outlines: INDEX/DICT parsing and a Type 2
//! charstring interpreter.
//!
//! The interpreter emits [`crate::outline::Path`]: closed contours of lines and
//! cubic Béziers in font units, y up. Type 2 charstrings are drawn in
//! *charstring* coordinates; the font's `FontMatrix` maps them to the em square
//! and `unitsPerEm` then gives font units. For the common `[0.001 0 0 0.001 0 0]`
//! matrix with a 1000-unit em that is the identity.
//!
//! Scope: CFF 1 (the `CFF ` table). CFF2 is not handled. CID-keyed fonts are
//! supported through `FDArray`/`FDSelect` (per-glyph Private DICT and local
//! subrs); `seac`-style `endchar` is reported to the caller, which resolves the
//! two standard-encoding components (see [`crate::outline::glyph_path`]).

use crate::error::{Error, Result};
use crate::outline::{Contour, Path, Pt, Seg};

/// Type 2 limits subroutine nesting to 10 levels.
const MAX_SUBR_DEPTH: usize = 10;
/// Operand stack limit (the spec's 513).
const MAX_STACK: usize = 513;

fn oob(needed: usize, len: usize) -> Error {
    Error::OutOfBounds { needed, len }
}

fn bad(msg: &'static str) -> Error {
    Error::BadFont(msg)
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    data.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be_i16(data: &[u8], at: usize) -> Option<i16> {
    be16(data, at).map(|v| v as i16)
}

fn be_i32(data: &[u8], at: usize) -> Option<i32> {
    data.get(at..at + 4)
        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Big-endian unsigned offset of `1..=4` bytes.
fn read_offset(data: &[u8], at: usize, size: usize) -> Option<u32> {
    let bytes = data.get(at..at + size)?;
    let mut v = 0u32;
    for b in bytes {
        v = (v << 8) | *b as u32;
    }
    Some(v)
}

/// A CFF INDEX: `count` byte strings addressed through a shared offset array.
#[derive(Clone, Debug)]
struct Index<'a> {
    data: &'a [u8],
    /// `count + 1` offsets, 1-based from `base`.
    offsets: Vec<u32>,
    /// Position such that item byte `k` lives at `base + k`.
    base: usize,
    /// First byte after the INDEX (the next structure starts here).
    next: usize,
}

impl<'a> Index<'a> {
    fn parse(data: &'a [u8], at: usize) -> Result<Self> {
        let count = be16(data, at).ok_or(oob(at + 2, data.len()))? as usize;
        if count == 0 {
            let next = at.checked_add(2).ok_or(oob(usize::MAX, data.len()))?;
            return Ok(Self {
                data,
                offsets: Vec::new(),
                base: next,
                next,
            });
        }
        let off_size = *data.get(at + 2).ok_or(oob(at + 3, data.len()))? as usize;
        if !(1..=4).contains(&off_size) {
            return Err(bad("CFF INDEX offSize"));
        }
        let mut offsets = Vec::with_capacity(count + 1);
        let mut pos = at + 3;
        for _ in 0..=count {
            offsets.push(read_offset(data, pos, off_size).ok_or(oob(pos + off_size, data.len()))?);
            pos += off_size;
        }
        // Offsets are 1-based with respect to the byte before the data.
        let base = pos.checked_sub(1).ok_or(bad("CFF INDEX layout"))?;
        let last = *offsets.last().ok_or(bad("CFF INDEX offsets"))? as usize;
        let next = base.checked_add(last).ok_or(bad("CFF INDEX length"))?;
        if next > data.len() {
            return Err(oob(next, data.len()));
        }
        Ok(Self {
            data,
            offsets,
            base,
            next,
        })
    }

    fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    fn get(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.len() {
            return None;
        }
        let start = self.base.checked_add(*self.offsets.get(i)? as usize)?;
        let end = self.base.checked_add(*self.offsets.get(i + 1)? as usize)?;
        self.data.get(start..end)
    }
}

/// `seac` data from a four-operand `endchar`: draw the `bchar` standard-encoding
/// glyph at the origin and `achar` at `(adx, ady)` (already in font units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seac {
    pub adx: f32,
    pub ady: f32,
    /// Base character code (Adobe StandardEncoding).
    pub bchar: u8,
    /// Accent character code (Adobe StandardEncoding).
    pub achar: u8,
}

/// The Private DICT of a font (or of one CID font dict).
#[derive(Clone, Debug, Default)]
struct Private<'a> {
    subrs: Option<Index<'a>>,
    default_width_x: Option<f64>,
    nominal_width_x: Option<f64>,
}

/// CID-keyed `FDArray` + `FDSelect`.
#[derive(Clone, Debug, Default)]
struct Cid<'a> {
    fds: Vec<Private<'a>>,
    select: Vec<u16>,
}

/// Parsed `CFF ` table.
pub struct Cff<'a> {
    upm: f64,
    font_matrix: [f64; 6],
    charstrings: Index<'a>,
    global_subrs: Index<'a>,
    private: Option<Private<'a>>,
    cid: Option<Cid<'a>>,
    default_width_x: Option<f64>,
    nominal_width_x: Option<f64>,
}

impl<'a> Cff<'a> {
    /// Parses the `CFF ` table. `upm` is the `head.unitsPerEm` value used to
    /// scale charstring coordinates into font units.
    pub fn parse(data: &'a [u8], upm: u16) -> Result<Self> {
        if data.len() < 4 {
            return Err(oob(4, data.len()));
        }
        let hdr_size = *data.get(2).ok_or(oob(3, data.len()))? as usize;
        let names = Index::parse(data, hdr_size)?;
        let top_dicts = Index::parse(data, names.next)?;
        let strings = Index::parse(data, top_dicts.next)?;
        let global_subrs = Index::parse(data, strings.next)?;
        let top_data = top_dicts.get(0).ok_or(bad("CFF Top DICT missing"))?;

        let mut charstrings_off = None;
        let mut private_off = None;
        let mut font_matrix = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];
        let mut default_width_x = None;
        let mut nominal_width_x = None;
        let mut fd_array_off = None;
        let mut fd_select_off = None;
        let mut is_cid = false;
        for (op, operands) in parse_dict(top_data)? {
            match op {
                17 => charstrings_off = operands.first().copied(),
                18 => {
                    if operands.len() >= 2 {
                        private_off = Some((operands[0], operands[1]));
                    }
                }
                20 => default_width_x = operands.first().copied(),
                21 => nominal_width_x = operands.first().copied(),
                0x0c07 => {
                    if operands.len() >= 6 {
                        for (slot, value) in font_matrix.iter_mut().zip(&operands) {
                            *slot = *value;
                        }
                    }
                }
                // 12 30 ROS
                0x0c1e => is_cid = true,
                // 12 36 FDArray, 12 37 FDSelect
                0x0c24 => fd_array_off = operands.first().copied(),
                0x0c25 => fd_select_off = operands.first().copied(),
                _ => {}
            }
        }

        let charstrings_off = charstrings_off.ok_or(bad("CFF CharStrings missing"))? as usize;
        let charstrings = Index::parse(data, charstrings_off)?;

        let private = match private_off {
            Some((size, off)) => Some(parse_private(data, off as usize, size as usize)?),
            None => None,
        };

        let mut cid = None;
        if is_cid && let (Some(fd_array_off), Some(fd_select_off)) = (fd_array_off, fd_select_off) {
            let fd_index = Index::parse(data, fd_array_off as usize)?;
            let mut fds = Vec::with_capacity(fd_index.len());
            for i in 0..fd_index.len() {
                let dict = fd_index.get(i).ok_or(bad("CFF FDArray entry"))?;
                fds.push(parse_font_dict(data, dict)?);
            }
            let select = parse_fd_select(data, fd_select_off as usize, charstrings.len())?;
            cid = Some(Cid { fds, select });
        }

        Ok(Self {
            upm: upm as f64,
            font_matrix,
            charstrings,
            global_subrs,
            private,
            cid,
            default_width_x,
            nominal_width_x,
        })
    }

    /// Number of glyphs in the `CharStrings` INDEX.
    pub fn num_glyphs(&self) -> usize {
        self.charstrings.len()
    }

    /// Outline of `gid` in font units, plus `seac` components when the glyph is
    /// an accented composite built by `endchar`.
    pub fn outline(&self, gid: u16) -> Result<(Path, Option<Seac>)> {
        let code = self
            .charstrings
            .get(gid as usize)
            .ok_or(bad("CFF glyph index out of range"))?;
        let private = match &self.cid {
            Some(cid) => {
                let fd = cid.select.get(gid as usize).copied().unwrap_or(0) as usize;
                cid.fds.get(fd)
            }
            None => self.private.as_ref(),
        };
        let local_subrs = private.and_then(|p| p.subrs.as_ref());
        let mut interp = Interp {
            upm: self.upm,
            matrix: self.font_matrix,
            global_subrs: &self.global_subrs,
            local_subrs,
            stack: Vec::new(),
            path: Vec::new(),
            start: None,
            segs: Vec::new(),
            cur: (0.0, 0.0),
            hint_count: 0,
            mask_bytes: 0,
            got_width: false,
            done: false,
            seac: None,
            transient: Vec::new(),
            rng: 0x2545_f491_4f6c_dd1d,
        };
        interp.exec(code, 0)?;
        interp.finish_contour();
        Ok((interp.path, interp.seac))
    }

    /// `defaultWidthX` from the Private/Top DICT, if present.
    pub fn default_width_x(&self) -> Option<f64> {
        self.private
            .as_ref()
            .and_then(|p| p.default_width_x)
            .or(self.default_width_x)
    }

    /// `nominalWidthX` from the Private/Top DICT, if present.
    pub fn nominal_width_x(&self) -> Option<f64> {
        self.private
            .as_ref()
            .and_then(|p| p.nominal_width_x)
            .or(self.nominal_width_x)
    }
}

fn parse_private<'a>(data: &'a [u8], off: usize, size: usize) -> Result<Private<'a>> {
    let end = off
        .checked_add(size)
        .ok_or(bad("CFF Private DICT offset"))?;
    let dict = data.get(off..end).ok_or(oob(end, data.len()))?;
    let mut private = Private::default();
    let mut subrs_off = None;
    for (op, operands) in parse_dict(dict)? {
        match op {
            19 => subrs_off = operands.first().copied(),
            20 => private.default_width_x = operands.first().copied(),
            21 => private.nominal_width_x = operands.first().copied(),
            _ => {}
        }
    }
    if let Some(subrs_off) = subrs_off {
        let at = off
            .checked_add(subrs_off as usize)
            .ok_or(bad("CFF Subrs offset"))?;
        private.subrs = Some(Index::parse(data, at)?);
    }
    Ok(private)
}

fn parse_font_dict<'a>(data: &'a [u8], dict: &[u8]) -> Result<Private<'a>> {
    let mut private_off = None;
    for (op, operands) in parse_dict(dict)? {
        if op == 18 && operands.len() >= 2 {
            private_off = Some((operands[0], operands[1]));
        }
    }
    match private_off {
        Some((size, off)) => parse_private(data, off as usize, size as usize),
        None => Ok(Private::default()),
    }
}

fn parse_fd_select(data: &[u8], at: usize, num_glyphs: usize) -> Result<Vec<u16>> {
    let format = *data.get(at).ok_or(oob(at + 1, data.len()))?;
    match format {
        0 => {
            let bytes = data
                .get(at + 1..at + 1 + num_glyphs)
                .ok_or(oob(at + 1 + num_glyphs, data.len()))?;
            Ok(bytes.iter().map(|b| *b as u16).collect())
        }
        3 => {
            // `nRanges`, then `nRanges` (first gid, fd) records, then a sentinel
            // gid that ends the last range.
            let n_ranges = be16(data, at + 1).ok_or(oob(at + 3, data.len()))? as usize;
            let mut select = vec![0u16; num_glyphs];
            let mut pos = at + 3;
            for _ in 0..n_ranges {
                let first = be16(data, pos).ok_or(oob(pos + 2, data.len()))? as usize;
                let fd = *data.get(pos + 2).ok_or(oob(pos + 3, data.len()))? as u16;
                // The next record's first gid (or the sentinel) ends this range.
                let end = be16(data, pos + 3).ok_or(oob(pos + 5, data.len()))? as usize;
                pos += 3;
                for slot in select.iter_mut().take(end.min(num_glyphs)).skip(first) {
                    *slot = fd;
                }
            }
            Ok(select)
        }
        _ => Err(bad("CFF FDSelect format")),
    }
}

/// Parses a CFF DICT into `(operator, operands)` pairs. Single-byte operators
/// keep their byte value; escaped operators are `0x0c00 | byte`.
fn parse_dict(data: &[u8]) -> Result<Vec<(u16, Vec<f64>)>> {
    let mut out: Vec<(u16, Vec<f64>)> = Vec::new();
    let mut operands: Vec<f64> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i];
        match b {
            32..=246 => {
                operands.push(b as f64 - 139.0);
                i += 1;
            }
            247..=250 => {
                let b1 = *data.get(i + 1).ok_or(oob(i + 2, data.len()))?;
                operands.push((b as f64 - 247.0) * 256.0 + b1 as f64 + 108.0);
                i += 2;
            }
            251..=254 => {
                let b1 = *data.get(i + 1).ok_or(oob(i + 2, data.len()))?;
                operands.push(-((b as f64 - 251.0) * 256.0 + b1 as f64 + 108.0));
                i += 2;
            }
            28 => {
                let v = be_i16(data, i + 1).ok_or(oob(i + 3, data.len()))?;
                operands.push(v as f64);
                i += 3;
            }
            29 => {
                let v = be_i32(data, i + 1).ok_or(oob(i + 5, data.len()))?;
                operands.push(v as f64);
                i += 5;
            }
            30 => {
                i += 1;
                operands.push(parse_real(data, &mut i)?);
            }
            255 => {
                // Reserved in DICT data; read as a 16.16 fixed number so a
                // non-conforming producer cannot desynchronise the stream.
                let v = be_i32(data, i + 1).ok_or(oob(i + 5, data.len()))?;
                operands.push(v as f64 / 65536.0);
                i += 5;
            }
            _ => {
                let op = if b == 12 {
                    let esc = *data.get(i + 1).ok_or(oob(i + 2, data.len()))?;
                    i += 2;
                    0x0c00 | esc as u16
                } else {
                    i += 1;
                    b as u16
                };
                out.push((op, std::mem::take(&mut operands)));
            }
        }
    }
    Ok(out)
}

/// Parses a DICT real number (`nibble` encoding, terminator `0xf`).
fn parse_real(data: &[u8], i: &mut usize) -> Result<f64> {
    let mut text = String::new();
    loop {
        let b = *data.get(*i).ok_or(oob(*i + 1, data.len()))?;
        *i += 1;
        let mut done = false;
        for nibble in [b >> 4, b & 0x0f] {
            match nibble {
                0..=9 => text.push((b'0' + nibble) as char),
                0x0a => text.push('.'),
                0x0b => text.push('E'),
                0x0c => text.push_str("E-"),
                0x0e => text.push('-'),
                0x0f => {
                    done = true;
                    break;
                }
                _ => {}
            }
        }
        if done {
            break;
        }
    }
    text.parse::<f64>().map_err(|_| bad("CFF DICT real number"))
}

/// Subroutine index bias for an INDEX with `count` entries.
fn subr_bias(count: usize) -> i32 {
    if count < 1240 {
        107
    } else if count < 33900 {
        1131
    } else {
        32768
    }
}

/// Type 2 charstring interpreter producing an [`crate::outline::Path`].
struct Interp<'a> {
    upm: f64,
    matrix: [f64; 6],
    global_subrs: &'a Index<'a>,
    local_subrs: Option<&'a Index<'a>>,
    stack: Vec<f64>,
    path: Path,
    start: Option<Pt>,
    segs: Vec<Seg>,
    /// Current point in charstring space.
    cur: (f64, f64),
    hint_count: usize,
    mask_bytes: usize,
    got_width: bool,
    done: bool,
    seac: Option<Seac>,
    /// Type 2 transient array (`put`/`get`).
    transient: Vec<f64>,
    /// Deterministic state for the `random` operator.
    rng: u64,
}

/// Pops the whole stack without consuming a width operand (escape operators are
/// never the first stack-clearing operator of a charstring).
impl Interp<'_> {
    fn pop_all(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.stack)
    }
}

impl<'a> Interp<'a> {
    fn push(&mut self, value: f64) -> Result<()> {
        if self.stack.len() >= MAX_STACK {
            return Err(bad("CFF operand stack overflow"));
        }
        self.stack.push(value);
        Ok(())
    }

    fn pop(&mut self) -> Result<f64> {
        self.stack.pop().ok_or(bad("CFF operand stack underflow"))
    }

    /// Pops the stack for a stack-clearing operator, consuming a leading width
    /// operand the first time it appears. `even_odd` mirrors the Type 2 rule:
    /// strip when `even_odd ^ (len % 2)` is true.
    fn take_args(&mut self, even_odd: bool) -> Vec<f64> {
        let mut args = std::mem::take(&mut self.stack);
        if !self.got_width {
            if even_odd ^ (args.len() % 2 == 1) && !args.is_empty() {
                args.remove(0);
            }
            self.got_width = true;
        }
        args
    }

    fn map(&self, x: f64, y: f64) -> Pt {
        let fx = self.upm * (self.matrix[0] * x + self.matrix[2] * y + self.matrix[4]);
        let fy = self.upm * (self.matrix[1] * x + self.matrix[3] * y + self.matrix[5]);
        Pt {
            x: fx as f32,
            y: fy as f32,
        }
    }

    fn map_vector(&self, dx: f64, dy: f64) -> (f32, f32) {
        let x = self.upm * (self.matrix[0] * dx + self.matrix[2] * dy);
        let y = self.upm * (self.matrix[1] * dx + self.matrix[3] * dy);
        (x as f32, y as f32)
    }

    fn finish_contour(&mut self) {
        let Some(start) = self.start.take() else {
            return;
        };
        if !self.segs.is_empty() {
            self.path.push(Contour {
                start,
                segs: std::mem::take(&mut self.segs),
            });
        }
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.finish_contour();
        self.start = Some(self.map(x, y));
        self.cur = (x, y);
    }

    /// Starts a contour at the current point if a drawing operator arrives with
    /// no preceding moveto (Type 2 treats the origin as the initial point).
    fn ensure_start(&mut self) {
        if self.start.is_none() {
            let cur = self.cur;
            self.move_to(cur.0, cur.1);
        }
    }

    fn line_to(&mut self, x: f64, y: f64) {
        self.ensure_start();
        self.segs.push(Seg::Line { to: self.map(x, y) });
        self.cur = (x, y);
    }

    fn curve_to(&mut self, c1: (f64, f64), c2: (f64, f64), to: (f64, f64)) {
        self.ensure_start();
        self.segs.push(Seg::Cubic {
            c1: self.map(c1.0, c1.1),
            c2: self.map(c2.0, c2.1),
            to: self.map(to.0, to.1),
        });
        self.cur = to;
    }

    fn exec(&mut self, code: &'a [u8], depth: usize) -> Result<()> {
        if depth > MAX_SUBR_DEPTH {
            return Err(bad("CFF subroutine nesting"));
        }
        let len = code.len();
        let mut i = 0usize;
        while i < len {
            if self.done {
                return Ok(());
            }
            let b = code[i];
            i += 1;
            match b {
                32..=246 => self.push(b as f64 - 139.0)?,
                247..=250 => {
                    let b1 = *code.get(i).ok_or(oob(i + 1, len))?;
                    i += 1;
                    self.push((b as f64 - 247.0) * 256.0 + b1 as f64 + 108.0)?;
                }
                251..=254 => {
                    let b1 = *code.get(i).ok_or(oob(i + 1, len))?;
                    i += 1;
                    self.push(-((b as f64 - 251.0) * 256.0 + b1 as f64 + 108.0))?;
                }
                28 => {
                    let v = be_i16(code, i).ok_or(oob(i + 2, len))?;
                    i += 2;
                    self.push(v as f64)?;
                }
                255 => {
                    let v = be_i32(code, i).ok_or(oob(i + 4, len))?;
                    i += 4;
                    self.push(v as f64 / 65536.0)?;
                }
                10 | 29 => {
                    let raw = self.pop()?;
                    let (index, bias) = if b == 10 {
                        (
                            self.local_subrs,
                            self.local_subrs.map(|s| subr_bias(s.len())).unwrap_or(0),
                        )
                    } else {
                        (Some(self.global_subrs), subr_bias(self.global_subrs.len()))
                    };
                    let subr_index = raw as i64 + bias as i64;
                    if subr_index < 0 {
                        return Err(bad("CFF subroutine index"));
                    }
                    let subr = index
                        .and_then(|s| s.get(subr_index as usize))
                        .ok_or(bad("CFF subroutine index out of range"))?;
                    self.exec(subr, depth + 1)?;
                }
                11 => return Ok(()),
                12 => {
                    let esc = *code.get(i).ok_or(oob(i + 1, len))?;
                    i += 1;
                    self.escape(esc)?;
                }
                14 => {
                    self.endchar();
                    self.done = true;
                    return Ok(());
                }
                _ => {
                    let skip = self.op(b)?;
                    i += skip;
                }
            }
        }
        Ok(())
    }

    fn endchar(&mut self) {
        let args = self.take_args(false);
        self.finish_contour();
        if args.len() >= 4 {
            let (adx, ady) = self.map_vector(args[0], args[1]);
            self.seac = Some(Seac {
                adx,
                ady,
                bchar: args[2] as i64 as u8,
                achar: args[3] as i64 as u8,
            });
        }
    }

    /// Handles a one-byte drawing/hint operator; returns the number of extra
    /// bytes it consumes (the mask after `hintmask`/`cntrmask`).
    fn op(&mut self, b: u8) -> Result<usize> {
        match b {
            1 | 3 | 18 | 23 => {
                let args = self.take_args(false);
                self.hint_count += args.len() / 2;
                Ok(0)
            }
            19 | 20 => {
                let args = self.take_args(false);
                self.hint_count += args.len() / 2;
                if self.mask_bytes == 0 {
                    self.mask_bytes = self.hint_count.div_ceil(8);
                }
                Ok(self.mask_bytes)
            }
            4 => {
                let args = self.take_args(true);
                let dy = *args.first().ok_or(bad("CFF vmoveto"))?;
                let cur = self.cur;
                self.move_to(cur.0, cur.1 + dy);
                Ok(0)
            }
            21 => {
                let args = self.take_args(false);
                if args.len() < 2 {
                    return Err(bad("CFF rmoveto"));
                }
                let cur = self.cur;
                self.move_to(cur.0 + args[0], cur.1 + args[1]);
                Ok(0)
            }
            22 => {
                let args = self.take_args(true);
                let dx = *args.first().ok_or(bad("CFF hmoveto"))?;
                let cur = self.cur;
                self.move_to(cur.0 + dx, cur.1);
                Ok(0)
            }
            5 => {
                let args = self.take_args(false);
                if !args.len().is_multiple_of(2) {
                    return Err(bad("CFF rlineto arguments"));
                }
                for pair in args.chunks(2) {
                    let cur = self.cur;
                    self.line_to(cur.0 + pair[0], cur.1 + pair[1]);
                }
                Ok(0)
            }
            6 | 7 => {
                let args = self.take_args(false);
                let mut horizontal = b == 6;
                for value in args {
                    let cur = self.cur;
                    if horizontal {
                        self.line_to(cur.0 + value, cur.1);
                    } else {
                        self.line_to(cur.0, cur.1 + value);
                    }
                    horizontal = !horizontal;
                }
                Ok(0)
            }
            8 => {
                let args = self.take_args(false);
                for curve in args.chunks(6) {
                    if curve.len() != 6 {
                        return Err(bad("CFF rrcurveto arguments"));
                    }
                    self.relative_curve(curve[0], curve[1], curve[2], curve[3], curve[4], curve[5]);
                }
                Ok(0)
            }
            24 => {
                // rcurveline: groups of six, then one final line pair.
                let args = self.take_args(false);
                if args.len() < 8 || !(args.len() - 2).is_multiple_of(6) {
                    return Err(bad("CFF rcurveline arguments"));
                }
                let split = args.len() - 2;
                for curve in args[..split].chunks(6) {
                    self.relative_curve(curve[0], curve[1], curve[2], curve[3], curve[4], curve[5]);
                }
                let cur = self.cur;
                self.line_to(cur.0 + args[split], cur.1 + args[split + 1]);
                Ok(0)
            }
            25 => {
                // rlinecurve: line pairs, then one final curve.
                let args = self.take_args(false);
                if args.len() < 8 || !(args.len() - 6).is_multiple_of(2) {
                    return Err(bad("CFF rlinecurve arguments"));
                }
                let split = args.len() - 6;
                for pair in args[..split].chunks(2) {
                    let cur = self.cur;
                    self.line_to(cur.0 + pair[0], cur.1 + pair[1]);
                }
                self.relative_curve(
                    args[split],
                    args[split + 1],
                    args[split + 2],
                    args[split + 3],
                    args[split + 4],
                    args[split + 5],
                );
                Ok(0)
            }
            26 | 27 => {
                // vvcurveto / hhcurveto: an optional leading cross-axis delta
                // when the argument count is odd.
                let mut args = self.take_args(false);
                let mut first = 0.0;
                if args.len() % 2 == 1 {
                    first = args.remove(0);
                }
                for chunk in args.chunks(4) {
                    if chunk.len() != 4 {
                        return Err(bad("CFF vv/hhcurveto arguments"));
                    }
                    if b == 26 {
                        self.relative_curve(first, chunk[0], chunk[1], chunk[2], 0.0, chunk[3]);
                    } else {
                        self.relative_curve(chunk[0], first, chunk[1], chunk[2], chunk[3], 0.0);
                    }
                    first = 0.0;
                }
                Ok(0)
            }
            30 | 31 => {
                let mut args = self.take_args(false);
                while !args.is_empty() {
                    if b == 31 {
                        self.h_curve(&mut args)?;
                    } else {
                        self.v_curve(&mut args)?;
                    }
                    if args.is_empty() {
                        break;
                    }
                    if b == 31 {
                        self.v_curve(&mut args)?;
                    } else {
                        self.h_curve(&mut args)?;
                    }
                }
                Ok(0)
            }
            _ => {
                // Unknown one-byte operator: drop its operands and continue.
                self.stack.clear();
                Ok(0)
            }
        }
    }

    fn relative_curve(&mut self, dx1: f64, dy1: f64, dx2: f64, dy2: f64, dx3: f64, dy3: f64) {
        let cur = self.cur;
        let c1 = (cur.0 + dx1, cur.1 + dy1);
        let c2 = (c1.0 + dx2, c1.1 + dy2);
        let to = (c2.0 + dx3, c2.1 + dy3);
        self.curve_to(c1, c2, to);
    }

    /// One horizontal-first curve of `hvcurveto`: `dxa dxb dyb dyc [dxc]`.
    fn h_curve(&mut self, args: &mut Vec<f64>) -> Result<()> {
        if args.len() < 4 {
            return Err(bad("CFF hvcurveto arguments"));
        }
        let (dxa, dxb, dyb, dyc) = (args[0], args[1], args[2], args[3]);
        args.drain(0..4);
        let dxc = if args.len() == 1 {
            let last = args[0];
            args.clear();
            last
        } else {
            0.0
        };
        let cur = self.cur;
        self.curve_to(
            (cur.0 + dxa, cur.1),
            (cur.0 + dxa + dxb, cur.1 + dyb),
            (cur.0 + dxa + dxb + dxc, cur.1 + dyb + dyc),
        );
        Ok(())
    }

    /// One vertical-first curve of `vhcurveto`: `dya dxb dyb dxc [dyc]`.
    fn v_curve(&mut self, args: &mut Vec<f64>) -> Result<()> {
        if args.len() < 4 {
            return Err(bad("CFF vhcurveto arguments"));
        }
        let (dya, dxb, dyb, dxc) = (args[0], args[1], args[2], args[3]);
        args.drain(0..4);
        let dyc = if args.len() == 1 {
            let last = args[0];
            args.clear();
            last
        } else {
            0.0
        };
        let cur = self.cur;
        self.curve_to(
            (cur.0, cur.1 + dya),
            (cur.0 + dxb, cur.1 + dya + dyb),
            (cur.0 + dxb + dxc, cur.1 + dya + dyb + dyc),
        );
        Ok(())
    }

    fn escape(&mut self, esc: u8) -> Result<()> {
        match esc {
            // flex
            35 => {
                let a = self.pop_all();
                if a.len() < 13 {
                    return Err(bad("CFF flex arguments"));
                }
                self.relative_curve(a[0], a[1], a[2], a[3], a[4], a[5]);
                self.relative_curve(a[6], a[7], a[8], a[9], a[10], a[11]);
                Ok(())
            }
            // hflex: dx1 dx2 dy2 dx3 dx4 dx5 dx6
            34 => {
                let a = self.pop_all();
                if a.len() < 7 {
                    return Err(bad("CFF hflex arguments"));
                }
                let (dx1, dx2, dy2, dx3, dx4, dx5, dx6) =
                    (a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
                self.relative_curve(dx1, 0.0, dx2, dy2, dx3, 0.0);
                self.relative_curve(dx4, 0.0, dx5, -dy2, dx6, 0.0);
                Ok(())
            }
            // hflex1: dx1 dy1 dx2 dy2 dx3 dx4 dx5 dy5 dx6
            36 => {
                let a = self.pop_all();
                if a.len() < 9 {
                    return Err(bad("CFF hflex1 arguments"));
                }
                let (dx1, dy1, dx2, dy2, dx3) = (a[0], a[1], a[2], a[3], a[4]);
                let (dx4, dx5, dy5, dx6) = (a[5], a[6], a[7], a[8]);
                let dy6 = -(dy1 + dy2 + dy5);
                self.relative_curve(dx1, dy1, dx2, dy2, dx3, 0.0);
                self.relative_curve(dx4, 0.0, dx5, dy5, dx6, dy6);
                Ok(())
            }
            // flex1: dx1 dy1 dx2 dy2 dx3 dy3 dx4 dy4 dx5 dy5 d6
            37 => {
                let a = self.pop_all();
                if a.len() < 11 {
                    return Err(bad("CFF flex1 arguments"));
                }
                let dx = a[0] + a[2] + a[4] + a[6] + a[8];
                let dy = a[1] + a[3] + a[5] + a[7] + a[9];
                let (dx6, dy6) = if dx.abs() > dy.abs() {
                    (a[10], -dy)
                } else {
                    (-dx, a[10])
                };
                self.relative_curve(a[0], a[1], a[2], a[3], a[4], a[5]);
                self.relative_curve(a[6], a[7], a[8], a[9], dx6, dy6);
                Ok(())
            }
            3 => {
                // and
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(if a != 0.0 && b != 0.0 { 1.0 } else { 0.0 })
            }
            4 => {
                // or
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(if a != 0.0 || b != 0.0 { 1.0 } else { 0.0 })
            }
            5 => {
                // not
                let a = self.pop()?;
                self.push(if a == 0.0 { 1.0 } else { 0.0 })
            }
            9 => {
                let a = self.pop()?;
                self.push(a.abs())
            }
            10 => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(a + b)
            }
            11 => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(a - b)
            }
            12 => {
                let b = self.pop()?;
                let a = self.pop()?;
                if b == 0.0 {
                    return Err(bad("CFF division by zero"));
                }
                self.push(a / b)
            }
            14 => {
                let a = self.pop()?;
                self.push(-a)
            }
            15 => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(if a == b { 1.0 } else { 0.0 })
            }
            18 => {
                let _ = self.pop()?;
                Ok(())
            }
            20 => {
                // put: `val index put`
                let index = self.pop()?;
                let value = self.pop()?;
                let slot = index as usize;
                if slot >= 32 {
                    return Err(bad("CFF transient array index"));
                }
                if self.transient.len() <= slot {
                    self.transient.resize(slot + 1, 0.0);
                }
                self.transient[slot] = value;
                Ok(())
            }
            21 => {
                // get
                let index = self.pop()? as usize;
                let value = self.transient.get(index).copied().unwrap_or(0.0);
                self.push(value)
            }
            22 => {
                // ifelse: `s1 s2 v1 v2 ifelse`
                let v2 = self.pop()?;
                let v1 = self.pop()?;
                let s2 = self.pop()?;
                let s1 = self.pop()?;
                self.push(if s1 <= s2 { v1 } else { v2 })
            }
            23 => {
                // random in (0, 1); the generator is fixed so outlines are
                // reproducible across runs.
                self.rng = self
                    .rng
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let value = ((self.rng >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
                self.push(value)
            }
            24 => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(a * b)
            }
            26 => {
                let a = self.pop()?;
                if a < 0.0 {
                    return Err(bad("CFF sqrt of a negative number"));
                }
                self.push(a.sqrt())
            }
            27 => {
                let a = self.pop()?;
                self.push(a)
            }
            28 => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(b)?;
                self.push(a)
            }
            29 => {
                // index: `n index` copies the n-th element from the top
                let n = self.pop()? as usize;
                let pos = self
                    .stack
                    .len()
                    .checked_sub(n + 1)
                    .ok_or(bad("CFF index operator underflow"))?;
                let value = self.stack[pos];
                self.push(value)
            }
            30 => {
                // roll: `n j roll`
                let j = self.pop()? as i64;
                let n = self.pop()? as i64;
                if n <= 0 || n as usize > self.stack.len() {
                    return Err(bad("CFF roll operator range"));
                }
                let n = n as usize;
                let start = self.stack.len() - n;
                let shift = ((j % n as i64) + n as i64) as usize % n;
                self.stack[start..].rotate_right(shift);
                Ok(())
            }
            _ => {
                // dotsection and every other escaped operator we do not model:
                // discard operands (fontTools clears the stack the same way).
                self.stack.clear();
                Ok(())
            }
        }
    }
}
