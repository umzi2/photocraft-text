//! Glyph outlines in font units, y up.
//!
//! Two representations share this module:
//!
//! * [`Contours`] — the TrueType view: closed contours of on/off-curve points
//!   (quadratic segments). All existing raster/optical code consumes this and
//!   it stays byte-for-byte what the `glyf` table stores.
//! * [`Path`] — explicit segments that can carry cubic Béziers, which is what
//!   CFF (Type 2) charstrings produce. [`glyph_path`] is the unified entry
//!   point; [`Path::to_contours`] flattens cubics for the quadratic-only
//!   consumers and [`path_bbox`] computes exact bounds (curve extrema, not
//!   control points).
//!
//! [`glyph_contours`] dispatches on the face: `glyf` faces keep the original
//! parser, CFF faces are flattened from [`Path`].

use crate::error::{Error, Result};
use crate::font::Face;

/// One outline point in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub on: bool,
}

/// A glyph as a list of closed contours.
pub type Contours = Vec<Vec<Point>>;

/// A bare coordinate in font units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// One path segment, relative to the previous point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line { to: Pt },
    Quad { ctrl: Pt, to: Pt },
    Cubic { c1: Pt, c2: Pt, to: Pt },
}

impl Seg {
    /// The segment's end point.
    pub fn end(&self) -> Pt {
        match self {
            Self::Line { to } | Self::Quad { to, .. } | Self::Cubic { to, .. } => *to,
        }
    }
}

/// A closed contour: a start point followed by segments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contour {
    pub start: Pt,
    pub segs: Vec<Seg>,
}

/// A glyph outline as closed contours of lines and (cubic or quadratic) curves.
pub type Path = Vec<Contour>;

impl Contour {
    /// Flattens the contour into on-curve points, `steps` line segments per
    /// curve. Cubics (CFF) and quadratics (`glyf`) are both accepted.
    pub fn flatten(&self, steps: usize) -> Vec<Point> {
        let steps = steps.max(1);
        let mut points = vec![Point { x: self.start.x, y: self.start.y, on: true }];
        let mut cur = self.start;
        for seg in &self.segs {
            match *seg {
                Seg::Line { to } => {
                    push_point(&mut points, to);
                }
                Seg::Quad { ctrl, to } => {
                    for i in 1..=steps {
                        let t = i as f32 / steps as f32;
                        push_point(&mut points, quad_at(cur, ctrl, to, t));
                    }
                }
                Seg::Cubic { c1, c2, to } => {
                    for i in 1..=steps {
                        let t = i as f32 / steps as f32;
                        push_point(&mut points, cubic_at(cur, c1, c2, to, t));
                    }
                }
            }
            cur = seg.end();
        }
        // A contour is implicitly closed; drop a duplicated closing point so
        // `raster`'s close-the-loop pass adds exactly one edge.
        if points.len() > 1 && points.last() == Some(&points[0]) {
            points.pop();
        }
        points
    }
}

fn push_point(points: &mut Vec<Point>, p: Pt) {
    let last = points.last().copied();
    if last.map(|q| q.x == p.x && q.y == p.y).unwrap_or(false) {
        return;
    }
    points.push(Point { x: p.x, y: p.y, on: true });
}

fn quad_at(a: Pt, c: Pt, b: Pt, t: f32) -> Pt {
    let u = 1.0 - t;
    Pt { x: u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, y: u * u * a.y + 2.0 * u * t * c.y + t * t * b.y }
}

fn cubic_at(a: Pt, c1: Pt, c2: Pt, b: Pt, t: f32) -> Pt {
    let u = 1.0 - t;
    let (u2, t2) = (u * u, t * t);
    Pt {
        x: u2 * u * a.x + 3.0 * u2 * t * c1.x + 3.0 * u * t2 * c2.x + t2 * t * b.x,
        y: u2 * u * a.y + 3.0 * u2 * t * c1.y + 3.0 * u * t2 * c2.y + t2 * t * b.y,
    }
}

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME: u8 = 0x10;
const Y_SAME: u8 = 0x20;

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn bei16(d: &[u8], at: usize) -> Option<i16> {
    be16(d, at).map(|v| v as i16)
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn f2dot14(v: i16) -> f32 {
    v as f32 / 16384.0
}

/// Outline of `gid` in font units, as quadratic contours.
///
/// `glyf` faces return the stored on/off-curve points unchanged; CFF faces are
/// flattened into on-curve points (24 line segments per cubic).
pub fn glyph_contours(face: &Face<'_>, gid: u16) -> Result<Contours> {
    if face.has_glyf() {
        return glyf_contours(face, gid);
    }
    if face.cff().is_some() {
        return Ok(path_to_contours(&glyph_path(face, gid)?));
    }
    Err(Error::MissingTable("glyf/CFF"))
}

/// Unified outline of `gid` in font units: `glyf` quadratics or CFF cubics.
pub fn glyph_path(face: &Face<'_>, gid: u16) -> Result<Path> {
    glyph_path_at(face, gid, 0)
}

fn glyph_path_at(face: &Face<'_>, gid: u16, depth: usize) -> Result<Path> {
    if depth > 4 {
        return Err(Error::BadFont("seac nesting"));
    }
    if face.has_glyf() {
        return Ok(contours_to_path(&glyf_contours(face, gid)?));
    }
    let cff = face.cff().ok_or(Error::MissingTable("glyf/CFF"))?;
    let (path, seac) = cff.outline(gid)?;
    match seac {
        None => Ok(path),
        Some(seac) => {
            let mut out = path;
            if let Some(base) = standard_encoding_char(seac.bchar)
                && let Some(gid) = face.glyph_index(base)
            {
                out.extend(glyph_path_at(face, gid, depth + 1)?);
            }
            if let Some(accent) = standard_encoding_char(seac.achar)
                && let Some(gid) = face.glyph_index(accent)
            {
                let mut accent_path = glyph_path_at(face, gid, depth + 1)?;
                translate_path(&mut accent_path, seac.adx, seac.ady);
                out.extend(accent_path);
            }
            Ok(out)
        }
    }
}

/// Exact bounding box of a path, `(x_min, y_min, x_max, y_max)`, using curve
/// extrema rather than control points. `None` for an empty path.
pub fn path_bbox(path: &Path) -> Option<(f32, f32, f32, f32)> {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    {
        let mut add = |x: f64, y: f64| {
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        };
        for contour in path {
            let start = (contour.start.x as f64, contour.start.y as f64);
            add(start.0, start.1);
            let mut cur = start;
            for seg in &contour.segs {
                match *seg {
                    Seg::Line { to } => add(to.x as f64, to.y as f64),
                    Seg::Quad { ctrl, to } => {
                        quad_bounds(cur, (ctrl.x as f64, ctrl.y as f64), (to.x as f64, to.y as f64), &mut add);
                    }
                    Seg::Cubic { c1, c2, to } => {
                        cubic_bounds(
                            cur,
                            (c1.x as f64, c1.y as f64),
                            (c2.x as f64, c2.y as f64),
                            (to.x as f64, to.y as f64),
                            &mut add,
                        );
                    }
                }
                cur = (seg.end().x as f64, seg.end().y as f64);
            }
        }
    }
    bounds.map(|(x0, y0, x1, y1)| (x0 as f32, y0 as f32, x1 as f32, y1 as f32))
}

/// Bounding box of one glyph, in font units.
pub fn glyph_bbox(face: &Face<'_>, gid: u16) -> Result<Option<(f32, f32, f32, f32)>> {
    Ok(path_bbox(&glyph_path(face, gid)?))
}

/// Converts quadratic `glyf` contours into an explicit [`Path`].
fn contours_to_path(contours: &Contours) -> Path {
    let mut path = Vec::with_capacity(contours.len());
    for contour in contours {
        if contour.len() < 2 {
            continue;
        }
        let first = contour[0];
        let last = contour[contour.len() - 1];
        let start = if first.on {
            first
        } else if last.on {
            last
        } else {
            Point { x: (last.x + first.x) * 0.5, y: (last.y + first.y) * 0.5, on: true }
        };
        let mut out = Contour { start: Pt::new(start.x, start.y), segs: Vec::new() };
        let mut cur = start;
        let mut ctrl: Option<Point> = None;
        for p in contour.iter().chain(std::iter::once(&start)) {
            if p.on {
                if let Some(c) = ctrl.take() {
                    out.segs.push(Seg::Quad { ctrl: Pt::new(c.x, c.y), to: Pt::new(p.x, p.y) });
                } else if p.x != cur.x || p.y != cur.y {
                    out.segs.push(Seg::Line { to: Pt::new(p.x, p.y) });
                }
                cur = *p;
            } else if let Some(c) = ctrl.take() {
                // Two consecutive off-curve points imply an on-curve midpoint
                // between them (the TrueType rule); emit that quadratic.
                let mid = Pt::new((c.x + p.x) * 0.5, (c.y + p.y) * 0.5);
                out.segs.push(Seg::Quad { ctrl: Pt::new(c.x, c.y), to: mid });
                cur = *p;
                ctrl = Some(*p);
            } else {
                ctrl = Some(*p);
            }
        }
        if !out.segs.is_empty() {
            path.push(out);
        }
    }
    path
}

const CFF_FLATTEN_STEPS: usize = 24;

/// Flattens a [`Path`] into the on-curve-point contour shape the rasteriser and
/// optical kerning consume.
pub fn path_to_contours(path: &Path) -> Contours {
    path.iter()
        .map(|c| c.flatten(CFF_FLATTEN_STEPS))
        .filter(|c| c.len() > 1)
        .collect()
}

fn translate_path(path: &mut Path, dx: f32, dy: f32) {
    for contour in path.iter_mut() {
        contour.start.x += dx;
        contour.start.y += dy;
        for seg in contour.segs.iter_mut() {
            match seg {
                Seg::Line { to } => {
                    to.x += dx;
                    to.y += dy;
                }
                Seg::Quad { ctrl, to } => {
                    ctrl.x += dx;
                    ctrl.y += dy;
                    to.x += dx;
                    to.y += dy;
                }
                Seg::Cubic { c1, c2, to } => {
                    c1.x += dx;
                    c1.y += dy;
                    c2.x += dx;
                    c2.y += dy;
                    to.x += dx;
                    to.y += dy;
                }
            }
        }
    }
}

/// Quadratic Bézier bounds: endpoints plus the axis extrema in `(0, 1)`.
fn quad_bounds(from: (f64, f64), ctrl: (f64, f64), to: (f64, f64), add: &mut impl FnMut(f64, f64)) {
    add(to.0, to.1);
    for axis in 0..2 {
        let p0 = if axis == 0 { from.0 } else { from.1 };
        let c = if axis == 0 { ctrl.0 } else { ctrl.1 };
        let p1 = if axis == 0 { to.0 } else { to.1 };
        let denom = p0 - 2.0 * c + p1;
        if denom.abs() < 1e-12 {
            continue;
        }
        let t = (p0 - c) / denom;
        if t > 0.0 && t < 1.0 {
            let u = 1.0 - t;
            let x = u * u * from.0 + 2.0 * u * t * ctrl.0 + t * t * to.0;
            let y = u * u * from.1 + 2.0 * u * t * ctrl.1 + t * t * to.1;
            add(x, y);
        }
    }
}

/// Cubic Bézier bounds: endpoints plus both axis extrema in `(0, 1)`.
fn cubic_bounds(
    from: (f64, f64),
    c1: (f64, f64),
    c2: (f64, f64),
    to: (f64, f64),
    add: &mut impl FnMut(f64, f64),
) {
    add(to.0, to.1);
    for axis in 0..2 {
        let p0 = if axis == 0 { from.0 } else { from.1 };
        let p1 = if axis == 0 { c1.0 } else { c1.1 };
        let p2 = if axis == 0 { c2.0 } else { c2.1 };
        let p3 = if axis == 0 { to.0 } else { to.1 };
        // B'(t) = 3 (a t^2 + b t + c)
        let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
        let b = 2.0 * (p0 - 2.0 * p1 + p2);
        let c = p1 - p0;
        let mut roots = [0.0f64; 2];
        let count = solve_quadratic(a, b, c, &mut roots);
        for t in roots.iter().take(count) {
            let t = *t;
            if t <= 0.0 || t >= 1.0 {
                continue;
            }
            let u = 1.0 - t;
            let (u2, t2) = (u * u, t * t);
            let x = u2 * u * from.0 + 3.0 * u2 * t * c1.0 + 3.0 * u * t2 * c2.0 + t2 * t * to.0;
            let y = u2 * u * from.1 + 3.0 * u2 * t * c1.1 + 3.0 * u * t2 * c2.1 + t2 * t * to.1;
            add(x, y);
        }
    }
}

fn solve_quadratic(a: f64, b: f64, c: f64, out: &mut [f64; 2]) -> usize {
    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return 0;
        }
        out[0] = -c / b;
        return 1;
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return 0;
    }
    let sqrt = disc.sqrt();
    out[0] = (-b + sqrt) / (2.0 * a);
    out[1] = (-b - sqrt) / (2.0 * a);
    2
}

/// Adobe StandardEncoding code -> Unicode character (best effort; `seac`
/// accents are old Type 1 names that a Unicode cmap usually covers).
fn standard_encoding_char(code: u8) -> Option<char> {
    const TABLE: [char; 256] = [
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        ' ', '!', '\u{22}', '#', '$', '%', '&', '\u{2019}',
        '(', ')', '*', '+', ',', '-', '.', '/',
        '0', '1', '2', '3', '4', '5', '6', '7',
        '8', '9', ':', ';', '<', '=', '>', '?',
        '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G',
        'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
        'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W',
        'X', 'Y', 'Z', '[', '\u{5c}', ']', '^', '_',
        '\u{2018}', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
        'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
        'p', 'q', 'r', 's', 't', 'u', 'v', 'w',
        'x', 'y', 'z', '{', '|', '}', '~', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\u{a1}', '\u{a2}', '\u{a3}', '\u{2044}', '\u{a5}', '\u{192}', '\u{a7}',
        '\u{a4}', '\u{27}', '\u{201c}', '\u{ab}', '\u{2039}', '\u{203a}', '\u{fb01}', '\u{fb02}',
        '\0', '\u{2013}', '\u{2020}', '\u{2021}', '\u{b7}', '\0', '\u{b6}', '\u{2022}',
        '\u{201a}', '\u{201e}', '\u{201d}', '\u{bb}', '\u{2026}', '\u{2030}', '\0', '\u{bf}',
        '\0', '`', '\u{b4}', '\u{2c6}', '\u{2dc}', '\u{af}', '\u{2d8}', '\u{2d9}',
        '\u{a8}', '\0', '\u{2da}', '\u{b8}', '\0', '\u{2dd}', '\u{2db}', '\u{2c7}',
        '\u{2014}', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\0', '\0', '\0', '\0', '\0', '\0', '\0',
        '\0', '\u{c6}', '\0', '\u{aa}', '\0', '\0', '\0', '\0',
        '\u{141}', '\u{d8}', '\u{152}', '\u{ba}', '\0', '\0', '\0', '\0',
        '\0', '\u{e6}', '\0', '\0', '\0', '\u{131}', '\0', '\0',
        '\u{142}', '\u{f8}', '\u{153}', '\u{df}', '\0', '\0', '\0', '\0',
    ];
    match TABLE[code as usize] {
        '\0' => None,
        c => Some(c),
    }
}

fn glyf_contours(face: &Face<'_>, gid: u16) -> Result<Contours> {
    glyph_contours_at(face, gid, 0)
}

fn glyph_contours_at(face: &Face<'_>, gid: u16, depth: usize) -> Result<Contours> {
    let loca = face.table(*b"loca").ok_or(Error::MissingTable("loca"))?;
    let glyf = face.table(*b"glyf").ok_or(Error::MissingTable("glyf"))?;
    let head = face.table(*b"head").ok_or(Error::MissingTable("head"))?;
    let long = bei16(head, 50).unwrap_or(0) != 0;
    let gid = gid as usize;
    let (start, end) = if long {
        (
            be32(loca, gid * 4).ok_or(Error::OutOfBounds { needed: gid * 4 + 4, len: loca.len() })? as usize,
            be32(loca, (gid + 1) * 4).ok_or(Error::OutOfBounds { needed: gid * 4 + 8, len: loca.len() })? as usize,
        )
    } else {
        (
            be16(loca, gid * 2).ok_or(Error::OutOfBounds { needed: gid * 2 + 2, len: loca.len() })? as usize * 2,
            be16(loca, (gid + 1) * 2).ok_or(Error::OutOfBounds { needed: gid * 2 + 4, len: loca.len() })? as usize * 2,
        )
    };
    if end <= start || end > glyf.len() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    parse_glyph(face, &glyf[start..end], depth, &mut out)?;
    Ok(out)
}

fn parse_glyph(face: &Face<'_>, data: &[u8], depth: usize, out: &mut Contours) -> Result<()> {
    if depth > 8 {
        return Err(Error::BadFont("composite glyph nesting"));
    }
    let num = bei16(data, 0).ok_or(Error::OutOfBounds { needed: 2, len: data.len() })?;
    if num >= 0 {
        return parse_simple(data, num as usize, out);
    }
    parse_composite(face, data, depth, out)
}

fn parse_simple(data: &[u8], contours: usize, out: &mut Contours) -> Result<()> {
    if contours == 0 {
        return Ok(());
    }
    let mut at = 10usize;
    let mut ends = Vec::with_capacity(contours);
    for _ in 0..contours {
        ends.push(be16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as usize);
        at += 2;
    }
    let points = ends.last().copied().unwrap_or(0) + 1;
    let instr = be16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as usize;
    at += 2 + instr;

    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let f = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })?;
        at += 1;
        flags.push(f);
        if f & REPEAT != 0 {
            let n = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })?;
            at += 1;
            for _ in 0..n {
                flags.push(f);
            }
        }
    }
    flags.truncate(points);

    let mut xs = Vec::with_capacity(points);
    let mut x = 0f32;
    for &f in &flags {
        if f & X_SHORT != 0 {
            let d = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })? as f32;
            at += 1;
            x += if f & X_SAME != 0 { d } else { -d };
        } else if f & X_SAME == 0 {
            x += bei16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as f32;
            at += 2;
        }
        xs.push(x);
    }
    let mut ys = Vec::with_capacity(points);
    let mut y = 0f32;
    for &f in &flags {
        if f & Y_SHORT != 0 {
            let d = *data.get(at).ok_or(Error::OutOfBounds { needed: at + 1, len: data.len() })? as f32;
            at += 1;
            y += if f & Y_SAME != 0 { d } else { -d };
        } else if f & Y_SAME == 0 {
            y += bei16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })? as f32;
            at += 2;
        }
        ys.push(y);
    }

    let mut start = 0usize;
    for end in ends {
        let contour: Vec<Point> = (start..=end.min(points.saturating_sub(1)))
            .map(|i| Point { x: xs[i], y: ys[i], on: flags[i] & ON_CURVE != 0 })
            .collect();
        if contour.len() > 1 {
            out.push(contour);
        }
        start = end + 1;
    }
    Ok(())
}

fn parse_composite(face: &Face<'_>, data: &[u8], depth: usize, out: &mut Contours) -> Result<()> {
    let mut at = 10usize;
    loop {
        let flags = be16(data, at).ok_or(Error::OutOfBounds { needed: at + 2, len: data.len() })?;
        let gid = be16(data, at + 2).ok_or(Error::OutOfBounds { needed: at + 4, len: data.len() })?;
        at += 4;
        let word = flags & ARG_1_AND_2_ARE_WORDS != 0;
        let (r1, r2) = if word {
            (be16(data, at).unwrap_or(0) as u32, be16(data, at + 2).unwrap_or(0) as u32)
        } else {
            (*data.get(at).unwrap_or(&0) as u32, *data.get(at + 1).unwrap_or(&0) as u32)
        };
        at += if word { 4 } else { 2 };
        let xy = flags & ARGS_ARE_XY_VALUES != 0;
        // Offset interpretation: signed XY, or unsigned point indices (parent, child).
        let (sx, sy) = if word { (r1 as u16 as i16 as f32, r2 as u16 as i16 as f32) } else { (r1 as u8 as i8 as f32, r2 as u8 as i8 as f32) };

        let (mut a, mut b, mut c, mut d) = (1.0f32, 0.0f32, 0.0f32, 1.0f32);
        if flags & WE_HAVE_A_SCALE != 0 {
            let s = f2dot14(bei16(data, at).unwrap_or(16384));
            at += 2;
            a = s;
            d = s;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            a = f2dot14(bei16(data, at).unwrap_or(16384));
            d = f2dot14(bei16(data, at + 2).unwrap_or(16384));
            at += 4;
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            a = f2dot14(bei16(data, at).unwrap_or(16384));
            b = f2dot14(bei16(data, at + 2).unwrap_or(0));
            c = f2dot14(bei16(data, at + 4).unwrap_or(0));
            d = f2dot14(bei16(data, at + 6).unwrap_or(16384));
            at += 8;
        }

        let mut inner: Contours = glyph_contours_at(face, gid, depth + 1)?
            .into_iter()
            .map(|contour| contour.into_iter().map(|p| Point { x: a * p.x + c * p.y, y: b * p.x + d * p.y, on: p.on }).collect())
            .collect();
        let (dx, dy) = if xy {
            (sx, sy)
        } else {
            let parent = out.iter().flatten().nth(r1 as usize).ok_or(Error::BadFont("composite parent point index"))?;
            let child = inner.iter().flatten().nth(r2 as usize).ok_or(Error::BadFont("composite component point index"))?;
            (parent.x - child.x, parent.y - child.y)
        };
        for p in inner.iter_mut().flatten() {
            p.x += dx;
            p.y += dy;
        }
        for contour in inner {
            out.push(contour);
        }

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    Ok(())
}
